//! Optional semantic search. An embedding model you run (Ollama, LM Studio, llama.cpp, or any
//! OpenAI-compatible `/embeddings` server) turns memories and queries into vectors, and matches
//! by meaning are fused with lexical search, so a question worded unlike the memory still finds
//! it. A similarity threshold keeps vague questions answered with nothing.
//!
//! Off unless your user config has a `[semantic]` section. A repository's `.memoryrc` can't turn
//! it on or choose where text goes, and endpoints off this machine need `allow_remote = true`,
//! because the prompt hook sends every request to the model.
//!
//! Vectors are a cache of memory text in the store's `semantic_vectors` table, created on first
//! use. Triggers drop a memory's vector when the memory is deleted or its text changes, whatever
//! memctrl version makes the change. Vectors of queries and prompts are used once, never stored.
use crate::sanitize::{sanitize_text, validate_identifier};
use crate::store::{AtomicWrite, MemoryStore};
use anyhow::{anyhow, bail, ensure, Context, Result};
use reqwest::{redirect::Policy, Url};
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// Memories without a vector that a search embeds on the spot; `semantic index` does the rest.
pub const LAZY_LIMIT: usize = 16;
/// Texts per request when indexing.
const BATCH: usize = 32;
/// Vector matches fused with lexical results per query.
pub const VECTOR_HITS: usize = 10;
/// Reciprocal rank fusion constant, the usual 60.
const RRF_K: f64 = 60.0;
/// Text sent per input, in characters; models truncate long input anyway.
const MAX_INPUT_CHARS: usize = 4000;
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_BASE_URL: &str = "http://localhost:11434/v1";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);
/// Indexing waits longer: a batch of documents, and a model that may still be loading.
const INDEX_TIMEOUT: Duration = Duration::from_secs(120);

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS semantic_vectors(
  memory_id TEXT NOT NULL,
  space TEXT NOT NULL,
  vector BLOB NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY(memory_id, space)
) WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS semantic_vectors_delete AFTER DELETE ON memories
BEGIN DELETE FROM semantic_vectors WHERE memory_id=old.id; END;
CREATE TRIGGER IF NOT EXISTS semantic_vectors_update AFTER UPDATE OF content ON memories
WHEN old.content IS NOT new.content
BEGIN DELETE FROM semantic_vectors WHERE memory_id=old.id; END;";

/// What a model family expects around text, and thresholds measured for it.
struct Preset {
    /// Lowercase substring of the model name.
    pattern: &'static str,
    query_prefix: &'static str,
    document_prefix: &'static str,
    /// Measured with `memctrl semantic tune` on the built-in suite (EmbeddingGemma 300M and
    /// multilingual-e5-small as quantized ONNX); `None` until someone has.
    threshold: Option<f64>,
    recall_threshold: Option<f64>,
}

const GEMMA_QUERY: &str = "task: search result | query: ";
const GEMMA_DOCUMENT: &str = "title: none | text: ";
/// More specific patterns first.
const PRESETS: &[Preset] = &[
    Preset {
        pattern: "embeddinggemma",
        query_prefix: GEMMA_QUERY,
        document_prefix: GEMMA_DOCUMENT,
        threshold: Some(0.455),
        recall_threshold: Some(0.56),
    },
    Preset {
        pattern: "embedding-gemma",
        query_prefix: GEMMA_QUERY,
        document_prefix: GEMMA_DOCUMENT,
        threshold: Some(0.455),
        recall_threshold: Some(0.56),
    },
    Preset {
        pattern: "multilingual-e5-small",
        query_prefix: "query: ",
        document_prefix: "passage: ",
        threshold: Some(0.849),
        recall_threshold: Some(0.85),
    },
    Preset {
        pattern: "e5-",
        query_prefix: "query: ",
        document_prefix: "passage: ",
        threshold: None,
        recall_threshold: None,
    },
    Preset {
        pattern: "nomic-embed-text",
        query_prefix: "search_query: ",
        document_prefix: "search_document: ",
        threshold: None,
        recall_threshold: None,
    },
    Preset {
        pattern: "mxbai-embed",
        query_prefix: "Represent this sentence for searching relevant passages: ",
        document_prefix: "",
        threshold: None,
        recall_threshold: None,
    },
    Preset {
        pattern: "qwen3-embedding",
        query_prefix: "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery:",
        document_prefix: "",
        threshold: None,
        recall_threshold: None,
    },
];

fn preset(model: &str) -> Option<&'static Preset> {
    let model = model.to_ascii_lowercase();
    PRESETS.iter().find(|p| model.contains(p.pattern))
}

/// Local servers `semantic setup` looks for: OpenAI-compatible, on this machine, in this order.
pub const LOCAL_SERVERS: &[(&str, &str)] = &[
    ("Ollama", "http://localhost:11434/v1"),
    ("LM Studio", "http://localhost:1234/v1"),
];

