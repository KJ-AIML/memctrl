//! Explainable local retrieval. Cache publication and scoring share one writer snapshot.
//!
//! `retrieve` ranks with SQLite FTS5 (see `search`). `keyword_retrieve` is the 1.x keyword
//! scorer, kept as the Python-parity oracle and as the baseline for `memctrl eval`.
use crate::model::{Memory, RetrievalResult, TreeNode};
use crate::sanitize::{sanitize_text, validate_identifier};
use crate::store::{AtomicWrite, MemoryStore};
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const STOP: &str = "the and for are but not you all can had her was one our out day get has him his how its may new now old see two who boy did she use way many oil sit set run eat far sea eye ago off too any say man try ask end why let put own tell very when much would there their what said each which will about could other after first never these think where being every great might shall still those while this that with have from they know want been good come made find give work life even here look down most long last only over such take than them well were time year also back just like into because people some make really thing things should through does doing done is be do must ought";

fn stem(word: &str) -> String {
    let word = match word {
        "authentication" | "authenticate" => "auth",
        "deploying" | "deployment" => "deploy",
        "database" => "db",
        "middleware" => "middle",
        other => other,
    };
    for (suffix, replacement) in [
        ("ational", "ate"),
        ("tional", "tion"),
        ("iveness", "ive"),
        ("ization", "ize"),
        ("fulness", "ful"),
        ("ousness", "ous"),
        ("biliti", "ble"),
        ("ation", "ate"),
        ("ition", "ite"),
        ("ator", "ate"),
        ("ment", ""),
        ("ness", ""),
        ("ance", ""),
        ("ence", ""),
        ("ible", ""),
        ("able", ""),
        ("ing", ""),
        ("ies", "y"),
        ("ied", "y"),
        ("s", ""),
        ("ed", ""),
        ("er", ""),
        ("ly", ""),
        ("ily", "y"),
    ] {
        if word.ends_with(suffix) && word.chars().count() > suffix.len() + 2 {
            return format!("{}{replacement}", &word[..word.len() - suffix.len()]);
        }
    }
    word.into()
}

fn words(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.chars().count() >= 2 && !STOP.split_whitespace().any(|stop| stop == *w))
        .map(stem)
        .collect()
}

fn score_node<'a>(
    node: &TreeNode,
    depth: usize,
    query: &BTreeSet<String>,
    lookup: &BTreeMap<&str, &'a Memory>,
    scores: &mut BTreeMap<&'a str, (f64, &'a Memory)>,
) {
    let structural = query.intersection(&words(&node.title)).count()
        + query.intersection(&words(&node.summary)).count();
    for id in &node.memory_ids {
        if let Some(memory) = lookup.get(id.as_str()) {
            if memory.content.is_empty() {
                continue;
            }
            let score = structural + 3 * query.intersection(&words(&memory.content)).count();
            if score == 0 {
                continue;
            }
            let boost = match memory.layer.as_str() {
                "project" => 2.0,
                "user" => 1.2,
                _ => 1.0,
            };
            let score = score as f64 * boost * memory.confidence + 1.0 / (depth + 1) as f64;
            if score >= 1.0
                && scores
                    .get(memory.id.as_str())
                    .map(|(existing, _)| score > *existing)
                    .unwrap_or(true)
            {
                scores.insert(&memory.id, (score, memory));
            }
        }
    }
    for child in &node.children {
        score_node(child, depth + 1, query, lookup, scores);
    }
}

