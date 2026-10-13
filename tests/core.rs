use memctrl::{model::*, sanitize::*, store::MemoryStore};
use serde_json::json;
fn mem(content: &str) -> NewMemory {
    NewMemory {
        content: content.into(),
        ..Default::default()
    }
}
#[test]
fn structured_secret_values_are_redacted_recursively() {
    let input = json!({"password":"plain","nested":{"api_key":"simple","ok":"ordinary"},"items":[{"authorization":"hello"}]});
    let safe = sanitize_value(&input);
    assert!(!safe.to_string().contains("plain"));
    assert!(!safe.to_string().contains("simple"));
    assert_eq!(safe["nested"]["ok"], "ordinary");
    let text = sanitize_text(r#"{"password":"plain","api_key":"simple"}"#);
    assert!(!text.contains("plain"));
    assert!(!text.contains("simple"));
    assert!(validate_identifier("api_key=secret", "layer").is_err());
    assert!(validate_identifier("session\nextra", "layer").is_err());
    assert_eq!(sanitize_revision(&"a".repeat(40)), "a".repeat(40));
    assert_ne!(sanitize_text(&"a".repeat(40)), "a".repeat(40));
}
#[test]
fn lifecycle_and_temporal_eligibility_controls_reads() -> anyhow::Result<()> {
    let store = MemoryStore::open(":memory:")?;
    let good = store.insert_memory(mem("architecture decision"))?;
    let candidate = store.insert_memory(NewMemory {
        lifecycle_state: LifecycleState::Candidate,
        ..mem("hypothesis candidate")
    })?;
    let future = store.insert_memory(NewMemory {
        valid_from: Some("2099-01-01T00:00:00Z".into()),
        ..mem("future fact")
    })?;
    assert_eq!(store.list_memories(None, false)?.len(), 1);
    assert!(store.get_memory(&candidate, false)?.is_none());
    assert!(store.get_memory(&future, false)?.is_none());
    store.refute_memory(&good, "reason password=bad", None)?;
    assert!(store
        .update_lifecycle(&good, LifecycleState::Accepted)
        .is_err());
    assert!(store.get_memory(&good, false)?.is_none());
    assert!(store.get_memory(&good, true)?.is_some());
    assert!(store
        .insert_memory(NewMemory {
            confidence: f64::NAN,
            ..mem("invalid")
        })
        .is_err());
    Ok(())
}
#[test]
fn legacy_migration_defaults_null_enums_and_preserves_data() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("legacy.db");
    let conn = rusqlite::Connection::open(&path)?;
    conn.execute_batch("CREATE TABLE memories(id TEXT PRIMARY KEY,layer TEXT NOT NULL,content TEXT NOT NULL,source TEXT,confidence REAL,created_at TEXT,expires_at TEXT,tags TEXT,claim_type TEXT,lifecycle_state TEXT,verification_state TEXT); INSERT INTO memories VALUES ('old','project','legacy fact','manual',1,'2020-01-01T00:00:00',NULL,'[]',NULL,'',NULL);")?;
    drop(conn);
    let store = MemoryStore::open(&path)?;
    let old = store.get_memory("old", false)?.unwrap();
    assert_eq!(old.claim_type, ClaimType::Assertion);
    assert_eq!(old.lifecycle_state, LifecycleState::Accepted);
    assert_eq!(store.list_memories(None, false)?.len(), 1);
    assert_eq!(
        store
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r
                .get::<_, i64>(0))?,
        4
    );
    Ok(())
}
#[test]
fn lineage_and_tree_failures_roll_back_atomically() -> anyhow::Result<()> {
    let store = MemoryStore::open(":memory:")?;
    let a = store.insert_memory(mem("old fact"))?;
    assert!(store.supersede_memory(&a, "absent", json!({})).is_err());
    assert_eq!(
        store.get_memory(&a, true)?.unwrap().lifecycle_state,
        LifecycleState::Accepted
    );
    let b = store.insert_memory(mem("new fact"))?;
    store.supersede_memory(&a, &b, json!({"password":"plain"}))?;
    assert_eq!(store.relations(&a)?.len(), 1);
    assert!(!store.relations(&a)?[0].to_string().contains("plain"));
    let tree = TreeNode {
        id: "root".into(),
        title: "Tree".into(),
        ..Default::default()
    };
    store.rebuild_tree(&tree)?;
    let mut bad = tree.clone();
    bad.children.push(tree.clone());
    assert!(store.rebuild_tree(&bad).is_err());
    assert_eq!(store.load_tree()?.unwrap().id, "root");
    Ok(())
}
#[test]
fn direct_semantic_mutations_invalidate_cache_but_access_does_not() -> anyhow::Result<()> {
    let store = MemoryStore::open(":memory:")?;
    let id = store.insert_memory(mem("stable fact"))?;
    let revision = store.revision()?;
    store.record_access(&id)?;
    assert_eq!(store.revision()?, revision);
    store.conn.execute(
        "INSERT INTO rust_query_cache VALUES ('q','{}',?,0)",
        [revision],
    )?;
    store
        .conn
        .execute("UPDATE memories SET confidence=0.5 WHERE id=?", [&id])?;
    assert!(store.revision()? > revision);
    assert_eq!(
        store
            .conn
            .query_row("SELECT COUNT(*) FROM rust_query_cache", [], |r| r
                .get::<_, i64>(0))?,
        0
    );
    Ok(())
}
#[test]
fn semantic_redaction_preserves_diagnostic_counts_and_git_revision_fields() {
    let sha = "a".repeat(40);
    let value = sanitize_value(
        &json!({"secret_finding_count":3,"secret_findings":["memory-id"],"source_revision":sha,"source_revisions":["b".repeat(64)],"commit_sha":"c".repeat(40),"untyped": "d".repeat(40),"client_secret":"simple"}),
    );
    assert_eq!(value["secret_finding_count"], 3);
    assert_eq!(value["secret_findings"][0], "memory-id");
    assert_eq!(value["source_revision"], "a".repeat(40));
    assert_eq!(value["source_revisions"][0], "b".repeat(64));
    assert_ne!(value["untyped"], "d".repeat(40));
    assert_ne!(value["client_secret"], "simple");
}
#[test]
fn provenance_stores_memory_ids_and_reasons_not_text() -> anyhow::Result<()> {
    let store = MemoryStore::open(":memory:")?;
    let id = store.insert_memory(mem("specific architecture fact"))?;
    let result = RetrievalResult {
        memory_ids: vec![id.clone()],
        sources: vec!["manual".into()],
        facts: vec!["specific architecture fact".into()],
        trace: vec!["root -> architecture".into()],
        confidence: 1.0,
        ..Default::default()
    };
    store.save_provenance("architecture", &result, 1, "keyword_tree")?;
    let provenance = store.provenance(1)?;
    assert_eq!(provenance[0]["method"], "keyword_tree");
    let source = &provenance[0]["sources"][0];
    assert_eq!(source["match_reason"], "keyword_tree");
    assert_eq!(source["memory_id"], id);
    // The text lives in the memory (`memctrl show`); copying it into every record grew
    // databases by megabytes per query.
    assert!(source.get("content").is_none() && source.get("source").is_none());
    assert_eq!(source["layer"], "session");
    assert_eq!(source["confidence"], 1.0);
    Ok(())
}
#[test]
fn outer_transaction_rolls_back_native_store_mutations() -> anyhow::Result<()> {
    let store = MemoryStore::open(":memory:")?;
    let a = store.insert_memory(mem("a fact"))?;
    let b = store.insert_memory(mem("b fact"))?;
    let revision = store.revision()?;
    store.conn.execute_batch("BEGIN IMMEDIATE")?;
    store.supersede_memory(&a, &b, json!({}))?;
    store.conn.execute_batch("ROLLBACK")?;
    assert_eq!(
        store.get_memory(&a, true)?.unwrap().lifecycle_state,
        LifecycleState::Accepted
    );
    assert!(store.relations(&a)?.is_empty());
    assert_eq!(store.revision()?, revision);
    Ok(())
}
#[test]
fn future_schema_refuses_open_without_modifying_database() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("future.db");
    let conn = rusqlite::Connection::open(&path)?;
    conn.execute_batch("CREATE TABLE schema_version(version INTEGER PRIMARY KEY);INSERT INTO schema_version VALUES(5);CREATE TABLE marker(value TEXT);INSERT INTO marker VALUES('preserved');")?;
    let before: Vec<String> = conn
        .prepare("SELECT sql FROM sqlite_master ORDER BY name")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    assert!(MemoryStore::open(&path).is_err());
    let after: Vec<String> = conn
        .prepare("SELECT sql FROM sqlite_master ORDER BY name")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    assert_eq!(before, after);
    assert_eq!(
        conn.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))?,
        "delete"
    );
    assert_eq!(
        conn.query_row("SELECT value FROM marker", [], |r| r.get::<_, String>(0))?,
        "preserved"
    );
    Ok(())
}
#[test]
fn clear_removes_agent_checkpoint_state() -> anyhow::Result<()> {
    let store = MemoryStore::open(":memory:")?;
    store.conn.execute_batch("CREATE TABLE IF NOT EXISTS agent_checkpoints(thread_id TEXT PRIMARY KEY,state_json TEXT NOT NULL,updated_at TEXT NOT NULL);INSERT INTO agent_checkpoints VALUES('thread','{}','2020-01-01');")?;
    store.clear()?;
    assert_eq!(
        store
            .conn
            .query_row("SELECT COUNT(*) FROM agent_checkpoints", [], |r| r
                .get::<_, i64>(0))?,
        0
    );
    Ok(())
}
#[test]
fn truncated_private_keys_and_fine_grained_tokens_are_fully_redacted() {
    let pem = "prefix -----BEGIN RSA PRIVATE KEY-----\nshort\nsecret_material\n";
    let sanitized = sanitize_text(pem);
    assert!(!sanitized.contains("secret_material"));
    assert!(!sanitized.contains("short"));
    let token = "github_pat_Aaaa_Bbbb_Cccc_Dddd_Eeee_Ffff";
    assert!(has_secrets(token));
    assert!(!sanitize_text(token).contains("Aaaa"));
    assert!(!sanitize_text(r#"configuration: {"password":"plain"}"#).contains("plain"));
}
#[test]
fn consolidation_audit_failure_rolls_back_memory_and_revision() -> anyhow::Result<()> {
    let store = MemoryStore::open(":memory:")?;
    let id = store.insert_memory(mem("session work fact"))?;
    let revision = store.revision()?;
    store.conn.execute_batch("CREATE TRIGGER reject_log BEFORE INSERT ON triggers_log BEGIN SELECT RAISE(ABORT,'injected log failure'); END;")?;
    assert!(store
        .consolidate_and_log("session", "project", "on_commit", "move", None)
        .is_err());
    assert_eq!(store.get_memory(&id, true)?.unwrap().layer, "session");
    assert_eq!(store.revision()?, revision);
    assert!(store.audit(10)?.is_empty());
    Ok(())
}
#[test]
fn evidence_validates_ids_and_redacts_metadata_preserving_revisions() -> anyhow::Result<()> {
    let store = MemoryStore::open(":memory:")?;
    let id = store.insert_memory(mem("source based fact"))?;
    assert!(store
        .add_evidence(
            &id,
            "heli",
            "api_key=credential",
            None,
            RelationType::DerivedFrom,
            json!({})
        )
        .is_err());
    assert!(store
        .add_evidence(
            "absent",
            "heli",
            "task-1",
            None,
            RelationType::DerivedFrom,
            json!({})
        )
        .is_err());
    let sha = "f".repeat(40);
    store.add_evidence(
        &id,
        "heli",
        "task-1",
        Some(&sha),
        RelationType::DerivedFrom,
        json!({"password":"plaintext"}),
    )?;
    let evidence = store.evidence(&id)?;
    assert_eq!(evidence[0]["source_revision"], sha);
    assert!(!evidence[0].to_string().contains("plaintext"));
    assert_eq!(sanitize_value(&evidence[0])["source_revision"], sha);
    Ok(())
}
#[test]
fn mutation_validation_leaves_confidence_and_layer_unchanged() -> anyhow::Result<()> {
    let store = MemoryStore::open(":memory:")?;
    let id = store.insert_memory(mem("normal fact"))?;
    assert!(store.update_layer(&id, "password=plain").is_err());
    assert!(store.update_confidence(&id, f64::INFINITY).is_err());
    let m = store.get_memory(&id, true)?.unwrap();
    assert_eq!(m.layer, "session");
    assert_eq!(m.confidence, 1.0);
    assert!(store
        .insert_memory(NewMemory {
            valid_from: Some("2026-01-01T00:00:00Z".into()),
            valid_until: Some("2025-01-01T00:00:00Z".into()),
            ..mem("invalid interval")
        })
        .is_err());
    assert!(store
        .insert_memory(NewMemory {
            expires_at: Some("yesterday".into()),
            ..mem("invalid time")
        })
        .is_err());
    Ok(())
}
#[test]
fn revision_context_does_not_exempt_credential_identifiers() {
    let layer = json!({"source_revision":"a".repeat(40)}).to_string();
    assert!(validate_identifier(&layer, "layer").is_err());
}
#[test]
fn unattributed_legacy_memory_remains_unknown_origin() -> anyhow::Result<()> {
    let store = MemoryStore::open(":memory:")?;
    store.conn.execute("INSERT INTO memories(id,layer,content,source,created_at) VALUES('legacy-unknown','project','old fact',NULL,'2020-01-01T00:00:00Z')",[])?;
    assert_eq!(
        store.get_memory("legacy-unknown", true)?.unwrap().source,
        ""
    );
    Ok(())
}