/// How well a model suits search by meaning, judged by its name: 2 when memctrl has measured
/// thresholds for it, 1 for another embedding model it knows, 0 for any other name with "embed"
/// in it. `None` when the name doesn't say it's an embedding model.
pub fn suitability(model: &str) -> Option<u8> {
    match preset(model) {
        Some(known) if known.threshold.is_some() => Some(2),
        Some(_) => Some(1),
        None => model.to_ascii_lowercase().contains("embed").then_some(0),
    }
}

/// The models an OpenAI-compatible server on this machine lists at `{base_url}/models`.
pub fn list_models(base_url: &str, timeout: Duration) -> Result<Vec<String>> {
    let mut url = Url::parse(base_url).with_context(|| format!("{base_url} is not a URL"))?;
    ensure!(
        is_loopback(&url) && matches!(url.scheme(), "http" | "https"),
        "semantic setup only uses servers on this machine; for {base_url}, add [semantic] to your user config yourself"
    );
    let path = format!("{}/models", url.path().trim_end_matches('/'));
    url.set_path(&path);
    let reply = std::thread::Builder::new()
        .name("memctrl-models".into())
        .spawn(move || -> Result<Value> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async move {
                let client = reqwest::Client::builder()
                    .connect_timeout(timeout)
                    .timeout(timeout)
                    .redirect(Policy::none())
                    .build()?;
                let response = client.get(url).send().await?;
                ensure!(
                    response.status().is_success(),
                    "the server answered {}",
                    response.status()
                );
                Ok(response.json::<Value>().await?)
            })
        })?
        .join()
        .map_err(|_| anyhow!("listing models failed"))??;
    Ok(reply["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| model["id"].as_str().map(str::to_owned))
        .collect())
}

/// `[semantic]` in your user config:
///
/// ```toml
/// [semantic]
/// model = "embeddinggemma"                  # required
/// base_url = "http://localhost:11434/v1"    # Ollama; LM Studio is http://localhost:1234/v1
/// # threshold = 0.455          # similarity a match by meaning alone needs (see semantic tune)
/// # recall_threshold = 0.56    # the same for the prompt hook, which nobody asked
/// # query_prefix / document_prefix, dimensions, timeout_ms = 3000
/// # api_key_env = "OPENAI_API_KEY" and allow_remote = true, for a server off this machine
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub base_url: String,
    pub model: String,
    pub query_prefix: String,
    pub document_prefix: String,
    pub threshold: Option<f64>,
    pub recall_threshold: Option<f64>,
    /// Keep only the first this many dimensions (for Matryoshka models such as EmbeddingGemma).
    pub dimensions: Option<usize>,
    pub api_key_env: Option<String>,
    pub allow_remote: bool,
    pub timeout: Duration,
}