/// Legacy 1.x scorer: counts stemmed words shared with the query over the memory tree.
pub fn keyword_retrieve(
    query: &str,
    tree: &TreeNode,
    memories: &[Memory],
    top_k: usize,
) -> RetrievalResult {
    let query = words(query);
    if query.is_empty() {
        return RetrievalResult {
            trace: vec!["no_keywords".into()],
            ..Default::default()
        };
    }
    let lookup: BTreeMap<_, _> = memories
        .iter()
        .filter(|m| m.eligible())
        .map(|m| (m.id.as_str(), m))
        .collect();
    if lookup.is_empty() {
        return RetrievalResult {
            trace: vec!["no_eligible_memories".into()],
            ..Default::default()
        };
    }
    let mut scores = BTreeMap::new();
    score_node(tree, 0, &query, &lookup, &mut scores);
    let mut scores: Vec<_> = scores.into_values().collect();
    scores.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    let gate = scores.first().map(|s| s.0 * 0.5).unwrap_or(1.0);
    let mut seen = BTreeSet::new();
    let selected: Vec<_> = scores
        .into_iter()
        .filter(|(score, m)| *score >= gate && seen.insert(m.content.clone()))
        .take(top_k)
        .collect();
    if selected.is_empty() {
        return RetrievalResult {
            trace: vec!["root".into(), "no_match".into()],
            ..Default::default()
        };
    }
    let confidence = ((selected.iter().map(|s| s.0).sum::<f64>() / selected.len() as f64 / 10.0)
        .min(1.0)
        * 100.0)
        .round_ties_even()
        / 100.0;
    RetrievalResult {
        facts: selected
            .iter()
            .map(|(_, m)| sanitize_text(&m.content))
            .collect(),
        sources: selected
            .iter()
            .map(|(_, m)| sanitize_text(&m.source))
            .collect(),
        memory_ids: selected.iter().map(|(_, m)| m.id.clone()).collect(),
        confidence,
        trace: vec![
            "root".into(),
            "keyword_search".into(),
            selected[0].1.layer.clone(),
        ],
        ..Default::default()
    }
}

/// Retrieval settings derived from `.memoryrc`, plus semantic search from your user config.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub alias_groups: Vec<Vec<String>>,
    /// `[retrieval] generic_words`: more words that rank but can't qualify a memory alone.
    pub generic_words: Vec<String>,
    /// `[retrieval] builtin_generic`: whether the built-in generic words apply.
    pub builtin_generic: bool,
    /// Matches by meaning, fused with lexical results when your user config turns it on.
    pub semantic: Option<crate::semantic::Engine>,
}
impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            alias_groups: Vec::new(),
            generic_words: Vec::new(),
            builtin_generic: true,
            semantic: None,
        }
    }
}
impl SearchOptions {
    pub fn from_rules(rules: &crate::rules::Rules) -> Self {
        Self {
            alias_groups: rules.alias_groups(),
            generic_words: rules.generic_words.clone(),
            builtin_generic: rules.builtin_generic,
            semantic: rules.semantic.clone(),
        }
    }
    fn fingerprint(&self) -> String {
        let mut joined: Vec<String> = self.alias_groups.iter().map(|g| g.join("\u{1f}")).collect();
        joined.push(format!(
            "generic:{}:{}",
            self.builtin_generic,
            self.generic_words.join("\u{1f}")
        ));
        if let Some(engine) = &self.semantic {
            joined.push(format!("semantic:{}", engine.fingerprint()));
        }
        let refs: Vec<&str> = joined.iter().map(String::as_str).collect();
        fingerprint(&refs)
    }
}

/// Ranks eligible memories for `query` with FTS5 (BM25 words plus trigrams for spaceless
/// scripts) without touching access counts, caches or provenance.
pub fn lexical_retrieve(
    store: &MemoryStore,
    query: &str,
    layer: Option<&str>,
    top_k: usize,
    options: &SearchOptions,
) -> Result<RetrievalResult> {
    let parsed = crate::search::parse_query_with(
        query,
        &options.alias_groups,
        &options.generic_words,
        options.builtin_generic,
    );
    if parsed.is_empty() {
        return Ok(RetrievalResult {
            trace: vec!["no_keywords".into()],
            ..Default::default()
        });
    }
    // Only topic-free words ("summarize the project"): an honest "nothing specific" beats
    // returning whatever mentions a project.
    if parsed.only_generic() {
        return Ok(RetrievalResult {
            trace: vec![METHOD.to_string(), "generic_only".into()],
            ..Default::default()
        });
    }
    let (hits, candidates) = crate::search::search(store, &parsed, layer, top_k)?;
    let mut trace = vec![
        METHOD.to_string(),
        format!("terms:{}", parsed.terms.len()),
        format!("runs:{}", parsed.runs.len()),
        format!("candidates:{candidates}"),
    ];
    if hits.is_empty() {
        trace.push("no_match".into());
        return Ok(RetrievalResult {
            trace,
            ..Default::default()
        });
    }
    let confidence = hits
        .iter()
        .map(|h| h.memory.confidence * h.why.coverage)
        .sum::<f64>()
        / hits.len() as f64;
    Ok(RetrievalResult {
        facts: hits
            .iter()
            .map(|h| sanitize_text(&h.memory.content))
            .collect(),
        sources: hits
            .iter()
            .map(|h| sanitize_text(&h.memory.source))
            .collect(),
        memory_ids: hits.iter().map(|h| h.memory.id.clone()).collect(),
        confidence: (confidence * 100.0).round() / 100.0,
        trace,
        why: hits.into_iter().map(|h| h.why).collect(),
    })
}

