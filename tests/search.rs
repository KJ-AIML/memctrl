//! FTS5 retrieval contracts: multilingual matching, BM25 ranking, aliases, explanations, and an
//! index that stays correct no matter which writer touched the database.
use memctrl::model::NewMemory;
use memctrl::retriever::{self, SearchOptions};
use memctrl::rules::Rules;
use memctrl::store::MemoryStore;
use memctrl::tree;

fn add(store: &MemoryStore, layer: &str, content: &str) -> String {
    store
        .insert_memory(NewMemory {
            layer: layer.into(),
            content: content.into(),
            ..Default::default()
        })
        .unwrap()
}

fn top(store: &mut MemoryStore, query: &str) -> Option<String> {
    retriever::retrieve(store, query, None, 5)
        .unwrap()
        .facts
        .into_iter()
        .next()
}

#[test]
fn scripts_written_without_spaces_are_found() {
    let mut store = MemoryStore::open(":memory:").unwrap();
    add(&store, "project", "我们选择了PostgreSQL因为需要事务支持");
    add(
        &store,
        "project",
        "デプロイの前に必ずマイグレーションを実行する",
    );
    add(&store, "project", "ห้ามใช้ไลบรารีที่ไม่มีไลเซนส์");
    add(&store, "project", "배포 전에 데이터베이스를 백업한다");
    add(&store, "project", "Deploys go through GitHub Actions");
    for (query, expected) in [
        ("事务", "我们选择了"),       // two characters: substring
        ("事务支持", "我们选择了"),   // trigrams
        ("PostgreSQL", "我们选择了"), // Latin word glued to CJK
        ("マイグレーション", "デプロイ"),
        ("ไลเซนส์", "ห้ามใช้"),         // verbatim Thai word, tone marks attached
        ("ใช้ไลบรารีอะไรได้", "ห้ามใช้"), // natural Thai question sharing a phrase
        ("데이터베이스", "배포"),
    ] {
        let found = top(&mut store, query).unwrap_or_default();
        assert!(found.starts_with(expected), "{query} → {found:?}");
    }
}

#[test]
fn rare_terms_outrank_common_ones() {
    let mut store = MemoryStore::open(":memory:").unwrap();
    for i in 0..30 {
        add(
            &store,
            "project",
            &format!("The service config number {i} is loaded at startup"),
        );
    }
    let kafka = add(
        &store,
        "project",
        "Kafka consumers read the service events topic",
    );
    let result = retriever::retrieve(&mut store, "kafka service", None, 3).unwrap();
    assert_eq!(
        result.memory_ids.first(),
        Some(&kafka),
        "{:?}",
        result.facts
    );
    assert_eq!(result.why[0].coverage, 1.0);
}

#[test]
fn generic_queries_no_longer_return_everything() {
    let mut store = MemoryStore::open(":memory:").unwrap();
    for fact in [
        "We chose SQLx over Diesel for compile-time checked queries",
        "Integration tests need Postgres running",
        "Use pnpm, never npm",
    ] {
        add(&store, "project", fact);
    }
    let memories = store.list_memories(None, false).unwrap();
    let legacy = retriever::keyword_retrieve(
        "current project context",
        &tree::build_tree(&memories),
        &memories,
        10,
    );
    assert_eq!(
        legacy.facts.len(),
        3,
        "1.x matched the layer name and returned everything"
    );
    assert!(
        retriever::retrieve(&mut store, "current project context", None, 10)
            .unwrap()
            .facts
            .is_empty()
    );
}

#[test]
fn aliases_expand_queries_and_explain_themselves() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MemoryStore::open(":memory:").unwrap();
    let deploy = add(
        &store,
        "project",
        "Production deploys need a manual approval step",
    );
    let orm = add(&store, "project", "We chose SQLx over Diesel");
    add(
        &store,
        "project",
        "Kubernetes manifests live in deploy/k8s-manifests",
    );

    let result = retriever::retrieve(&mut store, "release to prod", None, 5).unwrap();
    assert_eq!(result.memory_ids, vec![deploy.clone()]);
    assert_eq!(result.why[0].matched, ["prod → production (alias)"]);

    let rc = dir.path().join(".memoryrc");
    std::fs::write(
        &rc,
        "[retrieval]\naliases = [[\"orm\", \"sqlx\", \"diesel\"]]\n",
    )
    .unwrap();
    let custom = SearchOptions::from_rules(&Rules::load(&rc).unwrap());
    let found = retriever::retrieve_with(&mut store, "which orm", None, 5, &custom).unwrap();
    assert_eq!(found.memory_ids, vec![orm.clone()]);

    // Alias settings are part of the cache key: the same query re-ranks when they change.
    let none =
        retriever::retrieve_with(&mut store, "which orm", None, 5, &SearchOptions::default())
            .unwrap();
    assert!(none.facts.is_empty(), "{:?}", none.facts);

    // Without built-in aliases, "prod" still finds "Production" as a prefix, and says so.
    std::fs::write(&rc, "[retrieval]\nbuiltin_aliases = false\n").unwrap();
    let plain = SearchOptions::from_rules(&Rules::load(&rc).unwrap());
    let prefix = retriever::retrieve_with(&mut store, "prod", None, 5, &plain).unwrap();
    assert_eq!(prefix.memory_ids, vec![deploy]);
    assert_eq!(prefix.why[0].matched, ["prod (substring)"]);
}