impl Settings {
    /// Parses `[semantic]`; `None` when it says `enabled = false`.
    pub fn parse(value: &toml::Value) -> Result<Option<Self>> {
        let table = value.as_table().context("semantic must be a table")?;
        const KNOWN: [&str; 11] = [
            "enabled",
            "base_url",
            "model",
            "query_prefix",
            "document_prefix",
            "threshold",
            "recall_threshold",
            "dimensions",
            "api_key_env",
            "allow_remote",
            "timeout_ms",
        ];
        if let Some(key) = table.keys().find(|k| !KNOWN.contains(&k.as_str())) {
            bail!("unknown setting semantic.{key}");
        }
        let string = |key: &str| -> Result<Option<String>> {
            table
                .get(key)
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .with_context(|| format!("semantic.{key} must be a string"))
                })
                .transpose()
        };
        let number = |key: &str| -> Result<Option<f64>> {
            table
                .get(key)
                .map(|v| {
                    v.as_float()
                        .or_else(|| v.as_integer().map(|i| i as f64))
                        .filter(|x| (-1.0..=1.0).contains(x))
                        .with_context(|| format!("semantic.{key} must be a number from -1 to 1"))
                })
                .transpose()
        };
        let flag = |key: &str| -> Result<Option<bool>> {
            table
                .get(key)
                .map(|v| {
                    v.as_bool()
                        .with_context(|| format!("semantic.{key} must be true or false"))
                })
                .transpose()
        };
        if flag("enabled")? == Some(false) {
            return Ok(None);
        }
        let model = string("model")?
            .filter(|m| !m.trim().is_empty())
            .context("semantic.model is required, e.g. model = \"embeddinggemma\"")?;
        validate_identifier(&model, "semantic.model")?;
        let preset = preset(&model);
        let positive = |key: &str| -> Result<Option<u64>> {
            table
                .get(key)
                .map(|v| {
                    v.as_integer()
                        .filter(|n| *n > 0)
                        .map(|n| n as u64)
                        .with_context(|| format!("semantic.{key} must be a positive integer"))
                })
                .transpose()
        };
        let settings = Self {
            base_url: string("base_url")?.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            query_prefix: string("query_prefix")?
                .unwrap_or_else(|| preset.map_or("", |p| p.query_prefix).into()),
            document_prefix: string("document_prefix")?
                .unwrap_or_else(|| preset.map_or("", |p| p.document_prefix).into()),
            threshold: number("threshold")?,
            recall_threshold: number("recall_threshold")?,
            dimensions: positive("dimensions")?.map(|n| n as usize),
            api_key_env: string("api_key_env")?,
            allow_remote: flag("allow_remote")?.unwrap_or(false),
            timeout: positive("timeout_ms")?.map_or(DEFAULT_TIMEOUT, Duration::from_millis),
            model,
        };
        settings.endpoint()?;
        if let Some(name) = &settings.api_key_env {
            validate_identifier(name, "semantic.api_key_env")?;
        }
        Ok(Some(settings))
    }

    /// The `/embeddings` URL, refusing anything that could leak text or credentials.
    fn endpoint(&self) -> Result<Url> {
        let mut url = Url::parse(&self.base_url).context("semantic.base_url is not a URL")?;
        ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "semantic.base_url must not contain credentials, a query or a fragment"
        );
        ensure!(
            matches!(url.scheme(), "http" | "https"),
            "semantic.base_url must be http or https"
        );
        if !is_loopback(&url) {
            ensure!(
                self.allow_remote,
                "semantic.base_url {} is not on this machine. Memories and every prompt would be sent there; set allow_remote = true in [semantic] if that's what you want",
                self.base_url
            );
            ensure!(
                url.scheme() == "https",
                "semantic.base_url must use https for a server off this machine"
            );
        }
        let path = format!("{}/embeddings", url.path().trim_end_matches('/'));
        url.set_path(&path);
        Ok(url)
    }

    fn api_key(&self) -> Result<Option<String>> {
        match &self.api_key_env {
            None => Ok(None),
            Some(name) => std::env::var(name)
                .ok()
                .filter(|k| !k.trim().is_empty())
                .map(Some)
                .with_context(|| format!("environment variable {name} is not set")),
        }
    }

    /// The model's thresholds: yours, else the built-in values measured for the model.
    pub fn thresholds(&self) -> (Option<f64>, Option<f64>) {
        let preset = preset(&self.model);
        let threshold = self.threshold.or(preset.and_then(|p| p.threshold));
        let recall = self
            .recall_threshold
            .or(preset.and_then(|p| p.recall_threshold));
        (threshold, recall)
    }

    /// Non-secret description for `semantic status`.
    pub fn summary(&self) -> Value {
        let (threshold, recall_threshold) = self.thresholds();
        json!({"model": self.model, "base_url": self.base_url, "threshold": threshold,
               "recall_threshold": recall_threshold, "query_prefix": self.query_prefix,
               "document_prefix": self.document_prefix, "dimensions": self.dimensions,
               "allow_remote": self.allow_remote, "timeout_ms": self.timeout.as_millis() as u64,
               "key_variable": self.api_key_env})
    }
}

fn is_loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    })
}

/// Talks to the embedding server. Deliberately not Debug: it may hold an API key.
#[derive(Clone)]
pub struct Client {
    endpoint: Url,
    model: String,
    api_key: Option<String>,
    dimensions: Option<usize>,
}

impl Client {
    pub fn new(settings: &Settings) -> Result<Self> {
        Ok(Self {
            endpoint: settings.endpoint()?,
            model: settings.model.clone(),
            api_key: settings.api_key()?,
            dimensions: settings.dimensions,
        })
    }

