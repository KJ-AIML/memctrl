//! Session reflection retains source facts and derives reviewable candidates atomically.
use crate::model::{parse_time, ClaimType, LifecycleState, NewMemory, RelationType};
use crate::rules::Rules;
use crate::sanitize::sanitize_text;
use crate::store::{AtomicWrite, MemoryStore};
use anyhow::Result;
use chrono::Utc;
use rusqlite::params;
use std::collections::BTreeSet;

pub fn reflect(store: &mut MemoryStore, rules: &Rules, force: bool) -> Result<Vec<String>> {
    let store: &MemoryStore = store;
    let guard = AtomicWrite::new(&store.conn)?;
    let result = (|| -> Result<Vec<String>> {
        let mut sessions = store.list_memories(Some("session"), false)?;
        let mut pending = Vec::new();
        for memory in sessions.drain(..) {
            let reflected: bool = store.conn.query_row("SELECT EXISTS(SELECT 1 FROM memory_relations r JOIN memories m ON m.id=r.from_memory_id WHERE r.to_memory_id=?1 AND r.relation_type='derived_from' AND m.source='reflection')", params![memory.id], |row|row.get(0))?;
            if !reflected {
                pending.push(memory);
            }
        }
        if pending.is_empty() {
            return Ok(Vec::new());
        }
        let event = if force {
            "explicit"
        } else {
            let git = std::process::Command::new("git")
                .args(["log", "-1", "--since=2 hours ago", "--format=%H"])
                .output();
            if git.is_ok_and(|output| output.status.success() && !output.stdout.is_empty()) {
                "on_commit"
            } else {
                let mut latest = None;
                for memory in store.list_memories(None, false)? {
                    for time in [
                        Some(&memory.created_at),
                        memory.updated_at.as_ref(),
                        memory.last_accessed_at.as_ref(),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        let time = parse_time(time)?;
                        if latest.is_none_or(|latest| time > latest) {
                            latest = Some(time);
                        }
                    }
                }
                for key in ["reflection", "trigger"] {
                    if let Some(last) = store.last_maintenance(key)? {
                        let time = parse_time(&last)?;
                        if latest.is_none_or(|latest| time > latest) {
                            latest = Some(time);
                        }
                    }
                }
                if latest.is_none_or(|time| (Utc::now() - time).num_seconds() < 7200) {
                    return Ok(Vec::new());
                }
                "on_session_end"
            }
        };
        let mut seen = BTreeSet::new();
        let summary = pending
            .iter()
            .filter_map(|m| {
                if seen.insert(m.content.clone()) {
                    Some(sanitize_text(&m.content))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("; ");
        let id = store.insert_memory(NewMemory {
            layer: "project".into(),
            content: crate::model::clip_to_limit(&format!(
                "Session reflection ({event}): {summary}"
            )),
            source: "reflection".into(),
            confidence: 0.7,
            claim_type: ClaimType::DerivedLesson,
            lifecycle_state: LifecycleState::Candidate,
            expires_at: rules.expiry("project"),
            tags: vec!["reflection".into(), "review-required".into()],
            ..Default::default()
        })?;
        for memory in &pending {
            store.add_relation(
                &id,
                &memory.id,
                RelationType::DerivedFrom,
                serde_json::json!({"event":event,"source":memory.source}),
            )?;
        }
        let sources: Vec<_> = pending.iter().map(|m| &m.id).collect();
        store.maintenance("reflection",serde_json::json!({"event":event,"action":"reflection_consolidate","candidate_id":id,"source_ids":sources}))?;
        // Retain the legacy trigger audit alongside richer source lineage.
        store.conn.execute("INSERT INTO triggers_log(id,event,action,memories_affected,timestamp) VALUES(?1,?2,?3,?4,?5)", params![uuid::Uuid::new_v4().to_string(),event,"reflection_consolidate",serde_json::to_string(&sources)?,crate::model::now()])?;
        Ok(vec![id])
    })();
    let ids = result?;
    guard.commit()?;
    Ok(ids)
}
