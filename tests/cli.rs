//! Native process tests cover command compatibility, lifecycle safety, and the real stdio transport.
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Output, Stdio},
    sync::mpsc::{self, Receiver},
    time::Duration,
};
use tempfile::TempDir;
fn invoke(dir: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_memctrl"))
        .current_dir(dir.path())
        .env_remove("MEMCTRL_DB_PATH")
        .env_remove("MEMCTRL_LLM_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .args(args)
        .output()
        .expect("run memctrl")
}
fn success(dir: &TempDir, args: &[&str]) -> Value {
    let out = invoke(dir, args);
    assert!(
        out.status.success(),
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout must contain only JSON")
}
fn failure(dir: &TempDir, args: &[&str]) -> String {
    let out = invoke(dir, args);
    assert!(!out.status.success(), "{:?} unexpectedly succeeded", args);
    assert!(out.stdout.is_empty(), "failures must leave stdout clean");
    String::from_utf8(out.stderr).unwrap()
}
#[test]
fn lifecycle_and_command_arguments() {
    let dir = TempDir::new().unwrap();
    success(&dir, &["init"]);
    let value = success(
        &dir,
        &[
            "add",
            "We use SQLite for local memory",
            "--layer",
            "project",
            "--lifecycle-state",
            "candidate",
        ],
    );
    let id = value["id"].as_str().unwrap();
    assert_eq!(
        success(&dir, &["candidates"])["candidates"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(success(&dir, &["query", "SQLite"])["facts"]
        .as_array()
        .unwrap()
        .is_empty());
    success(&dir, &["accept", id]);
    assert!(!success(&dir, &["query", "SQLite"])["facts"]
        .as_array()
        .unwrap()
        .is_empty());
    success(&dir, &["dispute", id]);
    assert_eq!(
        success(&dir, &["show", id])["memory"]["verification_state"],
        "disputed"
    );
    success(
        &dir,
        &["refute", id, "-r", "Evidence proves this claim false"],
    );
    assert!(failure(&dir, &["accept", id]).contains("refuted"));
    assert!(success(&dir, &["query", "SQLite"])["facts"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        success(&dir, &["show", id])["memory"]["verification_state"],
        "refuted"
    );
    success(&dir, &["history", id]);
    success(&dir, &["reject", id, "--reason", "review complete"]);
    success(&dir, &["list", "--include-history"]);
    success(&dir, &["maintenance"]);
    success(&dir, &["forget", id]);
    failure(&dir, &["show", id]);
    for command in [
        "tree",
        "doctor",
        "audit",
        "provenance",
        "heatmap",
        "timeline",
        "spans",
        "otel-stats",
        "stats",
        "reflect",
        "done",
    ] {
        success(&dir, &[command]);
    }
    let export = success(&dir, &["otel-export", "--output", "spans.json", "--otlp"]);
    assert!(export["spans"].as_u64().unwrap() > 0);
    serde_json::from_slice::<Value>(&std::fs::read(dir.path().join("spans.json")).unwrap())
        .unwrap();
}
#[test]
fn invalid_input_is_nonzero_and_sanitized() {
    let dir = TempDir::new().unwrap();
    for args in [
        vec!["add", "fact", "--confidence", "NaN"],
        vec!["add", "fact", "--confidence", "2"],
        vec!["add", "fact", "--layer", "password=hunter2"],
        vec!["trigger", "on_commit", "--context", "[]"],
        vec!["trigger", "on_commit", "--context", "not JSON"],
        vec!["clear"],
        vec!["agent", "put", "password=hunter2", "{}"],
        vec!["agent", "node", "{\"memory_context\":[]}", "--persist"],
    ] {
        let message = failure(&dir, &args);
        assert!(!message.contains("hunter2"));
        assert!(!message.to_ascii_lowercase().contains("panicked"));
    }
    let parsed=success(&dir,&["add","{\"password\":\"plain-secret\",\"api_key\":\"ordinary-secret\",\"fact\":\"safe fact\"}"]);
    let shown = success(&dir, &["show", parsed["id"].as_str().unwrap()]).to_string();
    assert!(!shown.contains("plain-secret"));
    assert!(!shown.contains("ordinary-secret"));
    std::fs::write(dir.path().join(".memoryrc"), "[broken").unwrap();
    failure(&dir, &["list"]);
    success(&dir, &["init", "--force"]);
    let out = invoke(&dir, &["--version"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains(env!("CARGO_PKG_VERSION")));
    let help = invoke(&dir, &["--help"]);
    assert!(help.status.success());
    let text = String::from_utf8_lossy(&help.stdout);
    for name in [
        "query",
        "candidates",
        "agent",
        "review",
        "serve",
        "--db",
        "--rc",
    ] {
        assert!(text.contains(name));
    }
}
#[test]
fn checkpoints_and_agent_node_are_json_native() {
    let dir = TempDir::new().unwrap();
    success(
        &dir,
        &[
            "agent",
            "put",
            "thread-one",
            "{\"messages\":[\"hello\"],\"password\":\"state-secret\"}",
        ],
    );
    let state = success(&dir, &["agent", "get", "thread-one"]);
    assert_eq!(state["state"]["messages"][0], "hello");
    assert!(!state.to_string().contains("state-secret"));
    assert_eq!(
        success(&dir, &["agent", "list"])["checkpoints"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    success(
        &dir,
        &[
            "agent",
            "node",
            "{\"messages\":[{\"role\":\"user\",\"content\":\"I prefer concise explanations\"}]}",
            "--persist",
        ],
    );
    assert!(!success(&dir, &["list"])["memories"]
        .as_array()
        .unwrap()
        .is_empty());
}
#[test]
fn environment_database_and_rc_database_resolution() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("custom.rc"),
        "[memctrl]\ndb_path = 'nested/config.db'\n",
    )
    .unwrap();
    success(
        &dir,
        &[
            "--rc",
            "custom.rc",
            "add",
            "We use configured local storage",
        ],
    );
    assert!(dir.path().join("nested/config.db").exists());
    let out = Command::new(env!("CARGO_BIN_EXE_memctrl"))
        .current_dir(dir.path())
        .env("MEMCTRL_DB_PATH", "env.db")
        .args(["--rc", "custom.rc", "add", "Environment configured storage"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(dir.path().join("env.db").exists());
    success(
        &dir,
        &["--db", "explicit.db", "add", "Explicit local storage"],
    );
    assert!(dir.path().join("explicit.db").exists());
}
struct StdioServer {
    child: Child,
    input: ChildStdin,
    lines: Receiver<String>,
}
impl Drop for StdioServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl StdioServer {
    fn start(dir: &TempDir) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_memctrl"))
            .current_dir(dir.path())
            .env_remove("MEMCTRL_DB_PATH")
            .args(["--db", "mcp.db", "serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            child,
            input,
            lines,
        }
    }
    fn send(&mut self, value: Value) {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
    }
    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        // Timeout plus Drop guarantees a stalled server cannot hang the test process.
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(10))
                .expect("MCP response timed out");
            let value: Value =
                serde_json::from_str(&line).expect("MCP stdout must contain JSON-RPC only");
            if value["id"] == id {
                return value;
            }
        }
    }
    fn call(&mut self, id: u64, name: &str, args: Value) -> Value {
        self.request(id, "tools/call", json!({"name":name,"arguments":args}))
    }
}
fn tool_json(response: &Value) -> Value {
    serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}
#[test]
fn sdk_stdio_initialize_tools_validation_and_rule_reload() {
    let dir = TempDir::new().unwrap();
    let mut server = StdioServer::start(&dir);
    let initialized=server.request(1,"initialize",json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"integration-test","version":"1"}}));
    assert_eq!(initialized["result"]["serverInfo"]["name"], "memctrl");
    assert!(initialized["result"]["capabilities"]["tools"].is_object());
    server.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let listed = server.request(2, "tools/list", json!({}));
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 9);
    let added = server.call(
        3,
        "memctrl_add",
        json!({"content":"We use SQLite for durable local data","layer":"session"}),
    );
    let id = tool_json(&added)["id"].as_str().unwrap().to_string();
    for (n, name, args) in [
        (
            4,
            "memctrl_add",
            json!({"content":"new fact","layer":"session","source":null}),
        ),
        (
            5,
            "memctrl_review",
            json!({"id":id,"action":"reject","reason":24}),
        ),
        (
            6,
            "memctrl_trigger",
            json!({"event":"on_commit","context":[]}),
        ),
    ] {
        let result = server.call(n, name, args);
        assert_eq!(result["result"]["isError"], true);
    }
    let store = memctrl::store::MemoryStore::open(dir.path().join("mcp.db")).unwrap();
    assert_eq!(store.list_memories(None, true).unwrap().len(), 1);
    assert_eq!(
        store
            .get_memory(&id, true)
            .unwrap()
            .unwrap()
            .lifecycle_state,
        memctrl::model::LifecycleState::Accepted
    );
    let query = server.call(7, "memctrl_query", json!({"query":"SQLite"}));
    assert!(!tool_json(&query)["facts"].as_array().unwrap().is_empty());
    std::fs::write(
        dir.path().join(".memoryrc"),
        "[triggers]\non_commit = 'consolidate session -> user'\n",
    )
    .unwrap();
    let trigger = server.call(
        8,
        "memctrl_trigger",
        json!({"event":"on_commit","context":{}}),
    );
    assert_ne!(trigger["result"]["isError"], true);
    assert_eq!(store.get_memory(&id, true).unwrap().unwrap().layer, "user");
    std::fs::write(dir.path().join(".memoryrc"), "[broken").unwrap();
    assert_eq!(
        server.call(
            9,
            "memctrl_add",
            json!({"content":"invalid rules must block this","layer":"session"})
        )["result"]["isError"],
        true
    );
    assert_eq!(store.list_memories(None, true).unwrap().len(), 1);
}

#[test]
fn llm_is_opt_in_and_installer_respects_scope() {
    let dir = TempDir::new().unwrap();
    let make = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_memctrl"));
        command
            .current_dir(dir.path())
            .env_remove("MEMCTRL_DB_PATH")
            .env("MEMCTRL_LLM_API_KEY", "configured-key")
            .env("MEMCTRL_LLM_BASE_URL", "http://invalid.example");
        command
    };
    for args in [
        vec!["add", "We use deterministic retrieval by default"],
        vec!["query", "retrieval"],
        vec!["tree"],
    ] {
        let output = make().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap();
    }
    let output = make()
        .args(["add", "We use explicit LLM extraction", "--llm"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("HTTPS"));
    let home = dir.path().join("temporary-home");
    let output = Command::new(env!("CARGO_BIN_EXE_memctrl"))
        .current_dir(dir.path())
        .env_remove("MEMCTRL_DB_PATH")
        .env("HOME", &home)
        .args(["install", "--tool", "codex"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(home.join(".agents/skills/memctrl/SKILL.md").exists());
    assert!(!dir.path().join(".agents/skills/memctrl/SKILL.md").exists());
    success(&dir, &["install", "--tool", "codex", "--project"]);
    assert!(dir.path().join(".agents/skills/memctrl/SKILL.md").exists());
}

/// The session-start brief: silent where memctrl was never set up, Markdown by default, hook JSON
/// for Claude Code, and an idempotent marked block when written into AGENTS.md.
#[test]
fn context_brief_is_silent_without_a_store_and_renders_every_format() {
    let dir = TempDir::new().unwrap();
    let out = invoke(&dir, &["context"]);
    assert!(out.status.success());
    assert!(out.stdout.is_empty(), "no store means no brief");
    assert!(
        !dir.path().join(".memctrl").exists(),
        "a hook must not create a store"
    );
    assert_eq!(
        success(&dir, &["context", "--format", "json"])["store"],
        "missing"
    );

    success(&dir, &["init"]);
    success(
        &dir,
        &[
            "add",
            "We chose SQLx over Diesel",
            "--layer",
            "project",
            "--claim-type",
            "decision",
        ],
    );
    success(
        &dir,
        &["add", "Scratch note for today", "--layer", "session"],
    );
    let out = invoke(&dir, &["context"]);
    assert!(out.status.success());
    let markdown = String::from_utf8(out.stdout).unwrap();
    assert!(
        markdown.starts_with("# Project memory (memctrl)"),
        "{markdown}"
    );
    assert!(markdown.contains("## Decisions") && markdown.contains("We chose SQLx over Diesel"));
    assert!(
        !markdown.contains("Scratch note"),
        "session layer is opt-in"
    );

    let out = invoke(&dir, &["context", "--layer", "session", "--format", "hook"]);
    let hook: Value = serde_json::from_slice(&out.stdout).expect("hook output is JSON");
    assert_eq!(hook["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let injected = hook["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(injected.contains("Scratch note") && !injected.contains("SQLx"));

    let agents = dir.path().join("AGENTS.md");
    std::fs::write(&agents, "# Agent rules\n\nRun make ci.\n").unwrap();
    for _ in 0..2 {
        let written = success(&dir, &["context", "--write", "AGENTS.md"]);
        assert_eq!(written["shown"], 1);
    }
    let text = std::fs::read_to_string(&agents).unwrap();
    assert!(text.starts_with("# Agent rules\n\nRun make ci.\n"));
    assert_eq!(
        text.matches("<!-- memctrl:context:begin -->").count(),
        1,
        "{text}"
    );
    assert!(text.contains("We chose SQLx over Diesel"));
}

#[test]
fn claude_install_uses_the_real_skill_path_and_merges_the_hook_once() {
    let dir = TempDir::new().unwrap();
    let first = success(
        &dir,
        &["install", "--tool", "claude", "--project", "--hook"],
    );
    assert!(dir.path().join(".claude/skills/memctrl/SKILL.md").exists());
    assert!(!dir.path().join(".claude/agent").exists());
    assert_eq!(first["hook_changed"], true);
    let second = success(
        &dir,
        &["install", "--tool", "claude", "--project", "--hook"],
    );
    assert_eq!(second["hook_changed"], false);
    let settings: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    let command = settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(command.ends_with("context --format hook"), "{command}");
    assert!(
        failure(&dir, &["install", "--tool", "codex", "--project", "--hook"])
            .contains("only available for Claude Code")
    );
}