#[test]
fn invalid_alias_configuration_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let rc = dir.path().join(".memoryrc");
    for bad in [
        "[retrieval]\naliases = \"orm\"\n",
        "[retrieval]\naliases = [[\"orm\"]]\n",
        "[retrieval]\naliases = [[\"orm\", 3]]\n",
        "[retrieval]\naliases = [[\"orm\", \"password=hunter2-secret\"]]\n",
        "[retrieval]\nbuiltin_aliases = \"yes\"\n",
    ] {
        std::fs::write(&rc, bad).unwrap();
        assert!(Rules::load(&rc).is_err(), "{bad}");
    }
}

#[test]
fn prefixes_and_glued_words_fall_back_to_substrings() {
    let mut store = MemoryStore::open(":memory:").unwrap();
    let kube = add(&store, "project", "Kubernetes upgrades happen on Tuesdays");
    let result = retriever::retrieve(&mut store, "kube upgrades", None, 5).unwrap();
    assert_eq!(result.memory_ids, vec![kube]);
    assert!(result.why[0]
        .matched
        .contains(&"kube (substring)".to_string()));
}

#[test]
fn explanations_are_recorded_in_provenance() {
    let mut store = MemoryStore::open(":memory:").unwrap();
    add(&store, "project", "Integration tests need Postgres running");
    retriever::retrieve(&mut store, "integration tests", None, 5).unwrap();
    let record = &store.provenance(1).unwrap()[0];
    assert_eq!(record["method"], "lexical_bm25");
    let matched = &record["sources"][0]["why"]["matched"];
    assert_eq!(matched, &serde_json::json!(["integration", "tests"]));
}

#[test]
fn the_index_follows_every_writer() {
    let mut store = MemoryStore::open(":memory:").unwrap();
    // Raw SQL, as a 2.0 binary or the Python store would write it.
    store
        .conn
        .execute(
            "INSERT INTO memories(id,layer,content,source) VALUES('raw-1','project','Redis caches session tokens','py')",
            [],
        )
        .unwrap();
    assert_eq!(
        top(&mut store, "redis").as_deref(),
        Some("Redis caches session tokens")
    );
    store
        .conn
        .execute(
            "UPDATE memories SET content='Memcached caches session tokens' WHERE id='raw-1'",
            [],
        )
        .unwrap();
    assert!(top(&mut store, "redis").is_none());
    assert!(top(&mut store, "memcached").is_some());
    store
        .conn
        .execute("DELETE FROM memories WHERE id='raw-1'", [])
        .unwrap();
    assert!(top(&mut store, "memcached").is_none());
}

#[test]
fn databases_created_before_the_index_are_backfilled_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    {
        let store = MemoryStore::open(&path).unwrap();
        // Simulate a database last opened by memctrl 2.0: no search tables or triggers.
        store
            .conn
            .execute_batch(
                "DROP TRIGGER memory_fts_insert; DROP TRIGGER memory_fts_delete; DROP TRIGGER memory_fts_update;
                 DROP TABLE memory_fts; DROP TABLE memory_fts_tri;
                 INSERT INTO memories(id,layer,content,source) VALUES('old-1','project','Legacy facts stay searchable','v2');",
            )
            .unwrap();
    }
    let mut store = MemoryStore::open(&path).unwrap();
    assert_eq!(
        top(&mut store, "legacy searchable").as_deref(),
        Some("Legacy facts stay searchable")
    );
    // Maintenance can always re-derive the index.
    store.rebuild_search_index().unwrap();
    assert!(top(&mut store, "legacy").is_some());
}

#[test]
fn indexes_self_heal_after_writes_by_a_sqlite_without_fts5() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("healed.db");
    {
        let store = MemoryStore::open(&path).unwrap();
        add(&store, "project", "Kept fact about Redis");
        // What someone does to write with a SQLite that lacks FTS5: drop the sync triggers.
        store
            .conn
            .execute_batch(
                "DROP TRIGGER memory_fts_insert; DROP TRIGGER memory_fts_delete; DROP TRIGGER memory_fts_update;
                 INSERT INTO memories(id,layer,content,source) VALUES('ext-1','project','Written without FTS5 support','sqlite3');
                 DELETE FROM memories WHERE content='Kept fact about Redis';",
            )
            .unwrap();
    }
    let mut store = MemoryStore::open(&path).unwrap();
    assert!(
        top(&mut store, "fts5 support").is_some(),
        "missing triggers force a rebuild"
    );
    assert!(
        top(&mut store, "redis").is_none(),
        "deleted rows leave the index"
    );
    // Drift with triggers intact (rows removed behind the index's back) is caught by counts.
    store
        .conn
        .execute_batch(
            "DROP TRIGGER memory_fts_delete; DELETE FROM memories WHERE id='ext-1';
             CREATE TRIGGER memory_fts_delete AFTER DELETE ON memories BEGIN
               INSERT INTO memory_fts(memory_fts,rowid,content,tags) VALUES('delete',old.rowid,old.content,old.tags);
               INSERT INTO memory_fts_tri(memory_fts_tri,rowid,content) VALUES('delete',old.rowid,old.content);
             END;",
        )
        .unwrap();
    drop(store);
    let mut store = MemoryStore::open(&path).unwrap();
    assert!(top(&mut store, "fts5 support").is_none());
}
