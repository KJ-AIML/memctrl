//! Persistent confidence maintenance; access alone never increases belief confidence.
use crate::model::parse_time;
use crate::store::{AtomicWrite, MemoryStore};
use anyhow::{bail, Result};
use chrono::Utc;

pub fn run_decay(store: &mut MemoryStore, min_hours: f64) -> Result<usize> {
    if !min_hours.is_finite() || min_hours < 0.0 {
        bail!("minimum decay interval must be finite and nonnegative");
    }
    let store: &MemoryStore = store;
    let guard = AtomicWrite::new(&store.conn)?;
    let result = (|| -> Result<usize> {
        let Some(last) = store.last_maintenance("confidence_decay")? else {
            if min_hours > 0.0 {
                store.maintenance("confidence_decay", serde_json::json!({"init":true}))?;
                return Ok(0);
            }
            return apply_decay(store, 1.0);
        };
        let hours = (Utc::now() - parse_time(&last)?).num_milliseconds() as f64 / 3_600_000.0;
        if hours < min_hours {
            return Ok(0);
        }
        // Explicit maintenance (interval zero) applies one day. Scheduled runs
        // account for elapsed time instead of silently losing missed intervals.
        apply_decay(store, if min_hours == 0.0 { 1.0 } else { hours / 24.0 })
    })();
    let count = result?;
    guard.commit()?;
    Ok(count)
}

fn apply_decay(store: &MemoryStore, days: f64) -> Result<usize> {
    let mut count = 0;
    for memory in store.list_memories(None, false)? {
        let (rate, floor): (f64, f64) = match memory.layer.as_str() {
            "session" => (0.05, 0.3),
            "user" => (0.01, 0.5),
            _ => continue,
        };
        // Explicit facts never decay, and decay never raises a memory already below its floor.
        if memory.confidence >= 1.0 || memory.confidence <= floor {
            continue;
        }
        let confidence = (memory.confidence * (1.0 - rate).powf(days)).max(floor);
        if confidence != memory.confidence && store.update_confidence(&memory.id, confidence)? {
            count += 1;
        }
    }
    store.maintenance(
        "confidence_decay",
        serde_json::json!({"decayed_count":count,"days_elapsed":days}),
    )?;
    Ok(count)
}

pub fn reinforce(store: &mut MemoryStore, id: &str) -> Result<bool> {
    let store: &MemoryStore = store;
    let guard = AtomicWrite::new(&store.conn)?;
    let result = (|| -> Result<bool> {
        let Some(memory) = store.get_memory(id, true)? else {
            return Ok(false);
        };
        let updated = store.update_confidence(id, (memory.confidence + 0.1).min(1.0))?;
        if updated {
            store.record_access(id)?;
        }
        Ok(updated)
    })();
    let updated = result?;
    guard.commit()?;
    Ok(updated)
}
