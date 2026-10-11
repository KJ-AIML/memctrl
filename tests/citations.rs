//! Code-anchored memories: citations are hashed when saved, checked whenever the memory is used,
//! and re-anchored only through an audited refresh.
use memctrl::{mcp, rules::Rules, store::MemoryStore};
use serde_json::{json, Value};
use std::process::{Command, Output};
use tempfile::TempDir;

fn project() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/user.rs"),
        "use db;\n\npub fn get_by_id(id: u64) -> Option<User> {\n    db::find(id).filter(|u| !u.deleted)\n}\n",
    )
    .unwrap();
    dir
}

fn run(dir: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_memctrl"))
        .current_dir(dir.path())
        .env_remove("MEMCTRL_DB_PATH")
        .env_remove("MEMCTRL_PROJECT_ROOT")
        .args(args)
        .output()
        .unwrap()
}

fn ok(dir: &TempDir, args: &[&str]) -> Value {
    let out = run(dir, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn err(dir: &TempDir, args: &[&str]) -> String {
    let out = run(dir, args);
    assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
    assert!(out.stdout.is_empty());
    String::from_utf8(out.stderr).unwrap()
}

#[test]
fn cited_memories_turn_stale_when_code_changes_and_refresh_is_audited() {
    let dir = project();
    ok(&dir, &["init"]);
    let added = ok(
        &dir,
        &[
            "add",
            "get_by_id filters out soft-deleted users",
            "--layer",
            "project",
            "--claim-type",
            "decision",
            "--cite",
            "src/user.rs:3-5",
        ],
    );
    assert_eq!(added["citations"], 1);
    let id = added["id"].as_str().unwrap().to_string();

    let found = ok(&dir, &["query", "get_by_id deleted"]);
    assert_eq!(found["why"][0]["freshness"], "fresh");

    // New imports push the function down: moved, still fresh.
    std::fs::write(
        dir.path().join("src/user.rs"),
        "use db;\nuse log;\n\npub fn get_by_id(id: u64) -> Option<User> {\n    db::find(id).filter(|u| !u.deleted)\n}\n",
    )
    .unwrap();
    assert_eq!(
        ok(&dir, &["query", "get_by_id deleted"])["why"][0]["freshness"],
        "fresh"
    );
    let shown = ok(&dir, &["show", &id]);
    assert_eq!(shown["citations"][0]["status"], "moved");
    assert_eq!(shown["citations"][0]["now_at"], 4);

    // The behavior the memory describes changes: stale everywhere it is used.
    std::fs::write(
        dir.path().join("src/user.rs"),
        "use db;\nuse log;\n\npub fn get_by_id(id: u64) -> Option<User> {\n    db::find(id)\n}\n",
    )
    .unwrap();
    assert_eq!(
        ok(&dir, &["query", "get_by_id deleted"])["why"][0]["freshness"],
        "stale"
    );
    let stale = ok(&dir, &["stale"]);
    assert_eq!(stale["stale"].as_array().unwrap().len(), 1);
    assert_eq!(stale["stale"][0]["citations"][0]["status"], "changed");
    let brief = String::from_utf8(run(&dir, &["context"]).stdout).unwrap();
    assert!(brief.contains("## Check before trusting"), "{brief}");
    assert!(brief.contains("stale: src/user.rs changed"), "{brief}");
    assert!(!brief.contains("## Decisions"), "{brief}");
    assert_eq!(ok(&dir, &["doctor"])["stale_memories"], 1);

    // A human re-checks the code and confirms the memory: refresh re-anchors and is audited.
    let refreshed = ok(&dir, &["refresh", &id]);
    assert_eq!(refreshed["refreshed"][0]["was"], "changed");
    assert!(ok(&dir, &["stale"])["stale"].as_array().unwrap().is_empty());
    let audit = ok(&dir, &["audit"]);
    assert_eq!(audit["logs"][0]["event"], "citation_refresh");

    std::fs::remove_file(dir.path().join("src/user.rs")).unwrap();
    assert!(err(&dir, &["refresh", &id]).contains("supersede or forget"));
    assert_eq!(
        ok(&dir, &["show", &id])["citations"][0]["status"],
        "missing"
    );

    ok(&dir, &["forget", &id]);
    let store = MemoryStore::open(dir.path().join(".memctrl/memories.db")).unwrap();
    let rows: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM memory_citations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0, "forget removes citations with the memory");
}

#[test]
fn invalid_citations_store_nothing() {
    let dir = project();
    ok(&dir, &["init"]);
    for spec in [
        "../outside.rs",
        "/etc/hosts",
        "src/missing.rs",
        "src/user.rs:40-41",
    ] {
        err(
            &dir,
            &[
                "add",
                "should not be stored",
                "--layer",
                "project",
                "--cite",
                spec,
            ],
        );
    }
    assert!(
        err(&dir, &["add", "x y z", "--cite", "src/user.rs", "--llm"])
            .contains("--cite applies to one memory")
    );
    assert!(ok(&dir, &["list", "--include-history"])["memories"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn cite_adds_to_existing_memories_once_and_works_from_subdirectories() {
    let dir = project();
    ok(&dir, &["init"]);
    let id = ok(
        &dir,
        &["add", "Users are soft-deleted", "--layer", "project"],
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(ok(&dir, &["cite", &id, "src/user.rs:3"])["added"], 1);
    assert_eq!(ok(&dir, &["cite", &id, "src/user.rs:3"])["added"], 0);
    let sub = Command::new(env!("CARGO_BIN_EXE_memctrl"))
        .current_dir(dir.path().join("src"))
        .env_remove("MEMCTRL_PROJECT_ROOT")
        .args(["--db", "../.memctrl/memories.db", "cite", &id, "user.rs"])
        .output()
        .unwrap();
    assert!(
        sub.status.success(),
        "{}",
        String::from_utf8_lossy(&sub.stderr)
    );
    let shown = ok(&dir, &["show", &id]);
    let paths: Vec<&str> = shown["citations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["src/user.rs", "src/user.rs"]);
    assert_eq!(shown["citations"][0]["lines"], "3");
    assert_eq!(shown["citations"][1]["lines"], Value::Null);
}

#[test]
fn configured_root_resolves_relative_to_the_rules_file() {
    let dir = project();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config/.memoryrc"),
        "[citations]\nroot = \"..\"\n",
    )
    .unwrap();
    let rules = Rules::load(dir.path().join("config/.memoryrc")).unwrap();
    let root = memctrl::citations::project_root(rules.citations_root.as_deref()).unwrap();
    assert_eq!(root, dir.path().canonicalize().unwrap());
}

#[test]
fn mcp_add_cites_atomically_and_stale_tool_reports_changes() {
    let dir = project();
    let rc = dir.path().join(".memoryrc");
    std::fs::write(
        &rc,
        format!("[citations]\nroot = \"{}\"\n", dir.path().display()),
    )
    .unwrap();
    let rules = Rules::load(&rc).unwrap();
    let mut store = MemoryStore::open(":memory:").unwrap();
    let revision = store.revision().unwrap();
    for bad in [
        json!({"content":"x","layer":"project","cite":"src/user.rs"}),
        json!({"content":"x","layer":"project","cite":["../../etc/passwd"]}),
        json!({"content":"x","layer":"project","cite":["src/user.rs:99"]}),
        json!({"content":"x","layer":"project","claim_type":"opinion"}),
        json!({"content":"x","layer":"project","tags":[1]}),
    ] {
        assert!(
            mcp::dispatch(&mut store, &rules, "memctrl_add", &bad).is_err(),
            "{bad}"
        );
    }
    assert_eq!(
        store.revision().unwrap(),
        revision,
        "rejected input must not write"
    );
    let added = mcp::dispatch(
        &mut store,
        &rules,
        "memctrl_add",
        &json!({"content":"get_by_id hides deleted users","layer":"project","claim_type":"decision","tags":["users"],"cite":["src/user.rs:3-5"]}),
    )
    .unwrap();
    assert_eq!(added["citations"], 1);
    let report = mcp::dispatch(&mut store, &rules, "memctrl_stale", &json!({})).unwrap();
    assert!(report["stale"].as_array().unwrap().is_empty());
    std::fs::write(dir.path().join("src/user.rs"), "pub fn get_by_id() {}\n").unwrap();
    let report = mcp::dispatch(&mut store, &rules, "memctrl_stale", &json!({})).unwrap();
    assert_eq!(report["stale"][0]["freshness"], "stale");
    let found = mcp::dispatch(
        &mut store,
        &rules,
        "memctrl_query",
        &json!({"query":"get_by_id"}),
    )
    .unwrap();
    assert_eq!(found["why"][0]["freshness"], "stale");
}
