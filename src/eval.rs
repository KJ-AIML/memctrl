//! Retrieval evaluation. A suite of memories and queries runs against a throwaway in-memory
//! store, so evaluating never touches a real database. Reports ranking quality, how often
//! unanswerable queries correctly return nothing, the tokens a result would add to an agent's
//! context, and latency.
use crate::context::{approx_tokens, build};
use crate::model::{ClaimType, NewMemory, VerificationState};
use crate::retriever::{hybrid_retrieve, keyword_retrieve, lexical_retrieve, SearchOptions};
use crate::store::MemoryStore;
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

pub const BUILTIN_SUITE: &str = include_str!("../assets/eval/dev-memory.json");

#[derive(Debug, Clone, Deserialize)]
pub struct Suite {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub memories: Vec<SuiteMemory>,
    pub cases: Vec<Case>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SuiteMemory {
    pub key: String,
    pub content: String,
    #[serde(default = "project")]
    pub layer: String,
    #[serde(default)]
    pub claim_type: ClaimType,
    #[serde(default)]
    pub verification_state: VerificationState,
}

fn project() -> String {
    "project".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct Case {
    pub query: String,
    /// Keys of the memories that answer the query. Empty means nothing should be returned.
    pub expect: Vec<String>,
    #[serde(default = "general")]
    pub kind: String,
}

fn general() -> String {
    "general".into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retriever {
    /// FTS5 BM25 + trigrams (what `memctrl query` uses).
    Lexical,
    /// The 1.x keyword scorer, as a baseline.
    Legacy,
    /// Lexical fused with matches by meaning (what `query` uses when `[semantic]` is on).
    Semantic,
    /// What the prompt hook would add for each query (with semantic search when it's on).
    Recall,
}
impl Retriever {
    fn name(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Legacy => "legacy",
            Self::Semantic => "semantic",
            Self::Recall => "recall",
        }
    }
}

pub fn parse_suite(text: &str) -> Result<Suite> {
    let suite: Suite = serde_json::from_str(text).context("eval suite must be valid JSON")?;
    ensure!(!suite.memories.is_empty(), "eval suite has no memories");
    ensure!(!suite.cases.is_empty(), "eval suite has no cases");
    let keys: Vec<&str> = suite.memories.iter().map(|m| m.key.as_str()).collect();
    for case in &suite.cases {
        for key in &case.expect {
            ensure!(
                keys.contains(&key.as_str()),
                "case {:?} expects unknown memory {key:?}",
                case.query
            );
        }
    }
    Ok(suite)
}

#[derive(Default)]
struct Tally {
    positives: usize,
    negatives: usize,
    hit1: usize,
    hitk: usize,
    recall: f64,
    reciprocal_rank: f64,
    precision: f64,
    negatives_passed: usize,
    facts: usize,
    tokens: usize,
}
impl Tally {
    fn summary(&self, cases: usize) -> Value {
        let ratio = |n: f64, d: usize| {
            if d == 0 {
                Value::Null
            } else {
                json!(round(n / d as f64))
            }
        };
        json!({
            "cases": cases,
            "positive_cases": self.positives,
            "negative_cases": self.negatives,
            "hit_at_1": ratio(self.hit1 as f64, self.positives),
            "hit_at_k": ratio(self.hitk as f64, self.positives),
            "recall_at_k": ratio(self.recall, self.positives),
            "mrr": ratio(self.reciprocal_rank, self.positives),
            "precision_at_k": ratio(self.precision, self.positives),
            "negative_accuracy": ratio(self.negatives_passed as f64, self.negatives),
            "avg_facts": ratio(self.facts as f64, cases),
            "avg_tokens": ratio(self.tokens as f64, cases),
        })
    }
}

/// A UUID-shaped id derived from a suite key.
fn stable_id(key: &str) -> String {
    let hex = crate::citations::digest(key);
    format!(
        "{}-{}-4{}-8{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    round(sorted[index])
}

/// Runs every case through each retriever and returns the report.
pub fn run(
    suite: &Suite,
    retrievers: &[Retriever],
    top_k: usize,
    options: &SearchOptions,
) -> Result<Value> {
    ensure!(top_k > 0, "top-k must be at least 1");
    let store = MemoryStore::open(":memory:")?;
    let mut key_of = HashMap::new();
    for memory in &suite.memories {
        // Ids and timestamps derive from the suite, so tie-breaks (by id) and therefore every
        // metric are identical from run to run.
        let id = stable_id(&memory.key);
        store.insert_memory_as(
            NewMemory {
                layer: memory.layer.clone(),
                content: memory.content.clone(),
                claim_type: memory.claim_type,
                verification_state: memory.verification_state,
                source: "eval".into(),
                ..Default::default()
            },
            &id,
            Some("2026-01-01T00:00:00Z"),
        )?;
        key_of.insert(id, memory.key.clone());
    }
    let memories = store.list_memories(None, false)?;
    let tree = crate::tree::build_tree(&memories);
    let engine = if retrievers.contains(&Retriever::Semantic) {
        Some(options.semantic.as_ref().context(
            "the semantic retriever needs semantic search; add [semantic] to your user config",
        )?)
    } else {
        None
    };
    if let Some(engine) = engine.or(options
        .semantic
        .as_ref()
        .filter(|_| retrievers.contains(&Retriever::Recall)))
    {
        engine.index(&store, None)?;
    }
    let mut layers: Vec<String> = suite.memories.iter().map(|m| m.layer.clone()).collect();
    layers.sort();
    layers.dedup();
    let policy = crate::guard::Policy::default();
    let shown = std::collections::HashSet::new();
    let mut results = serde_json::Map::new();
    for &retriever in retrievers {
        let mut total = Tally::default();
        let mut by_kind: BTreeMap<String, (Tally, usize)> = BTreeMap::new();
        let mut latencies = Vec::new();
        let mut failures = Vec::new();
        for case in &suite.cases {
            let started = Instant::now();
            let result = match retriever {
                Retriever::Lexical => lexical_retrieve(&store, &case.query, None, top_k, options)?,
                Retriever::Legacy => keyword_retrieve(&case.query, &tree, &memories, top_k),
                Retriever::Semantic => {
                    let engine = engine.expect("set up above");
                    let prepared = engine.prepare(&store, &[&case.query], None);
                    if let Some(error) = &prepared.error {
                        anyhow::bail!("semantic search failed: {error}");
                    }
                    hybrid_retrieve(&store, &case.query, None, top_k, options, &prepared)?
                }
                Retriever::Recall => {
                    let found = crate::recall::recall(
                        &store,
                        &case.query,
                        &layers,
                        top_k,
                        options,
                        &policy,
                        &|_| HashMap::new(),
                        &shown,
                    )?;
                    crate::model::RetrievalResult {
                        memory_ids: found.entries.iter().map(|e| e.id.clone()).collect(),
                        facts: found.entries.into_iter().map(|e| e.content).collect(),
                        ..Default::default()
                    }
                }
            };
            latencies.push(started.elapsed().as_secs_f64() * 1000.0);
            let got: Vec<String> = result
                .memory_ids
                .iter()
                .map(|id| key_of.get(id).cloned().unwrap_or_else(|| id.clone()))
                .collect();
            let tokens: usize = result.facts.iter().map(|f| approx_tokens(f)).sum();
            let (kind_tally, kind_cases) = by_kind.entry(case.kind.clone()).or_default();
            *kind_cases += 1;
            let mut passed = true;
            for tally in [&mut total, kind_tally] {
                tally.facts += got.len();
                tally.tokens += tokens;
                if case.expect.is_empty() {
                    tally.negatives += 1;
                    if got.is_empty() {
                        tally.negatives_passed += 1;
                    } else {
                        passed = false;
                    }
                    continue;
                }
                tally.positives += 1;
                let relevant = got.iter().filter(|k| case.expect.contains(k)).count();
                if got.first().is_some_and(|k| case.expect.contains(k)) {
                    tally.hit1 += 1;
                }
                if relevant > 0 {
                    tally.hitk += 1;
                } else {
                    passed = false;
                }
                tally.recall += relevant as f64 / case.expect.len() as f64;
                if !got.is_empty() {
                    tally.precision += relevant as f64 / got.len() as f64;
                }
                if let Some(rank) = got.iter().position(|k| case.expect.contains(k)) {
                    tally.reciprocal_rank += 1.0 / (rank as f64 + 1.0);
                }
            }
            if !passed {
                failures.push(json!({"query": case.query, "kind": case.kind, "expected": case.expect, "got": got}));
            }
        }
        latencies.sort_by(f64::total_cmp);
        let mut summary = total.summary(suite.cases.len());
        summary["p50_ms"] = json!(percentile(&latencies, 0.5));
        summary["p95_ms"] = json!(percentile(&latencies, 0.95));
        summary["by_kind"] = by_kind
            .iter()
            .map(|(kind, (tally, cases))| {
                let mut s = tally.summary(*cases);
                let keep = ["cases", "hit_at_k", "mrr", "negative_accuracy"];
                s.as_object_mut()
                    .unwrap()
                    .retain(|k, v| keep.contains(&k.as_str()) && !v.is_null());
                (kind.clone(), s)
            })
            .collect::<serde_json::Map<_, _>>()
            .into();
        summary["failures"] = json!(failures);
        results.insert(retriever.name().into(), summary);
    }
    let layers = vec!["project".to_string(), "user".to_string()];
    let brief = build(&memories, &layers, crate::context::DEFAULT_BUDGET, &|_| {
        None
    });
    Ok(json!({
        "suite": suite.name,
        "top_k": top_k,
        "memories": suite.memories.len(),
        "cases": suite.cases.len(),
        "results": results,
        "brief": {"budget": brief.budget, "shown": brief.entries.len(), "approx_tokens": brief.approx_tokens},
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suites_with_unknown_keys_are_rejected() {
        let bad = r#"{"name":"x","memories":[{"key":"a","content":"alpha"}],"cases":[{"query":"q","expect":["b"]}]}"#;
        assert!(parse_suite(bad).is_err());
        assert!(parse_suite(r#"{"name":"x","memories":[],"cases":[]}"#).is_err());
    }

    #[test]
    fn metrics_count_hits_misses_and_negatives() {
        let suite = parse_suite(
            r#"{"name":"tiny","memories":[{"key":"a","content":"Redis caches sessions"},{"key":"b","content":"Postgres stores orders"}],
                "cases":[{"query":"redis","expect":["a"]},{"query":"orders postgres","expect":["b"]},{"query":"kafka","expect":[]},{"query":"mongo","expect":["a"]}]}"#,
        )
        .unwrap();
        let report = run(&suite, &[Retriever::Lexical], 3, &SearchOptions::default()).unwrap();
        let lexical = &report["results"]["lexical"];
        assert_eq!(lexical["positive_cases"], 3);
        assert_eq!(lexical["hit_at_k"], 0.667);
        assert_eq!(lexical["negative_accuracy"], 1.0);
        assert_eq!(lexical["failures"][0]["query"], "mongo");
    }
}