/// Lexical results fused with matches by meaning, as `query` returns them when semantic search
/// is on: reciprocal rank fusion of the lexical ranking and the memories above the model's
/// similarity threshold, so a question with no close memory still gets nothing. Falls back to
/// lexical results alone when the model couldn't be reached.
pub fn hybrid_retrieve(
    store: &MemoryStore,
    query: &str,
    layer: Option<&str>,
    top_k: usize,
    options: &SearchOptions,
    prepared: &crate::semantic::Prepared,
) -> Result<RetrievalResult> {
    let Some(engine) = options.semantic.as_ref().filter(|_| prepared.ok()) else {
        let mut result = lexical_retrieve(store, query, layer, top_k, options)?;
        result.trace.extend(prepared.trace());
        return Ok(result);
    };
    let wide = top_k.max(crate::semantic::VECTOR_HITS);
    let lexical = lexical_retrieve(store, query, layer, wide, options)?;
    let vector = prepared.above(0, engine.threshold(), crate::semantic::VECTOR_HITS);
    let mut trace = lexical.trace.clone();
    trace.extend(prepared.trace());
    trace.push(format!("vector_hits:{}", vector.len()));
    let similarity: BTreeMap<&str, f32> = vector.iter().map(|(id, s)| (id.as_str(), *s)).collect();
    let fused = crate::semantic::fuse(&[
        lexical.memory_ids.clone(),
        vector.iter().map(|(id, _)| id.clone()).collect(),
    ]);
    let mut result = RetrievalResult {
        trace,
        ..Default::default()
    };
    let mut strength = 0.0;
    for id in fused {
        if result.memory_ids.len() == top_k {
            break;
        }
        let label = similarity
            .get(id.as_str())
            .map(|s| format!("similar meaning ({s:.2})"));
        let (fact, source, why, weight) = match lexical.memory_ids.iter().position(|x| *x == id) {
            Some(i) => {
                let mut why = lexical.why[i].clone();
                why.matched.extend(label);
                let weight = why.coverage;
                (
                    lexical.facts[i].clone(),
                    lexical.sources[i].clone(),
                    why,
                    weight,
                )
            }
            None => {
                let Some(memory) = store.get_memory(&id, false)?.filter(Memory::eligible) else {
                    continue;
                };
                let s = f64::from(similarity[id.as_str()]);
                let why = crate::model::Why {
                    memory_id: id.clone(),
                    score: (s * 1000.0).round() / 1000.0,
                    coverage: 0.0,
                    matched: label.into_iter().collect(),
                    freshness: None,
                };
                (
                    sanitize_text(&memory.content),
                    sanitize_text(&memory.source),
                    why,
                    s,
                )
            }
        };
        if result.facts.contains(&fact) {
            continue;
        }
        strength += weight;
        result.facts.push(fact);
        result.sources.push(source);
        result.memory_ids.push(id);
        result.why.push(why);
    }
    if !result.facts.is_empty() {
        result.confidence = (strength / result.facts.len() as f64 * 100.0).round() / 100.0;
    }
    Ok(result)
}

/// Retrieval with built-in aliases. Prefer `retrieve_with` when `.memoryrc` is available.
pub fn retrieve(
    store: &mut MemoryStore,
    query: &str,
    layer: Option<&str>,
    top_k: usize,
) -> Result<RetrievalResult> {
    let options = SearchOptions::from_rules(&crate::rules::Rules::default());
    retrieve_with(store, query, layer, top_k, &options)
}

