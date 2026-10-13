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
        sources.contains("llm_selection") && !sources.contains("lexical_bm25"),
        "{sources}"
    );
    memctrl::retriever::retrieve(&mut store, "SQLite memory", None, 5)?;
    assert_eq!(latest_method(&store)?.0, "lexical_bm25");
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

/// Follow-up 1: numbers inside JSON *content* are user data and get redacted; numeric fields in
/// structured output (timestamps, counts) are schema values and stay numbers.
#[test]
fn numbers_in_json_content_are_redacted_but_structured_numbers_survive() -> anyhow::Result<()> {
    let safe = sanitize_text(
        r#"{"phone": 5551234567, "count": 3, "items": [5551234567], "note": "fine"}"#,
    );
    assert!(!safe.contains("5551234567"), "{safe}");
    assert!(
        safe.contains("\"count\":3") && safe.contains("fine"),
        "{safe}"
    );
    let store = MemoryStore::open(":memory:")?;
    let id = store.insert_memory(mem(
        r#"{"contact": 5551234567, "fact": "on-call rotation"}"#,
    ))?;
    let stored = store.get_memory(&id, true)?.unwrap().content;
    assert!(
        !stored.contains("5551234567") && stored.contains("on-call rotation"),
        "{stored}"
    );
    let output = json!({"timestamp": 1728646000.25, "duration_ms": 12.5, "count": 2});
    assert_eq!(sanitize_value(&output), output);
    Ok(())
}

/// Follow-up 2: a secret value is redacted whole, even when it contains `,` `;` or `}`.
#[test]
fn secret_values_with_punctuation_are_redacted_whole() {
    for input in [
        "password=p,ssw0rd;x",
        "Authorization: Bearer abc,def",
        "api_key=k3y}tail",
        "client_secret=a;b",
    ] {
        let out = sanitize_text(input);
        for fragment in ["ssw0rd", "def", "tail", ";b"] {
            assert!(
                !(input.contains(fragment) && out.contains(fragment)),
                "{input:?} -> {out:?}"
            );
        }
    }
}

/// Follow-up 4: `add --llm` and `clear --layer` write all-or-nothing. Failures are injected
/// with real SQLite triggers, not mocks.
#[test]
fn bulk_insert_and_layer_clear_are_all_or_nothing() -> anyhow::Result<()> {
    let store = MemoryStore::open(":memory:")?;
    store.conn.execute_batch(
        "CREATE TRIGGER poison_insert BEFORE INSERT ON memories WHEN NEW.content LIKE '%poison%'
         BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
    )?;
    let batch = vec![
        mem("first extracted fact"),
        mem("poison second fact"),
        mem("third extracted fact"),
    ];
    assert!(store.insert_memories(batch).is_err());
    assert!(
        store.list_memories(None, true)?.is_empty(),
        "no partial insert"
    );

    let keep = store.insert_memory(NewMemory {
        layer: "project".into(),
        ..mem("project decision")
    })?;
    for i in 0..3 {
        store.insert_memory(mem(&format!("session note {i}")))?;
    }
    let victim = store.list_memories(Some("session"), true)?[1].id.clone();
    store.conn.execute_batch(&format!(
        "CREATE TRIGGER poison_delete BEFORE DELETE ON memories WHEN OLD.id = '{victim}'
         BEGIN SELECT RAISE(ABORT, 'injected failure'); END;"
    ))?;
    assert!(store.clear_layer("session").is_err());
    assert_eq!(
        store.list_memories(Some("session"), true)?.len(),
        3,
        "no partial clear"
    );
    store.conn.execute_batch("DROP TRIGGER poison_delete")?;
    assert_eq!(store.clear_layer("session")?, 3);
    assert!(
        store.get_memory(&keep, true)?.is_some(),
        "other layers untouched"
    );
    Ok(())
}

