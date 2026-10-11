use chrono::{Duration, Utc};
use memctrl::model::{
    ClaimType, LifecycleState, Memory, NewMemory, RelationType, VerificationState,
};
use memctrl::{decay, reflection, retriever, rules::Rules, store::MemoryStore, tree};
use serde_json::json;

fn memory(store: &MemoryStore, layer: &str, content: &str, confidence: f64) -> String {
    store
        .insert_memory(NewMemory {
            layer: layer.into(),
            content: content.into(),
            source: "test-source".into(),
            confidence,
            ..Default::default()
        })
        .unwrap()
}

#[test]
fn captured_python_queries_match_native_facts_and_confidence() {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/retrieval-oracle.json")).unwrap();
    let memories: Vec<Memory> = serde_json::from_value(oracle["memories"].clone()).unwrap();
    let tree = tree::build_tree(&memories);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(include_str!("fixtures/python-v4.sql"))
        .unwrap();
    drop(conn);
    let mut store = MemoryStore::open(path).unwrap();
    for query in oracle["queries"].as_array().unwrap() {
        let query_text = query["query"].as_str().unwrap();
        for result in [
            retriever::keyword_retrieve(query_text, &tree, &memories, 5),
            retriever::retrieve(&mut store, query_text, None, 5).unwrap(),
        ] {
            assert_eq!(json!(result.facts), query["facts"], "{query_text}");
            assert_eq!(
                json!(result.confidence),
                query["confidence"],
                "{query_text}"
            );
        }
    }
}

#[test]
fn retrieval_cache_is_scoped_and_invalidates_content_lifecycle_and_time() {
    let mut store = MemoryStore::open(":memory:").unwrap();
    let project = memory(
        &store,
        "project",
        "Backend authentication uses JWT middleware",
        1.0,
    );
    let session = memory(
        &store,
        "session",
        "Backend authentication uses cookie middleware",
        1.0,
    );
    assert_eq!(
        retriever::retrieve(&mut store, "backend auth", None, 1)
            .unwrap()
            .memory_ids,
        vec![project.clone()]
    );
    assert_eq!(
        retriever::retrieve(&mut store, "backend auth", Some("session"), 5)
            .unwrap()
            .memory_ids,
        vec![session.clone()]
    );
    assert_eq!(
        retriever::retrieve(&mut store, "backend auth", None, 0)
            .unwrap()
            .facts
            .len(),
        0
    );
    store
        .update_lifecycle(&project, LifecycleState::Rejected)
        .unwrap();
    assert_eq!(
        retriever::retrieve(&mut store, "backend auth", None, 5)
            .unwrap()
            .memory_ids,
        vec![session.clone()]
    );
    let before = store.get_memory(&session, true).unwrap().unwrap();
    retriever::retrieve(&mut store, "backend auth", None, 5).unwrap();
    let after = store.get_memory(&session, true).unwrap().unwrap();
    assert_eq!(after.confidence, before.confidence);
    assert_eq!(after.access_count, before.access_count + 1);
    // Alter validity once, populate both active and inactive cache sets, then
    // change only wall-clock time. No revision mutation occurs at the boundary.
    store
        .conn
        .execute(
            "UPDATE memories SET valid_from=?1,valid_until=?2 WHERE id=?3",
            rusqlite::params![
                (Utc::now() + Duration::milliseconds(80)).to_rfc3339(),
                (Utc::now() + Duration::milliseconds(250)).to_rfc3339(),
                session
            ],
        )
        .unwrap();
    assert!(retriever::retrieve(&mut store, "backend auth", None, 5)
        .unwrap()
        .facts
        .is_empty());
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(!retriever::retrieve(&mut store, "backend auth", None, 5)
        .unwrap()
        .facts
        .is_empty());
    std::thread::sleep(std::time::Duration::from_millis(170));
    assert!(retriever::retrieve(&mut store, "backend auth", None, 5)
        .unwrap()
        .facts
        .is_empty());
}