/// What the cache keeps: ids and explanations, never memory text. A hit re-reads the memories
/// in the same snapshot, so it is small and can't serve text that changed.
#[derive(Serialize, Deserialize)]
struct Cached {
    memory_ids: Vec<String>,
    why: Vec<crate::model::Why>,
    trace: Vec<String>,
    confidence: f64,
}
impl Cached {
    fn of(result: &RetrievalResult) -> Self {
        Self {
            memory_ids: result.memory_ids.clone(),
            why: result.why.clone(),
            trace: result.trace.clone(),
            confidence: result.confidence,
        }
    }
    /// The full result, or `None` when any memory is no longer eligible.
    fn load(self, store: &MemoryStore) -> Result<Option<RetrievalResult>> {
        let mut result = RetrievalResult {
            memory_ids: self.memory_ids,
            why: self.why,
            trace: self.trace,
            confidence: self.confidence,
            ..Default::default()
        };
        for id in &result.memory_ids {
            let Some(memory) = store.get_memory(id, false)? else {
                return Ok(None);
            };
            result.facts.push(sanitize_text(&memory.content));
            result.sources.push(sanitize_text(&memory.source));
        }
        Ok(Some(result))
    }
}

/// Cache entries larger than this aren't stored (a `limit` of thousands of ids).
const MAX_CACHED_BYTES: usize = 32 * 1024;

/// Cached retrieval that records access and provenance. The search runs in a read snapshot,
/// without the write lock; access counts, provenance and the cache entry are written afterwards
/// as best-effort bookkeeping, skipped when another writer is busy or the database is read-only,
/// so a query never waits on or fails for them.
pub fn retrieve_with(
    store: &mut MemoryStore,
    query: &str,
    layer: Option<&str>,
    top_k: usize,
    options: &SearchOptions,
) -> Result<RetrievalResult> {
    if let Some(layer) = layer {
        validate_identifier(layer, "layer")?;
    }
    let store: &MemoryStore = store;
    // The model is asked before the read snapshot opens, so a slow model never holds it. Results
    // that depend on the model aren't cached: the query embedding is most of their cost anyway.
    let prepared = options
        .semantic
        .as_ref()
        .map(|engine| engine.prepare(store, &[&sanitize_text(query)], layer));
    let snapshot = crate::store::ReadSnapshot::new(&store.conn)?;
    let revision = store.revision()?;
    let mut eligible = store.eligible_ids(layer)?;
    eligible.sort_unstable();
    let eligible: Vec<&str> = eligible.iter().map(String::as_str).collect();
    // Eligibility may change with time alone. Binding the eligible set prevents
    // both stale inclusion and stale omission at validity/expiration boundaries.
    // A fingerprint keeps the key small however large the store grows.
    let key = serde_json::to_string(&(
        RETRIEVAL_VERSION,
        CACHE_FORMAT,
        sanitize_text(query),
        layer,
        top_k,
        eligible.len(),
        fingerprint(&eligible),
        options.fingerprint(),
    ))?;
    let timestamp = Utc::now().timestamp_millis() as f64 / 1000.0;
    let cached: Option<String> = match prepared {
        Some(_) => None,
        None => store.conn.query_row("SELECT result_json FROM rust_query_cache WHERE key=?1 AND revision=?2 AND cached_at>=?3", params![key, revision, timestamp - 300.0], |r| r.get(0)).optional()?,
    };
    let hit = match cached.map(|json| serde_json::from_str::<Cached>(&json)) {
        Some(Ok(entry)) => entry.load(store)?,
        _ => None,
    };
    let fresh = hit.is_none();
    let result = match hit {
        Some(result) => result,
        None if eligible.is_empty() => RetrievalResult {
            trace: vec!["no_eligible_memories".into()],
            ..Default::default()
        },
        None => match &prepared {
            Some(prepared) => hybrid_retrieve(
                store,
                &sanitize_text(query),
                layer,
                top_k,
                options,
                prepared,
            )?,
            None => lexical_retrieve(store, &sanitize_text(query), layer, top_k, options)?,
        },
    };
    let searched = eligible.len();
    let method = match &prepared {
        Some(p) if p.ok() => SEMANTIC_METHOD,
        _ => METHOD,
    };
    snapshot.finish()?;
    store.side_write(|| {
        for (index, id) in result.memory_ids.iter().enumerate() {
            // Access counts lift a memory in the brief. One found only through the start of
            // longer words (`conn` in `connections`) isn't counted, so a loose match can't climb
            // there. A short Thai or Chinese word is matched as a substring too, and counts.
            let fragments_only = result.why.get(index).is_some_and(|why| {
                !why.matched.is_empty()
                    && why.matched.iter().all(|m| {
                        m.ends_with("(generic)")
                            || m.strip_suffix(" (substring)")
                                .is_some_and(|word| !crate::search::spaceless_text(word))
                    })
            });
            if !fragments_only {
                store.record_access(id)?;
            }
        }
        store.save_provenance(query, &result, searched, method)?;
        // Publish only if nothing changed since the snapshot; access columns don't bump the
        // revision, so an unchanged revision proves the result still describes the store.
        let current = store.revision()?;
        let entry = serde_json::to_string(&Cached::of(&result))?;
        if fresh && prepared.is_none() && current == revision && entry.len() <= MAX_CACHED_BYTES {
            store.conn.execute("INSERT INTO rust_query_cache(key,result_json,revision,cached_at) VALUES(?1,?2,?3,?4) ON CONFLICT(key) DO UPDATE SET result_json=excluded.result_json,revision=excluded.revision,cached_at=excluded.cached_at", params![key, entry, current, timestamp])?;
        }
        store.conn.execute(
            "DELETE FROM rust_query_cache WHERE revision<>?1 OR cached_at<?2",
            params![current, timestamp - 300.0],
        )?;
        store.conn.execute("DELETE FROM rust_query_cache WHERE key NOT IN (SELECT key FROM rust_query_cache ORDER BY cached_at DESC,key LIMIT 256)", [])?;
        Ok(())
    });
    Ok(result)
}

