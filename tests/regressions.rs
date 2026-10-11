//! Regression contracts from the independent review of the native runtime.
use memctrl::{mcp, model::*, rules::Rules, sanitize::*, store::MemoryStore};
use rusqlite::params;
use serde_json::json;

const DIGIT_TAIL_ID: &str = "00000000-0000-4000-8000-123456789012";

fn mem(content: &str) -> NewMemory {
    NewMemory {
        content: content.into(),
        ..Default::default()
    }
}

/// About 0.35% of v4 UUIDs end in twelve decimal digits, which the PHONE_INTL pattern used
/// to read as a phone number: the id was redacted in output and rejected as an identifier.
#[test]
fn generated_ids_with_all_digit_tails_are_identities_not_phone_numbers() {
    assert!(validate_identifier(DIGIT_TAIL_ID, "memory_id").is_ok());
    assert_eq!(sanitize_text(DIGIT_TAIL_ID), DIGIT_TAIL_ID);
    let tree_id = format!("mem_{DIGIT_TAIL_ID}");
    assert!(validate_identifier(&tree_id, "tree id").is_ok());
    assert_eq!(
        sanitize_value(&json!({"memory_id": DIGIT_TAIL_ID})),
        json!({"memory_id": DIGIT_TAIL_ID})
    );
    // Real phone numbers and secrets that carry a UUID-shaped value are still redacted.
    assert!(sanitize_text("call 5551234567 tomorrow").contains("[REDACTED_PHONE_INTL]"));
    let keyed = sanitize_text(&format!("api_key={DIGIT_TAIL_ID}"));
    assert!(!keyed.contains(DIGIT_TAIL_ID), "{keyed}");
    assert!(validate_identifier(&format!("api_key={DIGIT_TAIL_ID}"), "layer").is_err());
}

/// One such memory must stay reviewable, and must not break the tree for the whole store.
#[test]
fn memories_whose_generated_id_ends_in_digits_stay_reviewable() -> anyhow::Result<()> {
    let mut store = MemoryStore::open(":memory:")?;
    let id = store.insert_memory(NewMemory {
        lifecycle_state: LifecycleState::Candidate,
        ..mem("lesson distilled from a session")
    })?;
    store.conn.execute(
        "UPDATE memories SET id=?1 WHERE id=?2",
        params![DIGIT_TAIL_ID, id],
    )?;
    let rules = Rules::default();
    let reviewed = mcp::dispatch(
        &mut store,
        &rules,
        "memctrl_review",
        &json!({"id": DIGIT_TAIL_ID, "action": "accept"}),
    )?;
    assert!(reviewed.to_string().contains(DIGIT_TAIL_ID), "{reviewed}");
    let accepted = store
        .get_memory(DIGIT_TAIL_ID, false)?
        .expect("accepted memory");
    assert_eq!(accepted.lifecycle_state, LifecycleState::Accepted);
    let tree = mcp::dispatch(&mut store, &rules, "memctrl_tree", &json!({}))?;
    assert!(tree.to_string().contains(DIGIT_TAIL_ID), "{tree}");
    Ok(())
}

/// Decay may lower confidence toward a floor, but never raise a memory that is already below it.
#[test]
fn decay_never_raises_confidence_that_is_already_below_the_floor() -> anyhow::Result<()> {
    let mut store = MemoryStore::open(":memory:")?;
    let low = store.insert_memory(NewMemory {
        confidence: 0.2,
        ..mem("weak session hunch")
    })?;
    let near = store.insert_memory(NewMemory {
        confidence: 0.31,
        ..mem("fading session note")
    })?;
    memctrl::decay::run_decay(&mut store, 0.0)?;
    assert_eq!(store.get_memory(&low, true)?.unwrap().confidence, 0.2);
    let floored = store.get_memory(&near, true)?.unwrap().confidence;
    assert!(
        (floored - 0.3).abs() < 1e-9,
        "decays to the session floor, got {floored}"
    );
    Ok(())
}