#[test]
fn stale_cache_publication_and_failed_access_cannot_poison_retrieval() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race.db");
    let mut store = MemoryStore::open(&path).unwrap();
    let old = memory(&store, "project", "Redis authentication cache", 1.0);
    retriever::retrieve(&mut store, "authentication", None, 5).unwrap();
    let (key, payload, revision): (String, String, i64) = store
        .conn
        .query_row(
            "SELECT key,result_json,revision FROM rust_query_cache",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let writer = MemoryStore::open(&path).unwrap();
    writer
        .conn
        .execute(
            "UPDATE memories SET content='PostgreSQL database migration' WHERE id=?1",
            [&old],
        )
        .unwrap();
    // Simulate delayed publication of a previously captured result. It retains
    // its captured revision and therefore cannot hit the updated snapshot.
    writer.conn.execute("INSERT OR REPLACE INTO rust_query_cache(key,result_json,revision,cached_at) VALUES(?1,?2,?3,?4)",rusqlite::params![key,payload,revision,Utc::now().timestamp_millis() as f64/1000.0]).unwrap();
    assert!(retriever::retrieve(&mut store, "authentication", None, 5)
        .unwrap()
        .facts
        .is_empty());
    let count: i64 = store
        .conn
        .query_row("SELECT count(*) FROM provenance", [], |r| r.get(0))
        .unwrap();
    store.conn.execute_batch("CREATE TRIGGER reject_access BEFORE UPDATE OF access_count ON memories BEGIN SELECT RAISE(ABORT,'access failed'); END;").unwrap();
    assert!(retriever::retrieve(&mut store, "database", None, 5).is_err());
    assert!(store.conn.is_autocommit());
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM provenance", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        count
    );
    store
        .conn
        .execute_batch("DROP TRIGGER reject_access")
        .unwrap();
    assert!(!retriever::retrieve(&mut store, "database", None, 5)
        .unwrap()
        .facts
        .is_empty());
}

#[test]
fn cache_is_bounded_across_unique_queries() {
    let mut store = MemoryStore::open(":memory:").unwrap();
    memory(&store, "project", "We use Rust for database services", 1.0);
    for index in 0..270 {
        retriever::retrieve(&mut store, &format!("database query{index}"), None, 5).unwrap();
    }
    let count: i64 = store
        .conn
        .query_row("SELECT count(*) FROM rust_query_cache", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 256);
}

#[test]
fn deterministic_tree_updates_same_count_content_and_unicode() {
    let store = MemoryStore::open(":memory:").unwrap();
    let first = memory(&store, "project", "我们决定使用 Rust 后端，支持国际化", 1.0);
    let original = tree::build_tree(&store.list_memories(None, false).unwrap());
    store
        .conn
        .execute(
            "UPDATE memories SET content='We prefer concise Unicode explanations' WHERE id=?1",
            [first],
        )
        .unwrap();
    let incremental = tree::incremental_tree(&original, &store.list_memories(None, false).unwrap());
    assert_ne!(
        serde_json::to_value(&original).unwrap(),
        serde_json::to_value(&incremental).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&incremental).unwrap(),
        serde_json::to_value(tree::build_tree(&store.list_memories(None, false).unwrap())).unwrap()
    );
}

#[test]
fn custom_unicode_triggers_respect_file_patterns_and_conditions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memoryrc");
    std::fs::write(
        &path,
        r#"[layers]
"知识" = "持久知识"
[triggers]
'on_file "文档/*.md"' = "extract -> 知识 if contains decision"
on_custom = "consolidate session -> 知识 if contains done"
[forget]
after_days = { session = 2, "知识" = 15 }
"#,
    )
    .unwrap();
    let rules = Rules::load(&path).unwrap();
    let mut store = MemoryStore::open(":memory:").unwrap();
    let id = memory(&store, "session", "We chose Rust for Unicode services", 1.0);
    assert!(rules
        .fire_trigger("on_custom", &json!({"text":"pending"}), &mut store)
        .unwrap()
        .is_empty());
    assert_eq!(
        rules
            .fire_trigger("on_custom", &json!({"text":"done"}), &mut store)
            .unwrap(),
        vec![id]
    );
    assert!(rules
        .fire_trigger(
            "on_file",
            &json!({"path":"文档/test.txt","text":"We decided to use SQLite decision"}),
            &mut store
        )
        .unwrap()
        .is_empty());
    let ids = rules
        .fire_trigger(
            "on_file",
            &json!({"path":"文档/test.md","text":"We decided to use SQLite decision"}),
            &mut store,
        )
        .unwrap();
    // Custom rules merge with defaults, so the default Markdown rule also fires.
    assert_eq!(ids.len(), 2);
    assert!(ids
        .iter()
        .any(|id| store.get_memory(id, false).unwrap().unwrap().layer == "知识"));
    std::fs::write(&path, "not [valid = TOML").unwrap();
    assert!(Rules::load(path).is_err());
}

