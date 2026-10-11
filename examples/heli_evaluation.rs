//! Replay the preserved Heli corpus through native distillation and evidence persistence.
use anyhow::Result;
use memctrl::{
    heli::{self, HeliTaskRecord},
    model::LifecycleState,
    store::MemoryStore,
};
use serde_json::json;
fn main() -> Result<()> {
    let tasks: Vec<HeliTaskRecord> =
        serde_json::from_str(include_str!("../tests/fixtures/heli-history.json"))?;
    let proposals = heli::distill(&tasks);
    let mut store = MemoryStore::open(":memory:")?;
    let ids = heli::persist_proposals(&mut store, &proposals)?;
    let accepted = store.list_memories(None, false)?.len();
    let candidates = store
        .list_memories(None, true)?
        .iter()
        .filter(|m| m.lifecycle_state == LifecycleState::Candidate)
        .count();
    println!(
        "{}",
        json!({"task_count":tasks.len(),"proposal_count":proposals.len(),"persisted_count":ids.len(),"ordinary_retrieval_fact_count":accepted,"candidate_count":candidates,"note":"Corpus replay verifies lineage; it is not a measured recall/precision comparison."})
    );
    Ok(())
}