/// Follow-up 3: read-then-write mutations take the write lock before reading, so a commit from
/// another connection can't invalidate their snapshot (SQLITE_BUSY_SNAPSHOT) mid-operation.
#[test]
fn concurrent_writers_do_not_fail_with_busy_snapshot() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("memctrl.db");
    let seed = MemoryStore::open(&path)?;
    let ids = (0..30)
        .map(|i| seed.insert_memory(mem(&format!("reviewed fact {i}"))))
        .collect::<anyhow::Result<Vec<_>>>()?;
    drop(seed);
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || -> anyhow::Result<()> {
        let store = MemoryStore::open(&writer_path)?;
        for i in 0..300 {
            store.insert_memory(mem(&format!("concurrent note {i}")))?;
        }
        Ok(())
    });
    let reviewer = std::thread::spawn(move || -> anyhow::Result<()> {
        let store = MemoryStore::open(&path)?;
        for _ in 0..6 {
            for id in &ids {
                store.update_lifecycle(id, LifecycleState::Candidate)?;
                store.update_lifecycle(id, LifecycleState::Accepted)?;
            }
        }
        Ok(())
    });
    writer.join().expect("writer thread")?;
    reviewer.join().expect("reviewer thread")?;
    Ok(())
}

/// Follow-up 3: telemetry composes with a caller's open transaction instead of failing on BEGIN.
#[test]
fn telemetry_records_inside_an_open_transaction() -> anyhow::Result<()> {
    let mut store = MemoryStore::open(":memory:")?;
    store.conn.execute_batch("BEGIN")?;
    memctrl::telemetry::record(&mut store, "store", 1.5, json!({"layer": "session"}))?;
    store.conn.execute_batch("COMMIT")?;
    let spans: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM otel_spans", [], |r| r.get(0))?;
    assert_eq!(spans, 1);
    Ok(())
}

/// Follow-up 5: databases written by the real Python store at schema v2 (released, PR #8),
/// v3 (1.3.0 development) and v4 (1.3.0) migrate with every row retained. The fixtures were
/// generated by running each commit's own `memctrl/store.py`, not written by hand.
#[test]
fn real_python_databases_migrate_with_all_rows_retained() -> anyhow::Result<()> {
    for (label, sql) in [
        ("v2", include_str!("fixtures/python-v2-full.sql")),
        ("v3", include_str!("fixtures/python-v3-full.sql")),
        ("v4", include_str!("fixtures/python-v4-full.sql")),
    ] {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("legacy.db");
        rusqlite::Connection::open(&path)?.execute_batch(sql)?;
        let mut store = MemoryStore::open(&path)?;
        let count =
            |sql: &str| -> anyhow::Result<i64> { Ok(store.conn.query_row(sql, [], |r| r.get(0))?) };

        assert_eq!(
            count("SELECT MAX(version) FROM schema_version")?,
            4,
            "{label}"
        );
        let all = store.list_memories(None, true)?;
        assert_eq!(all.len(), 4, "{label}: every memory retained");
        let decision = all
            .iter()
            .find(|m| m.content.contains("PostgreSQL"))
            .expect("project decision");
        assert_eq!(decision.layer, "project", "{label}");
        assert!(
            decision.tags.contains(&"architecture".to_string()),
            "{label}: tags kept"
        );
        assert_eq!(
            decision.lifecycle_state,
            LifecycleState::Accepted,
            "{label}"
        );
        assert_eq!(
            decision.verification_state,
            VerificationState::Unverified,
            "{label}"
        );
        assert_eq!(decision.access_count, 0, "{label}");
        // The 2001 expiry is no longer eligible; the 2999 one still is.
        assert_eq!(
            store.list_memories(None, false)?.len(),
            3,
            "{label}: expiry is eligibility"
        );

        assert_eq!(
            count("SELECT COUNT(*) FROM tree_nodes")?,
            2,
            "{label}: tree rows"
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM triggers_log")?,
            1,
            "{label}: audit rows"
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM provenance")?,
            1,
            "{label}: provenance rows"
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM otel_spans")?,
            1,
            "{label}: span rows"
        );
        let audit = store.audit(10)?;
        assert_eq!(audit[0]["event"], "on_commit", "{label}");
        assert_eq!(
            store.provenance(10)?[0]["query"],
            "what database do we use",
            "{label}"
        );
        if label == "v4" {
            assert_eq!(
                count("SELECT COUNT(*) FROM memory_relations")?,
                1,
                "relations retained"
            );
            assert_eq!(
                count("SELECT COUNT(*) FROM memory_evidence")?,
                1,
                "evidence retained"
            );
        }
        for table in ["memory_relations", "memory_evidence", "maintenance_state"] {
            let exists = count(&format!(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='{table}'"
            ))?;
            assert_eq!(exists, 1, "{label}: {table} created");
        }

        let found = memctrl::retriever::retrieve(&mut store, "PostgreSQL order storage", None, 5)?;
        assert!(
            found.facts.iter().any(|f| f.contains("PostgreSQL")),
            "{label}: {:?}",
            found.facts
        );
        let new_id = store.insert_memory(mem("Post-migration fact"))?;
        drop(store);
        let reopened = MemoryStore::open(&path)?;
        assert!(
            reopened.get_memory(&new_id, true)?.is_some(),
            "{label}: writes persist after reopen"
        );
        assert_eq!(
            reopened.list_memories(None, true)?.len(),
            5,
            "{label}: migration is idempotent"
        );
    }
    Ok(())
}

