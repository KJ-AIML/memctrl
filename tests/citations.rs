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
    // The function moved down a line and changed: refresh follows it, never the old numbers.
    assert_eq!(refreshed["refreshed"][0]["from"], "3-5");
    assert_eq!(refreshed["refreshed"][0]["lines"], "4-6");
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
        // A TOML literal string: Windows paths are full of backslashes, which basic strings
        // would read as escapes.
        format!("[citations]\nroot = '{}'\n", dir.path().display()),
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

/// The secrets-file check ran on the typed path, so a symlink to config/secrets.yml was cited
/// and its line hashes exported.
#[cfg(unix)]
#[test]
fn secret_files_are_refused_through_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(root.join("config/secrets.yml"), "token: hunter2\n").unwrap();
    std::fs::write(root.join(".env"), "KEY=1\n").unwrap();
    std::os::unix::fs::symlink(root.join("config/secrets.yml"), root.join("notes.txt")).unwrap();
    std::os::unix::fs::symlink(root.join(".env"), root.join("config/app.toml")).unwrap();
    for typed in ["notes.txt:1", "config/app.toml"] {
        let spec = memctrl::citations::parse_spec(typed).unwrap();
        let error = memctrl::citations::anchor(&root, &root, &spec).unwrap_err();
        assert!(
            error.to_string().contains("secrets file"),
            "{typed}: {error}"
        );
    }
}

/// The review found `refresh` re-hashing the old line numbers after a move plus an edit, so the
/// citation covered unrelated lines and read fresh while the memory was false.
#[test]
fn refresh_follows_an_edited_moved_block_or_asks_for_cite() {
    let dir = project();
    ok(&dir, &["init"]);
    let file = dir.path().join("src/db.ts");
    std::fs::write(
        &file,
        "const pool = createPool({\n  max: 16,\n});\nexport default pool;\n",
    )
    .unwrap();
    let id = ok(
        &dir,
        &[
            "add",
            "DB pool size is 16",
            "--layer",
            "project",
            "--cite",
            "src/db.ts:1-3",
        ],
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Three lines above it, and the pool size changes: moved and edited.
    std::fs::write(
        &file,
        "// a\n// b\n// c\nconst pool = createPool({\n  max: 32,\n});\nexport default pool;\n",
    )
    .unwrap();
    let refreshed = ok(&dir, &["refresh", &id[..8]]);
    assert_eq!(refreshed["refreshed"][0]["lines"], "4-6", "{refreshed}");
    assert_eq!(refreshed["refreshed"][0]["followed"], true);
    assert_eq!(ok(&dir, &["show", &id])["citations"][0]["status"], "fresh");

    // The block's first line now appears twice: refresh can't tell which, so it changes nothing.
    std::fs::write(
        &file,
        "const pool = createPool({\n  max: 8,\n});\nconst pool = createPool({\n  max: 64,\n});\n",
    )
    .unwrap();
    let refused = err(&dir, &["refresh", &id]);
    assert!(
        refused.contains("--cite src/db.ts:<start>-<end>"),
        "{refused}"
    );
    assert_eq!(
        ok(&dir, &["show", &id])["citations"][0]["status"],
        "changed"
    );
    let chosen = ok(&dir, &["refresh", &id, "--cite", "src/db.ts:1-3"]);
    assert_eq!(chosen["refreshed"][0]["from"], "4-6");
    assert_eq!(chosen["refreshed"][0]["lines"], "1-3");
    assert_eq!(chosen["refreshed"][0]["followed"], false);
    assert_eq!(ok(&dir, &["show", &id])["citations"][0]["status"], "fresh");
    assert!(err(&dir, &["refresh", &id, "--cite", "src/other.ts:1"]).contains("doesn't cite"));
}

/// The review found that with 62 memories the brief dropped every "Check before trusting"
/// entry: ranking kept the newest decisions and the budget cut the stale ones.
#[test]
fn stale_memory_survives_a_crowded_brief() {
    let dir = project();
    ok(&dir, &["init"]);
    let id = ok(
        &dir,
        &[
            "add",
            "get_by_id filters out soft-deleted users",
            "--layer",
            "project",
            "--cite",
            "src/user.rs:3-5",
        ],
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    for n in 0..160 {
        ok(
            &dir,
            &[
                "add",
                &format!("Decision {n}: service {n} talks to the queue through the shared client library"),
                "--layer",
                "project",
                "--claim-type",
                "decision",
            ],
        );
    }
    std::fs::write(dir.path().join("src/user.rs"), "pub fn get_by_id() {}\n").unwrap();
    let brief = String::from_utf8(run(&dir, &["context"]).stdout).unwrap();
    assert!(brief.contains("## Check before trusting"), "{brief}");
    assert!(brief.contains(&id[..8]), "{brief}");
    assert!(brief.contains("Not shown for space"), "{brief}");
}