/// Publishes memories chosen by an LLM. The model ran on an earlier snapshot, so eligibility is
/// re-checked under the writer lock: anything refuted, expired or superseded while the request
/// was in flight is dropped, and access plus provenance are recorded atomically.
pub fn publish_selection(
    store: &mut MemoryStore,
    query: &str,
    layer: Option<&str>,
    selected: &[String],
    top_k: usize,
) -> Result<RetrievalResult> {
    if let Some(layer) = layer {
        validate_identifier(layer, "layer")?;
    }
    let store: &MemoryStore = store;
    under_writer_lock(store, || {
        let memories = store.list_memories(layer, false)?;
        let mut result = RetrievalResult::default();
        for m in memories
            .iter()
            .filter(|m| m.eligible() && selected.contains(&m.id))
            .take(top_k)
        {
            result.memory_ids.push(m.id.clone());
            result.facts.push(m.content.clone());
            result.sources.push(m.source.clone());
            result.confidence += m.confidence;
            store.record_access(&m.id)?;
        }
        if !result.facts.is_empty() {
            result.confidence /= result.facts.len() as f64;
        }
        result.trace.push("explicit_llm_selection".into());
        store.save_provenance(query, &result, memories.len(), "llm_selection")?;
        Ok(result)
    })
}

/// Part of every cache key: bump when scoring changes so upgrades never serve old results.
const RETRIEVAL_VERSION: &str = "lexical-v3";
/// Part of every cache key: what an entry holds (ids only). 2.1 entries held full results and
/// never match these keys, in either direction.
const CACHE_FORMAT: &str = "ids-v1";
/// Provenance method recorded for `retrieve`.
pub const METHOD: &str = "lexical_bm25";
/// Provenance method when matches by meaning were fused in.
pub const SEMANTIC_METHOD: &str = "lexical_bm25+semantic";

/// Stable 64-bit FNV-1a over sorted ids. Not a security boundary: entries are also bound to
/// the store revision and a 300s TTL, and ids are random v4 UUIDs.
fn fingerprint(ids: &[&str]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for id in ids {
        for byte in id.bytes().chain(std::iter::once(b'\n')) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// Runs `operation` under the writer lock (`AtomicWrite` begins IMMEDIATE at top level), so no
/// other connection can change the snapshot between reading and publishing.
fn under_writer_lock<T>(store: &MemoryStore, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let guard = AtomicWrite::new(&store.conn)?;
    let result = operation()?;
    guard.commit()?;
    Ok(result)
}