/// The brief shows 8-character ids. Every command takes a unique prefix, and an ambiguous one
/// is refused with its matches instead of acting on the wrong memory.
#[test]
fn id_prefixes_resolve_uniquely_or_report_ambiguity() -> anyhow::Result<()> {
    let mut store = MemoryStore::open(":memory:")?;
    let candidate = |content: &str| NewMemory {
        lifecycle_state: LifecycleState::Candidate,
        ..mem(content)
    };
    store.insert_memory_as(
        candidate("first twin"),
        "abcdef12-0000-4000-8000-000000000001",
        None,
    )?;
    store.insert_memory_as(
        candidate("second twin"),
        "abcdef12-0000-4000-8000-000000000002",
        None,
    )?;
    store.insert_memory_as(candidate("digits first"), DIGIT_TAIL_ID, None)?;
    let unique = store.insert_memory(candidate("only one"))?;
    assert_eq!(store.resolve_id(&unique)?, unique);
    assert_eq!(store.resolve_id(&unique[..8])?, unique);
    assert_eq!(store.resolve_id(&unique[..8].to_uppercase())?, unique);
    assert_eq!(store.resolve_id("00000000-0000")?, DIGIT_TAIL_ID);
    let ambiguous = store.resolve_id("abcdef12").unwrap_err().to_string();
    assert!(
        ambiguous.contains("ambiguous") && ambiguous.contains("abcdef12-0000"),
        "{ambiguous}"
    );
    assert!(store.resolve_id("abc").is_err(), "too short to be a prefix");
    assert!(store
        .resolve_id("ffffffff")
        .unwrap_err()
        .to_string()
        .contains("not found"));

    let rules = Rules::default();
    let revision = store.revision()?;
    let refused = mcp::dispatch(
        &mut store,
        &rules,
        "memctrl_review",
        &json!({"id": "abcdef12", "action": "accept"}),
    );
    assert!(refused.unwrap_err().to_string().contains("ambiguous"));
    assert_eq!(
        store.revision()?,
        revision,
        "an ambiguous prefix changes nothing"
    );
    let reviewed = mcp::dispatch(
        &mut store,
        &rules,
        "memctrl_review",
        &json!({"id": &unique[..8], "action": "accept"}),
    )?;
    assert_eq!(reviewed["id"], unique.as_str(), "output names the full id");
    assert_eq!(
        store.get_memory(&unique, false)?.unwrap().lifecycle_state,
        LifecycleState::Accepted
    );
    Ok(())
}

