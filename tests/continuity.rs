//! Continuity: a branch's work survives /clear, compaction, a new session and a new worktree.
mod common;
#[allow(unused_imports)] // `json!` is first used by the hook tests of a later task.
use serde_json::{json, Value};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Output, Stdio},
};
use tempfile::TempDir;

fn memctrl(dir: &Path) -> std::process::Command {
    let mut command = common::memctrl();
    command
        .current_dir(dir)
        .env_remove("MEMCTRL_DB_PATH")
        .env_remove("MEMCTRL_PROJECT_ROOT")
        .env_remove("MEMCTRL_CONTINUITY")
        .env("MEMCTRL_SEMANTIC", "off")
        .env("XDG_CONFIG_HOME", dir.join("no-config"))
        .env("APPDATA", dir.join("no-config"));
    command
}
fn run(dir: &Path, args: &[&str]) -> Output {
    memctrl(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run memctrl")
}
fn ok(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[allow(dead_code)]
fn hook(dir: &Path, args: &[&str], input: &Value) -> Output {
    let mut child = memctrl(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run memctrl");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}
fn on_branch(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".git/HEAD"), format!("ref: refs/heads/{name}\n")).unwrap();
}
fn link_worktree(main: &Path, wt: &Path, branch: &str) {
    let gitdir = main.join(".git/worktrees/wt");
    std::fs::create_dir_all(&gitdir).unwrap();
    std::fs::write(gitdir.join("HEAD"), format!("ref: refs/heads/{branch}\n")).unwrap();
    std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();
    std::fs::create_dir_all(wt).unwrap();
    std::fs::write(wt.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
}
#[allow(dead_code)]
fn checkpoint_file(dir: &Path) -> PathBuf {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join(".memctrl/checkpoints"))
        .expect("checkpoints folder")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    files.remove(0)
}

#[test]
fn a_linked_worktree_reads_and_writes_the_main_store() {
    let root = TempDir::new().unwrap();
    let (main, wt) = (root.path().join("main"), root.path().join("wt"));
    std::fs::create_dir_all(&main).unwrap();
    on_branch(&main, "main");
    ok(&main, &["init"]);
    ok(
        &main,
        &[
            "add",
            "Sessions live in Redis, keyed by user id",
            "--layer",
            "project",
        ],
    );
    link_worktree(&main, &wt, "feat/x");

    assert!(ok(&wt, &["context"]).contains("Sessions live in Redis"));
    ok(
        &wt,
        &["add", "Saved from the worktree", "--layer", "project"],
    );
    assert!(ok(&main, &["context"]).contains("Saved from the worktree"));
    assert!(
        !wt.join(".memctrl").exists(),
        "the worktree gets no store of its own"
    );

    let refused = run(&wt, &["init"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("already uses the memctrl store at"));
    ok(&wt, &["init", "--force"]);
    assert!(wt.join(".memctrl/memories.db").exists());
    assert!(
        !ok(&wt, &["context"]).contains("Sessions live in Redis"),
        "now separate"
    );
}
