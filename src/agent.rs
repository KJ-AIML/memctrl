//! Local agent state integration and independent checkpoint persistence.
use crate::{
    model::NewMemory,
    rules::Rules,
    sanitize::{sanitize_text, sanitize_value, validate_identifier},
    store::MemoryStore,
};
use anyhow::{bail, Result};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

fn ensure(store: &MemoryStore) -> Result<()> {
    store.conn.execute_batch("CREATE TABLE IF NOT EXISTS agent_checkpoints (thread_id TEXT PRIMARY KEY, state_json TEXT NOT NULL, updated_at TEXT NOT NULL)")?;
    Ok(())
}
fn exists(store: &MemoryStore) -> Result<bool> {
    Ok(store.conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='agent_checkpoints')",[],|r|r.get(0))?)
}
/// Atomic replacement by thread_id; checkpoints do not pollute memory retrieval.
pub fn put_checkpoint(store: &mut MemoryStore, thread_id: &str, state: Value) -> Result<()> {
    validate_identifier(thread_id, "thread_id")?;
    if !state.is_object() {
        bail!("checkpoint state must be an object");
    }
    let state = sanitize_value(&state).to_string();
    ensure(store)?;
    store.conn.execute("INSERT INTO agent_checkpoints (thread_id,state_json,updated_at) VALUES (?1,?2,?3) ON CONFLICT(thread_id) DO UPDATE SET state_json=excluded.state_json,updated_at=excluded.updated_at",params![thread_id,state,crate::model::now()])?;
    Ok(())
}
pub fn get_checkpoint(store: &MemoryStore, thread_id: &str) -> Result<Option<Value>> {
    validate_identifier(thread_id, "thread_id")?;
    if !exists(store)? {
        return Ok(None);
    }
    let encoded: Option<String> = store
        .conn
        .query_row(
            "SELECT state_json FROM agent_checkpoints WHERE thread_id=?1",
            [thread_id],
            |r| r.get(0),
        )
        .optional()?;
    encoded
        .map(|v| {
            serde_json::from_str::<Value>(&v)
                .map(|v| sanitize_value(&v))
                .map_err(Into::into)
        })
        .transpose()
}
pub fn list_checkpoints(store: &MemoryStore, limit: usize) -> Result<Vec<Value>> {
    if !exists(store)? {
        return Ok(vec![]);
    }
    let mut stmt=store.conn.prepare("SELECT thread_id,state_json,updated_at FROM agent_checkpoints ORDER BY updated_at DESC,thread_id LIMIT ?1")?;
    let rows = stmt
        .query_map([limit.min(i64::MAX as usize) as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(|(thread,state,updated)|Ok(sanitize_value(&json!({"thread_id":thread,"state":serde_json::from_str::<Value>(&state)?,"updated_at":updated})))).collect()
}

fn validate_state(state: &Value) -> Result<()> {
    if !state.is_object() {
        bail!("agent state must be an object");
    }
    for key in ["memory_query", "memory_event"] {
        if let Some(v) = state.get(key) {
            if !v.is_string() {
                bail!("{key} must be a string");
            }
        }
    }
    if let Some(v) = state.get("memory_event") {
        validate_identifier(v.as_str().unwrap(), "event")?;
    }
    if let Some(v) = state.get("memory_consolidate") {
        if !v.is_boolean() {
            bail!("memory_consolidate must be a boolean");
        }
    }
    if let Some(v) = state.get("memory_context") {
        if !v.is_object() {
            bail!("memory_context must be an object");
        }
    }
    if let Some(v) = state.get("messages") {
        let messages = v
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("messages must be an array"))?;
        for message in messages {
            if message.is_string() {
                continue;
            }
            if !message.is_object() {
                bail!("messages must contain strings or objects");
            }
            if let Some(content) = message.get("content") {
                if !content.is_string() {
                    bail!("message content must be a string");
                }
            }
        }
    }
    Ok(())
}
/// Extract the latest message, recall with provenance, and optionally consolidate.
///
/// All three steps commit together: if consolidation fails, the extracted memories roll back
/// instead of staying behind (a retry would otherwise store them twice).
pub fn memory_node(
    store: &mut MemoryStore,
    state: Value,
    rules: &Rules,
    auto_extract: bool,
) -> Result<Value> {
    validate_state(&state)?;
    let outer = store.conn.is_autocommit();
    let (begin, commit, rollback) = if outer {
        ("BEGIN IMMEDIATE", "COMMIT", "ROLLBACK")
    } else {
        (
            "SAVEPOINT agent_memory_node",
            "RELEASE agent_memory_node",
            "ROLLBACK TO agent_memory_node; RELEASE agent_memory_node",
        )
    };
    store.conn.execute_batch(begin)?;
    match run_memory_node(store, state, rules, auto_extract) {
        Ok(state) => {
            store.conn.execute_batch(commit)?;
            Ok(state)
        }
        Err(error) => {
            let _ = store.conn.execute_batch(rollback);
            Err(error)
        }
    }
}

fn run_memory_node(
    store: &mut MemoryStore,
    state: Value,
    rules: &Rules,
    auto_extract: bool,
) -> Result<Value> {
    let mut state = sanitize_value(&state);
    if auto_extract {
        if let Some(last) = state["messages"].as_array().and_then(|v| v.last()) {
            let content = last
                .as_str()
                .or_else(|| last["content"].as_str())
                .unwrap_or("");
            let content = sanitize_text(&content.chars().take(500).collect::<String>());
            let mut extracted = rules.extract(&content, "session");
            if extracted.is_empty() && content.chars().count() > 20 {
                extracted.push(NewMemory {
                    content,
                    source: "langgraph_conversation".into(),
                    confidence: 0.7,
                    expires_at: rules.expiry("session"),
                    ..Default::default()
                });
            }
            for mut memory in extracted {
                memory.source = "langgraph_conversation".into();
                store.insert_memory(memory)?;
            }
        }
    }
    let query = state["memory_query"].as_str().unwrap_or("");
    if !query.trim().is_empty() {
        let options = crate::retriever::SearchOptions::from_rules(rules);
        let result = crate::retriever::retrieve_with(store, query, None, 5, &options)?;
        state["memory_facts"] = json!(result.facts);
        state["memory_trace"] = json!(result.trace);
        state["memory_confidence"] = json!(result.confidence);
    } else {
        let obj = state.as_object_mut().unwrap();
        obj.entry("memory_facts").or_insert(json!([]));
        obj.entry("memory_trace").or_insert(json!([]));
        obj.entry("memory_confidence").or_insert(json!(0.0));
    }
    if state["memory_consolidate"] == true {
        let event = state["memory_event"].as_str().unwrap_or("on_commit");
        let context = state.get("memory_context").cloned().unwrap_or(json!({}));
        state["memory_consolidated"] = json!(rules.fire_trigger(event, &context, store)?);
    }
    Ok(sanitize_value(&state))
}