/// The review found 20 of 34 fake secret formats stored in plain text: `\b` never fires between
/// `_` and a letter, so `DB_PASSWORD=` slipped through, and common vendor formats were missing.
/// Fakes are assembled at runtime so no realistic token sits in the source.
#[test]
fn sanitizer_catches_prefixed_keys_and_vendor_tokens() {
    let fake = |prefix: &str, body: &str| format!("{prefix}{body}");
    let cases = [
        ("DB_PASSWORD=hunter3fake".to_string(), "hunter3fake", "PASSWORD"),
        ("PGPASSWORD=hunter4fake psql -h db".into(), "hunter4fake", "PASSWORD"),
        ("MYSQL_ROOT_PASSWORD=hunter5fake".into(), "hunter5fake", "PASSWORD"),
        ("STRIPE_API_KEY=fakevalue0006".into(), "fakevalue0006", "API_KEY"),
        ("JWT_SECRET=fakevalue0007".into(), "fakevalue0007", "SECRET"),
        ("login with pwd=hunter8fake".into(), "hunter8fake", "PASSWORD"),
        (r#"{"db": {"user": "app", "pass": "hunter12fake"}}"#.into(), "hunter12fake", "PASSWORD"),
        ("Authorization: Basic YWRtaW46aHVudGVyMTM=".into(), "YWRtaW46aHVudGVyMTM", "TOKEN"),
        ("postgres://admin:p@ss10fake@db.example.com/app".into(), "ss10fake", "CREDENTIAL"),
        ("redis://default:pa/ss11fake@cache.example.com:6379".into(), "ss11fake", "CREDENTIAL"),
        (fake("token gl", "pat-FAKEfake0000000000x1"), "FAKEfake0000000000x1", "GITLAB_TOKEN"),
        (fake("xo", "xb-000000000001-000000000002-FAKEfakeFAKEfake0001"), "FAKEfakeFAKEfake0001", "SLACK_TOKEN"),
        (fake("sk_", "live_FAKEfakeFAKEfake00000001"), "FAKEfakeFAKEfake00000001", "STRIPE_KEY"),
        (fake("np", "m_FAKEfakeFAKEfakeFAKEfake0000000001"), "FAKEfakeFAKEfakeFAKEfake0000000001", "NPM_TOKEN"),
        (fake("AI", "zaSyFAKE-fake_FAKE-fake_FAKE0000001"), "SyFAKE-fake_FAKE-fake_FAKE0000001", "GOOGLE_API_KEY"),
        (fake("eyJhbGciOiJIUzI1NiJ9.", "eyJzdWIiOiJ0ZXN0In0.FAKEsig0001"), "FAKEsig0001", "JWT"),
        ("-----BEGIN ENCRYPTED PRIVATE KEY-----\nMIIFAKE0002\n-----END ENCRYPTED PRIVATE KEY-----".into(), "MIIFAKE0002", "PRIVATE_KEY"),
        ("-----BEGIN PGP PRIVATE KEY BLOCK-----\nlQOFAKE0003\n-----END PGP PRIVATE KEY BLOCK-----".into(), "lQOFAKE0003", "PRIVATE_KEY"),
        ("Test card 4111 1111 1111 1111".into(), "4111 1111 1111 1111", "CARD"),
        ("On-call phone (555) 123-4567".into(), "123-4567", "PHONE"),
        ("Thai support line +66 81 234 5678".into(), "81 234 5678", "PHONE_INTL"),
    ];
    for (input, secret, label) in &cases {
        let out = sanitize_text(input);
        assert!(!out.contains(secret), "{input:?} -> {out:?}");
        assert!(
            out.contains(&format!("[REDACTED_{label}]")),
            "{input:?} -> {out:?}"
        );
        assert!(has_secrets(input), "{input:?}");
        assert!(redactions(input).contains(&label.to_string()), "{input:?}");
    }
    // The key name stays, so the memory still says what was configured.
    assert_eq!(
        sanitize_text("DB_PASSWORD=hunter3fake"),
        "DB_PASSWORD=[REDACTED_PASSWORD]"
    );
}

/// Prose and placeholders that look like secrets at a glance stay exactly as written.
#[test]
fn sanitizer_leaves_prose_and_placeholders_alone() {
    for prose in [
        "Bearer token in the header",
        "max_tokens=4096",
        "PASSWORD_MIN_LENGTH=12",
        "DB_PASSWORD=${DB_PASSWORD}",
        "Set API_KEY=<your key> in .env.example",
        "Session token: rotate weekly",
        "BYPASS=1",
        "Use https://host.example:8080/path@v2 for the mirror",
        "npm install from https://registry.npmjs.org/@babel/core",
        "Run with +1 retry and -2 jobs",
        "Version 1.2.3+build.45",
    ] {
        assert_eq!(sanitize_text(prose), prose);
        assert!(redactions(prose).is_empty(), "{prose:?}");
    }
}