/// Refutation is terminal: a refuted claim cannot be disputed back into acceptance.
#[test]
fn a_refuted_claim_cannot_be_disputed_back_into_retrieval() -> anyhow::Result<()> {
    let mut store = MemoryStore::open(":memory:")?;
    let rules = Rules::default();
    let id = store.insert_memory(mem("cache invalidation is never needed"))?;
    store.refute_memory(&id, "contradicted by incident 42", None)?;
    let dispute = mcp::dispatch(
        &mut store,
        &rules,
        "memctrl_review",
        &json!({"id": id, "action": "dispute"}),
    );
    assert!(dispute.is_err(), "dispute must not reopen a refuted claim");
    let accept = mcp::dispatch(
        &mut store,
        &rules,
        "memctrl_review",
        &json!({"id": id, "action": "accept"}),
    );
    assert!(accept.is_err());
    let stored = store.get_memory(&id, true)?.unwrap();
    assert_eq!(stored.verification_state, VerificationState::Refuted);
    assert!(
        memctrl::retriever::retrieve(&mut store, "cache invalidation", None, 5)?
            .facts
            .is_empty()
    );
    Ok(())
}

/// The model picks from an earlier snapshot. Publication must re-check eligibility under the
/// writer lock and record provenance with the method that actually produced the answer.
#[test]
fn llm_selection_is_rechecked_and_labelled_when_published() -> anyhow::Result<()> {
    let latest_method = |store: &MemoryStore| -> anyhow::Result<(String, String)> {
        Ok(store.conn.query_row(
            "SELECT method, sources_json FROM provenance ORDER BY rowid DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    };
    let mut store = MemoryStore::open(":memory:")?;
    let kept = store.insert_memory(mem("SQLite stores every memory locally"))?;
    let refuted = store.insert_memory(mem("Postgres stores every memory"))?;
    let selected = vec![kept.clone(), refuted.clone()];
    // Refuted while the provider request was in flight.
    store.refute_memory(&refuted, "never adopted", None)?;
    let result = memctrl::retriever::publish_selection(
        &mut store,
        "where are memories stored",
        None,
        &selected,
        5,
    )?;
    assert_eq!(result.memory_ids, vec![kept.clone()]);
    let (method, sources) = latest_method(&store)?;
    assert_eq!(method, "llm_selection");
    assert!(
        sources.contains("llm_selection") && !sources.contains("keyword_tree"),
        "{sources}"
    );
    memctrl::retriever::retrieve(&mut store, "SQLite memory", None, 5)?;
    assert_eq!(latest_method(&store)?.0, "keyword_tree");
    Ok(())
}

/// Cache keys bind the eligible set by fingerprint, so their size no longer grows with the store.
#[test]
fn cache_keys_stay_small_as_the_store_grows() -> anyhow::Result<()> {
    let mut store = MemoryStore::open(":memory:")?;
    for i in 0..2_000 {
        store.insert_memory(mem(&format!("deployment note number {i}")))?;
    }
    memctrl::retriever::retrieve(&mut store, "deployment", None, 5)?;
    let longest: i64 =
        store
            .conn
            .query_row("SELECT MAX(LENGTH(key)) FROM rust_query_cache", [], |r| {
                r.get(0)
            })?;
    assert!(longest < 300, "cache key is {longest} bytes");
    // Same query, same snapshot: still served from the cache (one entry, reused).
    memctrl::retriever::retrieve(&mut store, "deployment", None, 5)?;
    let entries: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM rust_query_cache", [], |r| r.get(0))?;
    assert_eq!(entries, 1);
    Ok(())
}

/// Extraction, recall and consolidation commit together: a failing trigger must not leave the
/// extracted memories behind, or every retry stores duplicates.
#[test]
fn agent_memory_node_rolls_back_extraction_when_consolidation_fails() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(".memoryrc");
    std::fs::write(&path, "[triggers]\non_commit = \"notify team\"\n")?;
    let rules = Rules::load(&path)?;
    let mut store = MemoryStore::open(":memory:")?;
    let state = json!({
        "messages": [{"content": "We decided to keep PostgreSQL as the primary database for orders."}],
        "memory_consolidate": true
    });
    assert!(memctrl::agent::memory_node(&mut store, state, &rules, true).is_err());
    assert!(
        store.list_memories(None, true)?.is_empty(),
        "no half-applied writes"
    );
    Ok(())
}

/// New databases get the same indexes the Python store created (audit, tree and span lookups).
#[test]
fn new_databases_have_the_python_era_indexes() -> anyhow::Result<()> {
    let store = MemoryStore::open(":memory:")?;
    for name in ["idx_tree_layer", "idx_triggers_ts", "idx_otel_spans_op"] {
        let found: i64 = store.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
            [name],
            |r| r.get(0),
        )?;
        assert_eq!(found, 1, "missing index {name}");
    }
    Ok(())
}

/// Truth table for ordinary retrieval: only accepted (or accepted + disputed), unexpired and
/// currently valid facts are eligible; history listing returns everything.
#[test]
fn eligibility_truth_table_for_ordinary_retrieval() -> anyhow::Result<()> {
    let mut store = MemoryStore::open(":memory:")?;
    let fact = |content: &str| mem(&format!("{content} widget fact"));
    let accepted = store.insert_memory(fact("accepted"))?;
    let disputed = store.insert_memory(NewMemory {
        verification_state: VerificationState::Disputed,
        ..fact("disputed")
    })?;
    let mut hidden = Vec::new();
    for state in [
        LifecycleState::Candidate,
        LifecycleState::Superseded,
        LifecycleState::Rejected,
        LifecycleState::Archived,
    ] {
        hidden.push(store.insert_memory(NewMemory {
            lifecycle_state: state,
            ..fact("lifecycle")
        })?);
    }
    hidden.push(store.insert_memory(NewMemory {
        expires_at: Some("2001-01-01T00:00:00Z".into()),
        ..fact("expired")
    })?);
    hidden.push(store.insert_memory(NewMemory {
        valid_until: Some("2001-01-01T00:00:00Z".into()),
        ..fact("no longer valid")
    })?);
    hidden.push(store.insert_memory(NewMemory {
        valid_from: Some("2999-01-01T00:00:00Z".into()),
        ..fact("not yet valid")
    })?);
    let refuted = store.insert_memory(fact("refuted"))?;
    store.refute_memory(&refuted, "disproven", None)?;
    hidden.push(refuted);

    let mut visible: Vec<String> = store
        .list_memories(None, false)?
        .into_iter()
        .map(|m| m.id)
        .collect();
    visible.sort();
    let mut expected = vec![accepted.clone(), disputed.clone()];
    expected.sort();
    assert_eq!(visible, expected);
    let found = memctrl::retriever::retrieve(&mut store, "widget fact", None, 20)?;
    assert_eq!(found.memory_ids.len(), 2, "{:?}", found.memory_ids);
    assert!(hidden.iter().all(|id| !found.memory_ids.contains(id)));
    assert_eq!(store.list_memories(None, true)?.len(), 2 + hidden.len());
    Ok(())
}

/// One redaction per secret/PII type, plus prose that must survive untouched.
#[test]
fn sanitizer_redacts_each_secret_type_and_leaves_prose_alone() {
    let cases = [
        ("key sk-abcdefghijklmnopqrstuvwxyz123456", "API_KEY"),
        (
            "token ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "GITHUB_TOKEN",
        ),
        ("aws AKIAABCDEFGHIJKLMNOP here", "AWS_KEY"),
        ("Authorization: Bearer abc123def456", "TOKEN"),
        ("access_token=xyz789", "TOKEN"),
        (r#"password = "hunter2""#, "PASSWORD"),
        ("client_secret=shh", "SECRET"),
        ("postgres://admin:pa55@db.internal/app", "CREDENTIAL"),
        ("mail ops@example.com", "EMAIL"),
        ("ssn 123-45-6789", "SSN"),
        ("call 555-123-4567", "PHONE"),
        ("call 5551234567", "PHONE_INTL"),
        (
            "-----BEGIN PRIVATE KEY-----\nMIIE\n-----END PRIVATE KEY-----",
            "PRIVATE_KEY",
        ),
    ];
    for (input, label) in cases {
        let out = sanitize_text(input);
        assert!(
            out.contains(&format!("[REDACTED_{label}]")),
            "{input:?} -> {out:?}"
        );
        assert!(has_secrets(input), "{input:?}");
    }
    for prose in [
        "We use FastAPI and PostgreSQL",
        "We discussed the API design for the new endpoint",
    ] {
        assert_eq!(sanitize_text(prose), prose);
        assert!(!has_secrets(prose), "{prose:?}");
    }
}