    /// Unit-length vectors for `inputs`, in order. The request runs on its own thread with its
    /// own runtime, so it works the same from the CLI and from inside the MCP server's runtime.
    pub fn embed(&self, inputs: Vec<String>, timeout: Duration) -> Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let count = inputs.len();
        let body = json!({"model": self.model, "input": inputs});
        let endpoint = self.endpoint.clone();
        let key = self.api_key.clone();
        let worker = std::thread::Builder::new()
            .name("memctrl-embed".into())
            .spawn(move || -> Result<Value> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                runtime.block_on(async move {
                    let client = reqwest::Client::builder()
                        .connect_timeout(timeout.min(Duration::from_secs(5)))
                        .timeout(timeout)
                        .redirect(Policy::none())
                        .build()
                        .map_err(|_| anyhow!("unable to configure the embedding HTTP client"))?;
                    let mut request = client.post(endpoint).json(&body);
                    if let Some(key) = key {
                        request = request.bearer_auth(key);
                    }
                    let mut response = request.send().await.map_err(describe)?;
                    let status = response.status();
                    let mut bytes = Vec::new();
                    while let Some(chunk) = response.chunk().await.map_err(describe)? {
                        bytes.extend_from_slice(&chunk);
                        ensure!(
                            bytes.len() <= MAX_RESPONSE_BYTES,
                            "embedding response is too large"
                        );
                    }
                    if !status.is_success() {
                        let text: String =
                            String::from_utf8_lossy(&bytes).chars().take(300).collect();
                        bail!(
                            "embedding server answered {status}: {}",
                            sanitize_text(&text)
                        );
                    }
                    serde_json::from_slice(&bytes)
                        .map_err(|_| anyhow!("embedding server returned invalid JSON"))
                })
            })?;
        let value = worker
            .join()
            .map_err(|_| anyhow!("embedding request failed"))??;
        let mut rows: Vec<(usize, &Value)> = value["data"]
            .as_array()
            .context("embedding response has no data array")?
            .iter()
            .enumerate()
            .map(|(i, item)| (item["index"].as_u64().map_or(i, |n| n as usize), item))
            .collect();
        rows.sort_by_key(|(index, _)| *index);
        ensure!(
            rows.len() == count,
            "embedding server returned {} vectors for {count} inputs",
            rows.len()
        );
        let mut vectors = Vec::with_capacity(count);
        for (_, item) in rows {
            let mut vector: Vec<f32> = item["embedding"]
                .as_array()
                .context("embedding is not an array of numbers")?
                .iter()
                .map(|x| x.as_f64().map(|x| x as f32).filter(|x| x.is_finite()))
                .collect::<Option<_>>()
                .context("embedding is not an array of numbers")?;
            if let Some(dimensions) = self.dimensions {
                vector.truncate(dimensions);
            }
            vectors.push(normalized(vector)?);
        }
        let dimensions = vectors[0].len();
        ensure!(
            vectors.iter().all(|v| v.len() == dimensions),
            "embedding server returned vectors of different lengths"
        );
        Ok(vectors)
    }
}

fn describe(error: reqwest::Error) -> anyhow::Error {
    if error.is_timeout() {
        anyhow!("the embedding server didn't answer in time")
    } else if error.is_connect() {
        anyhow!("can't reach the embedding server")
    } else {
        anyhow!(
            "embedding request failed: {}",
            sanitize_text(&error.to_string())
        )
    }
}

fn normalized(mut vector: Vec<f32>) -> Result<Vec<f32>> {
    ensure!(
        !vector.is_empty(),
        "embedding server returned an empty vector"
    );
    let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    ensure!(norm > 0.0, "embedding server returned a zero vector");
    for x in &mut vector {
        *x /= norm;
    }
    Ok(vector)
}

fn clip(text: &str) -> String {
    text.chars().take(MAX_INPUT_CHARS).collect()
}

