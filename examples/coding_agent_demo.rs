use anyhow::Result;
use memctrl::{agent, model::NewMemory, rules::Rules, store::MemoryStore};
use serde_json::json;
fn main() -> Result<()> {
    let mut store = MemoryStore::open(":memory:")?;
    store.insert_memory(NewMemory {
        layer: "project".into(),
        content: "We use Rust and PostgreSQL".into(),
        ..Default::default()
    })?;
    let state = agent::memory_node(
        &mut store,
        json!({"memory_query":"database","messages":[{"content":"I prefer concise explanations"}]}),
        &Rules::default(),
        true,
    )?;
    agent::put_checkpoint(&mut store, "demo", state)?;
    println!(
        "{}",
        agent::get_checkpoint(&store, "demo")?.unwrap_or_default()
    );
    Ok(())
}
