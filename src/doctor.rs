//! Health diagnostics distinguish retrieval exposure from factual verification.
use crate::{
    model::{parse_time, Memory},
    sanitize::{has_secrets, sanitize_value},
    store::MemoryStore,
    telemetry,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashSet;
const RECOGNIZED: &[&str] = &[
    "manual",
    "user",
    "mcp",
    "cli",
    "test",
    "benchmark",
    "arch_decision",
    "schema",
    "postmortem",
    "incident",
    "reflection",
    "langgraph",
    "langgraph_conversation",
];
fn summary(memory: &Memory) -> Value {
    sanitize_value(
        &json!({"id":memory.id,"layer":memory.layer,"source":memory.source,"confidence":memory.confidence,"content":memory.content.chars().take(120).collect::<String>()}),
    )
}
fn collect_ids(value: &Value, ids: &mut HashSet<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if key == "memory_id" {
                    if let Some(id) = value.as_str() {
                        ids.insert(id.to_owned());
                    }
                } else if key == "memory_ids" {
                    if let Some(values) = value.as_array() {
                        for id in values {
                            if let Some(id) = id.as_str() {
                                ids.insert(id.into());
                            }
                        }
                    }
                } else if key == "sources_json" {
                    if let Some(encoded) = value.as_str() {
                        if let Ok(v) = serde_json::from_str::<Value>(encoded) {
                            collect_ids(&v, ids);
                        }
                    }
                } else {
                    collect_ids(value, ids);
                }
            }
        }
        Value::Array(values) => {
            for v in values {
                collect_ids(v, ids);
            }
        }
        _ => {}
    }
}
pub fn report(store: &MemoryStore) -> Result<Value> {
    let memories = store.list_memories(None, true)?;
    let mut expired = Vec::new();
    let mut low = Vec::new();
    let mut risky = Vec::new();
    let mut secrets = Vec::new();
    let now = chrono::Utc::now();
    let threshold = 0.5;
    // Inspect raw content as well as decoded memories so a read sanitizer cannot hide stored secrets.
    let mut stmt = store.conn.prepare("SELECT id,content FROM memories")?;
    let secret_ids = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|(_, v)| has_secrets(v))
        .map(|(id, _)| id)
        .collect::<HashSet<_>>();
    for m in &memories {
        if m.expires_at
            .as_deref()
            .and_then(|v| parse_time(v).ok())
            .is_some_and(|t| t < now)
        {
            expired.push(summary(m));
        }
        if m.confidence < threshold {
            low.push(summary(m));
        }
        let source = m.source.to_lowercase();
        if !RECOGNIZED.contains(&source.as_str())
            && !crate::origin::AGENT_MARKERS
                .iter()
                .any(|(_, agent)| *agent == source)
        {
            risky.push(summary(m));
        }
        if secret_ids.contains(&m.id) || has_secrets(&m.content) {
            secrets.push(summary(m));
        }
    }
    let provenance = store.provenance(1000)?;
    let mut ids = HashSet::new();
    let mut low_retrievals = 0;
    for p in &provenance {
        collect_ids(p, &mut ids);
        if p.get("avg_confidence")
            .or_else(|| p.get("confidence"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            < threshold
        {
            low_retrievals += 1;
        }
    }
    let covered = memories.iter().filter(|m| ids.contains(&m.id)).count();
    let exposure = if memories.is_empty() {
        1.0
    } else {
        covered as f64 / memories.len() as f64
    };
    let spans = telemetry::spans(store, 1000)?;
    let errors = spans.iter().filter(|s| s["status"] == "error").count();
    let mut warnings = Vec::new();
    if !expired.is_empty() {
        warnings.push("expired");
    }
    if !low.is_empty() {
        warnings.push("low_confidence");
    }
    if !risky.is_empty() {
        warnings.push("risky_sources");
    }
    if !secrets.is_empty() {
        warnings.push("secret_findings");
    }
    if !memories.is_empty() && provenance.is_empty() {
        warnings.push("missing_provenance");
    } else if exposure < 0.5 {
        warnings.push("low_provenance_coverage");
    }
    if errors > 0 {
        warnings.push("otel_errors");
    }
    let integrity: String = store
        .conn
        .query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        warnings.push("database_integrity");
    }
    Ok(
        json!({"status":if warnings.is_empty(){"ok"}else{"warn"},"warnings":warnings,"memory_count":memories.len(),"expired_count":expired.len(),"low_confidence_count":low.len(),"risky_source_count":risky.len(),"secret_finding_count":secrets.len(),"low_confidence_threshold":threshold,"memory_samples":{"expired":expired.into_iter().take(5).collect::<Vec<_>>(),"low_confidence":low.into_iter().take(5).collect::<Vec<_>>(),"risky_sources":risky.into_iter().take(5).collect::<Vec<_>>(),"secret_findings":secrets.into_iter().take(5).collect::<Vec<_>>()},"provenance":{"records":provenance.len(),"covered_memories":covered,"coverage":exposure,"retrieval_exposure":exposure,"retrieval_exposure_coverage":exposure,"low_confidence_retrievals":low_retrievals},"retrieval_exposure":{"records":provenance.len(),"exposed_memories":covered,"exposure_rate":exposure,"low_confidence_retrievals":low_retrievals},"opentelemetry":{"spans":spans.len(),"error_spans":errors},"database":{"integrity":integrity},"store":store.stats()?}),
    )
}