fn encode(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn decode(bytes: &[u8]) -> Option<Vec<f32>> {
    (!bytes.is_empty() && bytes.len().is_multiple_of(4)).then(|| {
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect()
    })
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Semantic search, ready to use: a client plus what the model needs around text.
#[derive(Clone)]
pub struct Engine(Arc<Inner>);
struct Inner {
    client: Client,
    model: String,
    query_prefix: String,
    document_prefix: String,
    threshold: f64,
    recall_threshold: f64,
    timeout: Duration,
    /// Identifies the vector space: vectors from another model, prefix or size never mix.
    space: String,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("model", &self.0.model)
            .field("threshold", &self.0.threshold)
            .finish()
    }
}

impl Engine {
    /// Fails when the model has no measured threshold and you set none: without one, every
    /// question would return something.
    pub fn new(settings: &Settings) -> Result<Self> {
        let (threshold, recall) = settings.thresholds();
        let threshold = threshold.with_context(|| {
            format!(
                "semantic search needs a threshold for {}; run `memctrl semantic tune` and add the threshold it suggests to [semantic]",
                settings.model
            )
        })?;
        let space = crate::citations::digest(&format!(
            "{}\u{1f}{}\u{1f}{:?}",
            settings.model, settings.document_prefix, settings.dimensions
        ));
        Ok(Self(Arc::new(Inner {
            client: Client::new(settings)?,
            model: settings.model.clone(),
            query_prefix: settings.query_prefix.clone(),
            document_prefix: settings.document_prefix.clone(),
            threshold,
            // Unasked additions need more than a ranked answer to a question does.
            recall_threshold: recall.unwrap_or(threshold).max(threshold),
            timeout: settings.timeout,
            space: space[..16].to_string(),
        })))
    }

    pub fn threshold(&self) -> f64 {
        self.0.threshold
    }

    pub fn recall_threshold(&self) -> f64 {
        self.0.recall_threshold
    }

    pub fn model(&self) -> &str {
        &self.0.model
    }

    /// Changes whenever results could: another model, prefix, size or threshold.
    pub fn fingerprint(&self) -> String {
        format!(
            "{}:{}:{}",
            self.0.space, self.0.threshold, self.0.recall_threshold
        )
    }

    fn document(&self, content: &str) -> String {
        format!("{}{}", self.0.document_prefix, clip(content))
    }

    fn query(&self, text: &str) -> String {
        format!("{}{}", self.0.query_prefix, clip(text))
    }

    /// Embeds `queries` and up to `LAZY_LIMIT` eligible memories that have no vector yet, in
    /// one request, and scores every eligible memory with a vector against each query.
    pub fn prepare(&self, store: &MemoryStore, queries: &[&str], layer: Option<&str>) -> Prepared {
        match self.try_prepare(store, queries, layer) {
            Ok(prepared) => prepared,
            Err(error) => Prepared {
                error: Some(format!("{error:#}")),
                ..Prepared::default()
            },
        }
    }

    fn try_prepare(
        &self,
        store: &MemoryStore,
        queries: &[&str],
        layer: Option<&str>,
    ) -> Result<Prepared> {
        let eligible = store.eligible_ids(layer)?;
        if eligible.is_empty() {
            return Ok(Prepared {
                scores: vec![Vec::new(); queries.len()],
                ..Prepared::default()
            });
        }
        let mut vectors = load(store, &self.0.space, &eligible)?;
        let missing: Vec<&String> = eligible
            .iter()
            .filter(|id| !vectors.contains_key(*id))
            .collect();
        let mut lazy = Vec::new();
        for id in missing.iter().take(LAZY_LIMIT) {
            if let Some(memory) = store.get_memory(id, false)? {
                lazy.push((memory.id, self.document(&memory.content)));
            }
        }
        let mut inputs: Vec<String> = queries.iter().map(|q| self.query(q)).collect();
        inputs.extend(lazy.iter().map(|(_, text)| text.clone()));
        let timeout = if lazy.is_empty() {
            self.0.timeout
        } else {
            self.0.timeout * 3
        };
        let mut embedded = self.0.client.embed(inputs, timeout)?;
        let documents = embedded.split_off(queries.len());
        let fresh: Vec<(String, Vec<f32>)> =
            lazy.into_iter().map(|(id, _)| id).zip(documents).collect();
        let saved = store.side_write(|| save(store, &self.0.space, &fresh));
        let unindexed = missing.len() - fresh.len();
        vectors.extend(fresh);
        if let Some(dimensions) = embedded.first().map(Vec::len) {
            vectors.retain(|_, v| v.len() == dimensions);
        }
        let scores = embedded
            .iter()
            .map(|query| {
                let mut ranked: Vec<(String, f32)> = vectors
                    .iter()
                    .map(|(id, v)| (id.clone(), dot(query, v)))
                    .collect();
                ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                ranked
            })
            .collect();
        Ok(Prepared {
            scores,
            unindexed,
            unsaved: !saved,
            error: None,
        })
    }

    /// Embeds every eligible memory that has no vector yet, in batches.
    pub fn index(&self, store: &MemoryStore, layer: Option<&str>) -> Result<Value> {
        let eligible = store.eligible_ids(layer)?;
        let have = load(store, &self.0.space, &eligible)?;
        let missing: Vec<&String> = eligible
            .iter()
            .filter(|id| !have.contains_key(*id))
            .collect();
        let started = std::time::Instant::now();
        let mut indexed = 0;
        for batch in missing.chunks(BATCH) {
            let mut ids = Vec::new();
            let mut texts = Vec::new();
            for id in batch {
                if let Some(memory) = store.get_memory(id, false)? {
                    ids.push(memory.id);
                    texts.push(self.document(&memory.content));
                }
            }
            let vectors = self.0.client.embed(texts, INDEX_TIMEOUT)?;
            let items: Vec<(String, Vec<f32>)> = ids.into_iter().zip(vectors).collect();
            let guard = AtomicWrite::new(&store.conn)?;
            save(store, &self.0.space, &items)?;
            guard.commit()?;
            indexed += items.len();
        }
        Ok(
            json!({"indexed": indexed, "already_indexed": have.len(), "eligible": eligible.len(),
                  "seconds": (started.elapsed().as_secs_f64() * 10.0).round() / 10.0}),
        )
    }

    /// How many eligible memories have a vector in this model's space.
    pub fn coverage(&self, store: &MemoryStore, layer: Option<&str>) -> Result<(usize, usize)> {
        let eligible = store.eligible_ids(layer)?;
        Ok((load(store, &self.0.space, &eligible)?.len(), eligible.len()))
    }

    /// Embeds one short text, to check the server answers.
    pub fn ping(&self) -> Result<usize> {
        let vectors = self
            .0
            .client
            .embed(vec![self.query("ping")], self.0.timeout * 3)?;
        Ok(vectors[0].len())
    }
}

/// The similarity of every eligible memory that has a vector to each query, best first.
#[derive(Debug, Default)]
pub struct Prepared {
    pub scores: Vec<Vec<(String, f32)>>,
    /// Eligible memories without a vector, left for `memctrl semantic index`.
    pub unindexed: usize,
    /// Vectors made on the spot couldn't be saved (busy or read-only store).
    pub unsaved: bool,
    /// Why semantic search couldn't run; results are then lexical only.
    pub error: Option<String>,
}

impl Prepared {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }

    /// Memories similar enough to query `index`, best first, at most `limit`.
    pub fn above(&self, index: usize, threshold: f64, limit: usize) -> Vec<(String, f32)> {
        self.scores.get(index).map_or_else(Vec::new, |ranked| {
            ranked
                .iter()
                .take_while(|(_, s)| f64::from(*s) >= threshold)
                .take(limit)
                .cloned()
                .collect()
        })
    }

    /// Similarity of one memory to query `index`, if it has a vector.
    pub fn similarity(&self, index: usize, id: &str) -> Option<f32> {
        self.scores
            .get(index)?
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, s)| *s)
    }

    /// Notes for a retrieval trace.
    pub fn trace(&self) -> Vec<String> {
        let mut trace = Vec::new();
        match &self.error {
            Some(error) => trace.push(format!("semantic_unavailable: {}", sanitize_text(error))),
            None => trace.push("semantic".into()),
        }
        if self.unindexed > 0 {
            trace.push(format!(
                "semantic_unindexed:{} (run `memctrl semantic index`)",
                self.unindexed
            ));
        }
        trace
    }
}