#[test]
fn preference_migration_and_framework_patterns_extract_without_network() {
    let rules = Rules::default();
    for text in [
        "I prefer concise responses",
        "Always keep tests deterministic",
        "Migrated from SQLite to PostgreSQL for scalability",
        "Written using Rust for reliable tools",
        "Developed with Axum for our web backend",
    ] {
        let result = rules.extract(text, "user");
        assert_eq!(result.len(), 1, "{text}");
        assert!(result[0].expires_at.is_some());
    }
    assert!(rules
        .extract("We use password=plain-text-credential for login", "project")
        .is_empty());
}

#[test]
fn scheduled_decay_and_explicit_reinforcement_preserve_origin() {
    let mut store = MemoryStore::open(":memory:").unwrap();
    let inferred = memory(
        &store,
        "session",
        "Authentication inferred from middleware",
        0.7,
    );
    let permanent = memory(&store, "project", "Authentication project fact", 0.7);
    let explicit = memory(&store, "session", "Explicit authentication decision", 1.0);
    assert_eq!(decay::run_decay(&mut store, 24.0).unwrap(), 0);
    store
        .conn
        .execute(
            "UPDATE maintenance_state SET last_run_at=?1 WHERE key='confidence_decay'",
            [(Utc::now() - Duration::hours(48)).to_rfc3339()],
        )
        .unwrap();
    assert_eq!(decay::run_decay(&mut store, 24.0).unwrap(), 1);
    let before = store.get_memory(&inferred, true).unwrap().unwrap();
    assert!((before.confidence - 0.7 * 0.95_f64.powi(2)).abs() < 0.00001);
    assert_eq!(
        store
            .get_memory(&permanent, true)
            .unwrap()
            .unwrap()
            .confidence,
        0.7
    );
    assert_eq!(
        store
            .get_memory(&explicit, true)
            .unwrap()
            .unwrap()
            .confidence,
        1.0
    );
    assert_eq!(decay::run_decay(&mut store, 24.0).unwrap(), 0);
    assert!(decay::reinforce(&mut store, &inferred).unwrap());
    let after = store.get_memory(&inferred, true).unwrap().unwrap();
    assert_eq!(before.created_at, after.created_at);
    assert!((after.confidence - before.confidence - 0.1).abs() < 1e-12);
}

#[test]
fn reflection_is_atomic_reviewable_source_preserving_and_idempotent() {
    let mut store = MemoryStore::open(":memory:").unwrap();
    let rules = Rules::default();
    let first = memory(
        &store,
        "session",
        "We implemented authentication middleware",
        1.0,
    );
    let second = memory(
        &store,
        "session",
        "We chose PostgreSQL for transactions",
        1.0,
    );
    store.conn.execute_batch("CREATE TRIGGER reject_lineage BEFORE INSERT ON memory_relations BEGIN SELECT RAISE(ABORT,'lineage failed'); END;").unwrap();
    assert!(reflection::reflect(&mut store, &rules, true).is_err());
    assert_eq!(store.list_memories(None, true).unwrap().len(), 2);
    store
        .conn
        .execute_batch("DROP TRIGGER reject_lineage")
        .unwrap();
    let ids = reflection::reflect(&mut store, &rules, true).unwrap();
    assert_eq!(ids.len(), 1);
    let candidate = store.get_memory(&ids[0], true).unwrap().unwrap();
    assert_eq!(candidate.lifecycle_state, LifecycleState::Candidate);
    assert_eq!(candidate.claim_type, ClaimType::DerivedLesson);
    assert_eq!(candidate.verification_state, VerificationState::Unverified);
    assert_eq!(store.relations(&ids[0]).unwrap().len(), 2);
    for id in [first, second] {
        let source = store.get_memory(&id, false).unwrap().unwrap();
        assert_eq!(source.layer, "session");
        assert_eq!(source.source, "test-source");
    }
    assert!(reflection::reflect(&mut store, &rules, true)
        .unwrap()
        .is_empty());
    let result = retriever::retrieve(&mut store, "authentication", Some("project"), 5).unwrap();
    assert!(result.facts.is_empty());
    let _ = RelationType::DerivedFrom;
}
