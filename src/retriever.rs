//! Explainable local retrieval. Cache publication and scoring share one writer snapshot.
use crate::model::{Memory, RetrievalResult, TreeNode};
use crate::sanitize::{sanitize_text, validate_identifier};
use crate::store::{AtomicWrite, MemoryStore};
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
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
    }
}

pub fn retrieve(
    store: &mut MemoryStore,
    query: &str,
    layer: Option<&str>,
    top_k: usize,
) -> Result<RetrievalResult> {
    if let Some(layer) = layer {
        validate_identifier(layer, "layer")?;
    }
    // An immediate transaction prevents another connection from changing a snapshot
    // between scoring and publication. A cache entry can never acquire a newer revision.
    let store: &MemoryStore = store;
    let transaction = if store.conn.is_autocommit() {
        Some(rusqlite::Transaction::new_unchecked(
            &store.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?)
    } else {
        None
    };
    let guard = AtomicWrite::new(&store.conn)?;
    let operation = (|| -> Result<RetrievalResult> {
        let memories = store.list_memories(layer, false)?;
        let revision = store.revision()?;
        let mut eligible: Vec<_> = memories
            .iter()
            .filter(|m| m.eligible())
            .map(|m| m.id.as_str())
            .collect();
        eligible.sort_unstable();
        // Eligibility may change with time alone. Binding the eligible set prevents
        // both stale inclusion and stale omission at validity/expiration boundaries.
        let key = serde_json::to_string(&(sanitize_text(query), layer, top_k, eligible))?;
        let timestamp = Utc::now().timestamp_millis() as f64 / 1000.0;
        let cached: Option<String> = store.conn.query_row("SELECT result_json FROM rust_query_cache WHERE key=?1 AND revision=?2 AND cached_at>=?3", params![key, revision, timestamp - 300.0], |r| r.get(0)).optional()?;
        let result = if let Some(cached) = cached {
            serde_json::from_str(&cached)?
        } else {
            let tree = crate::tree::build_tree(&memories);
            keyword_retrieve(&sanitize_text(query), &tree, &memories, top_k)
        };
        for id in &result.memory_ids {
            store.record_access(id)?;
        }
        store.save_provenance(query, &result, memories.len())?;
        // Access writes may bump the revision, but the writer lock proves that no
        // content changed. Tagging that revision here is therefore snapshot safe.
        let after_access = store.revision()?;
        store.conn.execute("INSERT INTO rust_query_cache(key,result_json,revision,cached_at) VALUES(?1,?2,?3,?4) ON CONFLICT(key) DO UPDATE SET result_json=excluded.result_json,revision=excluded.revision,cached_at=excluded.cached_at", params![key, serde_json::to_string(&result)?, after_access, timestamp])?;
        store.conn.execute(
            "DELETE FROM rust_query_cache WHERE revision<>?1 OR cached_at<?2",
            params![after_access, timestamp - 300.0],
        )?;
        store.conn.execute("DELETE FROM rust_query_cache WHERE key NOT IN (SELECT key FROM rust_query_cache ORDER BY cached_at DESC,key LIMIT 256)", [])?;
        Ok(result)
    })();
    let result = operation?;
    guard.commit()?;
    if let Some(transaction) = transaction {
        transaction.commit()?;
    }
    Ok(result)
}