/// Reciprocal rank fusion: each list adds 1/(60 + rank) to its items. Ties go to the earlier
/// list (lexical, whose word matches are the more precise evidence), then the higher rank.
pub fn fuse(lists: &[Vec<String>]) -> Vec<String> {
    struct Fused {
        score: f64,
        /// Where the item first appeared: (list, rank).
        first: (usize, usize),
    }
    let mut fused: HashMap<&str, Fused> = HashMap::new();
    for (list_index, list) in lists.iter().enumerate() {
        for (rank, id) in list.iter().enumerate() {
            fused
                .entry(id.as_str())
                .or_insert(Fused {
                    score: 0.0,
                    first: (list_index, rank),
                })
                .score += 1.0 / (RRF_K + rank as f64 + 1.0);
        }
    }
    let mut ranked: Vec<(&str, Fused)> = fused.into_iter().collect();
    ranked.sort_by(|a, b| {
        b.1.score
            .total_cmp(&a.1.score)
            .then_with(|| a.1.first.cmp(&b.1.first))
    });
    ranked.into_iter().map(|(id, _)| id.to_string()).collect()
}

fn has_table(store: &MemoryStore) -> Result<bool> {
    Ok(store.conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='semantic_vectors')",
        [],
        |r| r.get(0),
    )?)
}

/// Vectors in `space` for the memories in `ids`.
fn load(store: &MemoryStore, space: &str, ids: &[String]) -> Result<HashMap<String, Vec<f32>>> {
    if ids.is_empty() || !has_table(store)? {
        return Ok(HashMap::new());
    }
    let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let mut statement = store
        .conn
        .prepare("SELECT memory_id, vector FROM semantic_vectors WHERE space=?1")?;
    let rows = statement.query_map(params![space], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (id, bytes) = row?;
        if wanted.contains(id.as_str()) {
            if let Some(vector) = decode(&bytes) {
                out.insert(id, vector);
            }
        }
    }
    Ok(out)
}

/// Stores vectors, creating the table and its triggers on first use. Call inside a write.
fn save(store: &MemoryStore, space: &str, items: &[(String, Vec<f32>)]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    store.conn.execute_batch(SCHEMA)?;
    let now = crate::model::now();
    let mut statement = store.conn.prepare(
        "INSERT INTO semantic_vectors(memory_id, space, vector, created_at)
         SELECT ?1, ?2, ?3, ?4 WHERE EXISTS(SELECT 1 FROM memories WHERE id=?1)
         ON CONFLICT(memory_id, space) DO UPDATE SET vector=excluded.vector, created_at=excluded.created_at",
    )?;
    for (id, vector) in items {
        statement.execute(params![id, space, encode(vector), now])?;
    }
    Ok(())
}

