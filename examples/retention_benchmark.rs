//! Offline SQLite insertion/retrieval reference workload, not a cross-language speedup.
use anyhow::Result;
use memctrl::{model::NewMemory, retriever, store::MemoryStore};
use serde_json::json;
use std::time::Instant;
fn main() -> Result<()> {
    let count = std::env::args()
        .nth(1)
        .map(|s| s.parse::<usize>())
        .transpose()?
        .unwrap_or(1000);
    let mut store = MemoryStore::open(":memory:")?;
    let start = Instant::now();
    for i in 0..count {
        store.insert_memory(NewMemory {
            layer: "project".into(),
            content: format!("Rust backend service number {i} uses PostgreSQL database"),
            ..Default::default()
        })?;
    }
    let insert_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    for _ in 0..30 {
        retriever::retrieve(&mut store, "backend database", None, 5)?;
    }
    let query_ms = start.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{}",
        json!({"memory_count":count,"inserts_ms":insert_ms,"query_count":30,"queries_ms":query_ms,"queries_per_second":30000.0/query_ms})
    );
    Ok(())
}
