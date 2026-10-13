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
    // Memories that use generic words in passing used to answer every vague question.
    add(&store, "project", "Never use npm in this project");
    add(
        &store,
        "project",
        "Project information lives in docs/overview.md",
    );
    for vague in [
        "current project context",
        "how do we do things in this project",
        "summarize the project",
        "ภาพรวมของโปรเจกต์นี้",
        "项目概况",
    ] {
        let found = retriever::retrieve(&mut store, vague, None, 10).unwrap();
        assert!(found.facts.is_empty(), "{vague}: {:?}", found.facts);
    }
}

/// Generic words still help ranking beside a topic word, and identifiers that contain one are
/// never generic.
#[test]
fn generic_words_still_rank_beside_topic_words() {
    let mut store = MemoryStore::open(":memory:").unwrap();
    let docs = add(
        &store,
        "project",
        "Project information lives in docs/overview.md",
    );
    add(
        &store,
        "project",
        "Information about releases lives in the wiki",
    );
    add(
        &store,
        "project",
        "Set GOOGLE_CLOUD_PROJECT before running the emulator",
    );
    let found =
        retriever::retrieve(&mut store, "where does project information live", None, 5).unwrap();
    assert_eq!(found.memory_ids.first(), Some(&docs), "{:?}", found.facts);
    assert!(
        found.why[0]
            .matched
            .iter()
            .any(|m| m == "project (generic)"),
        "{:?}",
        found.why[0]
    );
    assert_eq!(
        top(&mut store, "GOOGLE_CLOUD_PROJECT").as_deref(),
        Some("Set GOOGLE_CLOUD_PROJECT before running the emulator")
    );
    let only = retriever::retrieve(&mut store, "project information", None, 5).unwrap();
    assert!(only.facts.is_empty());
    assert!(
        only.trace.contains(&"generic_only".to_string()),
        "{:?}",
        only.trace
    );
}

/// English glued to Thai or Chinese has no token of its own, and used to be missed whenever the
/// same word also appeared on its own somewhere else.
#[test]
fn english_glued_to_thai_is_found_beside_spaced_copies() {
    let mut store = MemoryStore::open(":memory:").unwrap();
    let spaced = add(&store, "project", "Sessions live in Redis for 24 hours");
    let thai = add(&store, "project", "ทีมเราใช้Redisเป็นคิวงาน");
    add(&store, "project", "ทีมเราใช้Redissonเป็นไคลเอนต์");
    let chinese = add(&store, "project", "订单服务使用PostgreSQL");
    add(&store, "project", "Integration tests need Postgres running");
    let found = retriever::retrieve(&mut store, "redis", None, 5).unwrap();
    assert!(
        found.memory_ids.contains(&spaced) && found.memory_ids.contains(&thai),
        "{:?}",
        found.facts
    );
    assert_eq!(
        found.memory_ids.len(),
        2,
        "Redisson is another word: {:?}",
        found.facts
    );
    let index = found.memory_ids.iter().position(|id| *id == thai).unwrap();
    assert!(found.why[index]
        .matched
        .iter()
        .any(|m| m == "redis (glued)"));
    let found = retriever::retrieve(&mut store, "PostgreSQL", None, 5).unwrap();
    assert!(found.memory_ids.contains(&chinese), "{:?}", found.facts);
}

/// A single Thai letter is a word only when it stands apart (ณ in หัก ณ ที่จ่าย); it used to
/// match every memory that contained it anywhere.
#[test]
fn single_thai_letters_do_not_match_inside_words() {
    let mut store = MemoryStore::open(":memory:").unwrap();
    add(&store, "project", "ภาษีหัก ณ ที่จ่ายคิดตอนสิ้นเดือน");
    add(&store, "project", "API สาธารณะต้องมี rate limit");
    add(&store, "project", "事務所の鍵は受付にある");
    let found = retriever::retrieve(&mut store, "ณ", None, 5).unwrap();
    assert_eq!(found.facts, ["ภาษีหัก ณ ที่จ่ายคิดตอนสิ้นเดือน"]);
    assert!(retriever::retrieve(&mut store, "の", None, 5)
        .unwrap()
        .facts
        .is_empty());
    assert!(
        top(&mut store, "事").is_some(),
        "one Han character is still a word"
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

#[test]
fn expired_memories_never_crowd_out_valid_ones() {
    let mut store = MemoryStore::open(":memory:").unwrap();
    let past = (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339();
    for i in 0..600 {
        store
            .insert_memory(NewMemory {
                layer: "session".into(),
                content: format!("deploy rollback note {i}"),
                expires_at: Some(past.clone()),
                ..Default::default()
            })
            .unwrap();
    }
    // Legacy naive timestamps take the same path.
    store
        .conn
        .execute(
            "INSERT INTO memories(id,layer,content,source,expires_at) VALUES('naive-1','session','deploy rollback naive','py','2001-01-01 00:00:00')",
            [],
        )
        .unwrap();
    let valid = add(
        &store,
        "project",
        "Deploy rollback uses the previous image tag",
    );
    let result = retriever::retrieve(&mut store, "deploy rollback", None, 5).unwrap();
    assert_eq!(result.memory_ids, vec![valid], "{:?}", result.trace);
}