/// Removes every vector, the table and its triggers, returning how many vectors there were.
pub fn clear(store: &MemoryStore) -> Result<usize> {
    if !has_table(store)? {
        return Ok(0);
    }
    let guard = AtomicWrite::new(&store.conn)?;
    let count: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM semantic_vectors", [], |r| r.get(0))?;
    store.conn.execute_batch(
        "DROP TRIGGER IF EXISTS semantic_vectors_delete;
         DROP TRIGGER IF EXISTS semantic_vectors_update;
         DROP TABLE IF EXISTS semantic_vectors;",
    )?;
    guard.commit()?;
    Ok(count as usize)
}

/// Every vector the store holds, across models.
pub fn stored(store: &MemoryStore) -> Result<usize> {
    if !has_table(store)? {
        return Ok(0);
    }
    let count: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM semantic_vectors", [], |r| r.get(0))?;
    Ok(count as usize)
}

/// Measures thresholds for the configured model on an eval suite: `threshold` balances finding
/// answers against saying nothing to unanswerable questions, as the published comparison did;
/// `recall_threshold` is just above anything an unanswerable question reached.
pub fn tune(settings: &Settings, suite: &crate::eval::Suite, top_k: usize) -> Result<Value> {
    let client = Client::new(settings)?;
    let documents: Vec<String> = suite
        .memories
        .iter()
        .map(|m| format!("{}{}", settings.document_prefix, clip(&m.content)))
        .collect();
    let queries: Vec<String> = suite
        .cases
        .iter()
        .map(|c| format!("{}{}", settings.query_prefix, clip(&c.query)))
        .collect();
    let mut doc_vectors = Vec::new();
    for batch in documents.chunks(BATCH) {
        doc_vectors.extend(client.embed(batch.to_vec(), INDEX_TIMEOUT)?);
    }
    let mut query_vectors = Vec::new();
    for batch in queries.chunks(BATCH) {
        query_vectors.extend(client.embed(batch.to_vec(), INDEX_TIMEOUT)?);
    }
    let keys: Vec<&str> = suite.memories.iter().map(|m| m.key.as_str()).collect();
    let ranked: Vec<Vec<(f32, &str)>> = query_vectors
        .iter()
        .map(|q| {
            let mut row: Vec<(f32, &str)> = doc_vectors
                .iter()
                .zip(&keys)
                .map(|(d, k)| (dot(q, d), *k))
                .collect();
            row.sort_by(|a, b| b.0.total_cmp(&a.0));
            row
        })
        .collect();
    let score = |tau: f32| {
        let (mut hits, mut positives, mut declined, mut negatives) = (0, 0, 0, 0);
        for (case, row) in suite.cases.iter().zip(&ranked) {
            let got: Vec<&str> = row
                .iter()
                .take(top_k)
                .filter(|(s, _)| *s >= tau)
                .map(|(_, k)| *k)
                .collect();
            if case.expect.is_empty() {
                negatives += 1;
                declined += usize::from(got.is_empty());
            } else {
                positives += 1;
                hits += usize::from(got.iter().any(|k| case.expect.iter().any(|e| e == k)));
            }
        }
        let rate = |n: usize, d: usize| (d > 0).then(|| n as f64 / d as f64);
        (rate(hits, positives), rate(declined, negatives))
    };
    let mut candidates: Vec<f32> = ranked
        .iter()
        .filter_map(|row| row.first().map(|(s, _)| (s * 1000.0).round() / 1000.0))
        .collect();
    candidates.sort_by(f32::total_cmp);
    candidates.dedup();
    let mut best = (-1.0, 0.0f32);
    for tau in candidates {
        let (hit, declined) = score(tau);
        let value = (hit.unwrap_or(0.0) + declined.unwrap_or(0.0)) / 2.0;
        if value > best.0 {
            best = (value, tau);
        }
    }
    let threshold = best.1;
    let highest_negative = suite
        .cases
        .iter()
        .zip(&ranked)
        .filter(|(c, _)| c.expect.is_empty())
        .filter_map(|(_, row)| row.first().map(|(s, _)| *s))
        .fold(f32::MIN, f32::max);
    // Just above the closest vague question, but similarity never exceeds 1.
    let recall_threshold = if highest_negative > f32::MIN {
        ((highest_negative * 100.0).floor() + 1.0) / 100.0
    } else {
        threshold
    }
    .max(threshold)
    .min(1.0);
    let round = |x: f32| (f64::from(x) * 1000.0).round() / 1000.0;
    let (hit, declined) = score(threshold);
    Ok(json!({
        "model": settings.model,
        "suite": suite.name,
        "threshold": round(threshold),
        "recall_threshold": round(recall_threshold),
        "at_threshold": {"hit_at_k": hit.map(|x| (x * 1000.0).round() / 1000.0),
                          "negative_accuracy": declined.map(|x| (x * 1000.0).round() / 1000.0)},
        "config": format!("threshold = {}\nrecall_threshold = {}", round(threshold), round(recall_threshold)),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(text: &str) -> Result<Option<Settings>> {
        Settings::parse(&toml::from_str::<toml::Value>(text)?)
    }

    #[test]
    fn setup_prefers_models_with_measured_thresholds() {
        assert_eq!(suitability("embeddinggemma:latest"), Some(2));
        assert_eq!(
            suitability("text-embedding-embeddinggemma-300m-qat"),
            Some(2)
        );
        assert_eq!(suitability("multilingual-e5-small"), Some(2));
        assert_eq!(suitability("nomic-embed-text:v1.5"), Some(1));
        assert_eq!(suitability("snowflake-arctic-embed2"), Some(0));
        assert_eq!(suitability("llama3.2:3b"), None);
        assert!(
            list_models("https://api.openai.com/v1", Duration::from_secs(1))
                .unwrap_err()
                .to_string()
                .contains("only uses servers on this machine")
        );
    }

    #[test]
    fn settings_use_presets_and_refuse_leaky_endpoints() {
        let s = settings("model = \"embeddinggemma\"").unwrap().unwrap();
        assert_eq!(s.base_url, DEFAULT_BASE_URL);
        assert_eq!(s.query_prefix, GEMMA_QUERY);
        assert_eq!(s.thresholds(), (Some(0.455), Some(0.56)));
        assert!(settings("model = \"m\"\nenabled = false")
            .unwrap()
            .is_none());
        assert!(settings("base_url = \"http://localhost:1234/v1\"").is_err());
        for url in [
            "http://example.com/v1",
            "https://example.com/v1",
            "http://user:pw@localhost:1/v1",
            "http://localhost:1/v1?key=x",
            "file:///etc/passwd",
        ] {
            assert!(
                settings(&format!("model = \"m\"\nbase_url = \"{url}\"")).is_err(),
                "{url}"
            );
        }
        assert!(settings(
            "model = \"m\"\nbase_url = \"https://example.com/v1\"\nallow_remote = true"
        )
        .unwrap()
        .is_some());
        assert!(settings(
            "model = \"m\"\nbase_url = \"http://example.com/v1\"\nallow_remote = true"
        )
        .is_err());
        assert!(settings("model = \"m\"\nthreshold = 2").is_err());
        assert!(settings("model = \"m\"\nthresold = 0.5").is_err());
        let custom = settings("model = \"m\"").unwrap().unwrap();
        assert!(
            Engine::new(&custom).is_err(),
            "a model without a measured threshold needs one"
        );
        let custom = settings("model = \"m\"\nthreshold = 0.5").unwrap().unwrap();
        let engine = Engine::new(&custom).unwrap();
        assert_eq!(engine.recall_threshold(), 0.5);
    }

    #[test]
    fn fusion_rewards_agreement_and_vectors_round_trip() {
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let fused = fuse(&[ids(&["a", "b", "c"]), ids(&["c", "d"])]);
        assert_eq!(fused, ["c", "a", "b", "d"]);
        let v = normalized(vec![3.0, 4.0]).unwrap();
        assert_eq!(decode(&encode(&v)).unwrap(), v);
        assert!((dot(&v, &v) - 1.0).abs() < 1e-6);
        assert!(normalized(vec![0.0, 0.0]).is_err());
        assert!(decode(&[1, 2, 3]).is_none());
    }

    #[test]
    fn vectors_follow_their_memory() -> Result<()> {
        let store = MemoryStore::open(":memory:")?;
        let id = store.insert_memory(crate::model::NewMemory {
            content: "Releases are cut from main".into(),
            ..Default::default()
        })?;
        let ids = vec![id.clone()];
        assert!(load(&store, "s", &ids)?.is_empty(), "no table yet");
        save(&store, "s", &[(id.clone(), vec![1.0, 0.0])])?;
        save(&store, "s", &[("not-a-memory".into(), vec![1.0, 0.0])])?;
        assert_eq!(load(&store, "s", &ids)?.len(), 1);
        assert_eq!(stored(&store)?, 1, "vectors only for memories that exist");
        store.conn.execute(
            "UPDATE memories SET content='Releases are cut from release branches' WHERE id=?1",
            [&id],
        )?;
        assert!(
            load(&store, "s", &ids)?.is_empty(),
            "edited text drops the vector"
        );
        save(&store, "s", &[(id.clone(), vec![1.0, 0.0])])?;
        assert!(store.delete_memory(&id)?);
        assert_eq!(stored(&store)?, 0, "deleting the memory drops its vector");
        save(&store, "s", &[])?;
        assert_eq!(clear(&store)?, 0);
        assert!(!has_table(&store)?);
        Ok(())
    }
}
