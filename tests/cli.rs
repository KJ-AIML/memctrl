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
        .env("MEMCTRL_SEMANTIC", "off")
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
fn invoke_in(path: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_memctrl"))
        .current_dir(path)
        .env_remove("MEMCTRL_DB_PATH")
        .env("MEMCTRL_SEMANTIC", "off")
        .env_remove("MEMCTRL_PROJECT_ROOT")
        .args(args)
        .output()
        .expect("run memctrl")
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
    // The brief shows 8-character ids; they work wherever an id does.
    assert_eq!(success(&dir, &["show", &id[..8]])["memory"]["id"], id);
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
    success(&dir, &["init"]);
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
    success(&dir, &["init"]);
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
            .env("MEMCTRL_SEMANTIC", "off")
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
            .env("MEMCTRL_SEMANTIC", "off")
            .env("MEMCTRL_LLM_API_KEY", "configured-key")
            .env("MEMCTRL_LLM_BASE_URL", "http://invalid.example");
        command
    };
    assert!(make().arg("init").output().unwrap().status.success());
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
        .env("MEMCTRL_SEMANTIC", "off")
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
    assert!(
        !dir.path().join(".claude/settings.json").exists(),
        "project hooks go to the uncommitted settings.local.json"
    );
    let settings: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap(),
    )
    .unwrap();
    let command = settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(command.ends_with("context --format hook"), "{command}");
    let recall = settings["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(recall.ends_with("recall --format hook"), "{recall}");
    assert!(
        failure(&dir, &["install", "--tool", "codex", "--project", "--hook"])
            .contains("only available for Claude Code")
    );
}

/// Runs a hook command the way Claude Code does: the event's JSON on stdin.
fn hook(dir: &TempDir, args: &[&str], input: &Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_memctrl"))
        .current_dir(dir.path())
        .env_remove("MEMCTRL_DB_PATH")
        .env("MEMCTRL_SEMANTIC", "off")
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

/// Agents don't search on their own, so the prompt hook brings what the brief left out, once
/// per session, and stays silent when nothing matches or no store exists.
#[test]
fn the_prompt_hook_recalls_what_the_brief_left_out_once_per_session() {
    let dir = TempDir::new().unwrap();
    let prompt = |text: &str| json!({"session_id": "s-1", "hook_event_name": "UserPromptSubmit", "prompt": text});
    let silent = hook(
        &dir,
        &["recall", "--format", "hook"],
        &prompt("rotate the JWT signing key"),
    );
    assert!(
        silent.status.success() && silent.stdout.is_empty(),
        "no store: nothing"
    );
    assert!(!dir.path().join(".memctrl").exists(), "and nothing created");

    success(&dir, &["init"]);
    let rotation = success(
        &dir,
        &[
            "add",
            "When rotating the JWT signing key, send the new public key to the mobile team via #platform",
            "--layer",
            "project",
        ],
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    // A one-token brief has room for nothing, so recall is the only way the memory arrives.
    let start = hook(
        &dir,
        &["context", "--format", "hook", "--budget", "1"],
        &json!({"session_id": "s-1", "hook_event_name": "SessionStart", "source": "startup"}),
    );
    assert!(start.status.success());
    assert!(dir.path().join(".memctrl/sessions/s-1").exists());

    let request = "We need to rotate the JWT signing key. Give me the exact steps.";
    let first = hook(&dir, &["recall", "--format", "hook"], &prompt(request));
    assert!(first.status.success());
    let output: Value = serde_json::from_slice(&first.stdout).expect("hook JSON");
    assert_eq!(
        output["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    let context = output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains("#platform") && context.contains(&rotation[..8]),
        "{context}"
    );

    let again = hook(&dir, &["recall", "--format", "hook"], &prompt(request));
    assert!(again.stdout.is_empty(), "already shown in this session");
    let other = json!({"session_id": "s-2", "prompt": request});
    assert!(!hook(&dir, &["recall", "--format", "hook"], &other)
        .stdout
        .is_empty());
    let chat = hook(
        &dir,
        &["recall", "--format", "hook"],
        &prompt("thanks, looks good"),
    );
    assert!(chat.stdout.is_empty());
    // A path-like session id is never used as a file name.
    let odd = json!({"session_id": "../../x", "prompt": request});
    assert!(hook(&dir, &["recall", "--format", "hook"], &odd)
        .status
        .success());
    assert!(!dir.path().join("x").exists());

    let plain = success(&dir, &["recall", request, "--format", "json"]);
    assert_eq!(plain["entries"][0]["id"], rotation.as_str());
    assert!(plain["entries"][0]["matched"].as_array().unwrap().len() >= 2);
}

#[test]
fn memory_text_cannot_forge_the_context_markers() {
    let dir = TempDir::new().unwrap();
    success(&dir, &["init"]);
    success(
        &dir,
        &[
            "add",
            "memctrl writes <!-- memctrl:context:end --> after the brief",
            "--layer",
            "project",
        ],
    );
    let mut sizes = Vec::new();
    for _ in 0..3 {
        success(&dir, &["context", "--write", "AGENTS.md"]);
        sizes.push(
            std::fs::metadata(dir.path().join("AGENTS.md"))
                .unwrap()
                .len(),
        );
    }
    assert!(sizes.windows(2).all(|w| w[0] == w[1]), "{sizes:?}");
    let text = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert_eq!(text.matches("<!-- memctrl:context:end -->").count(), 1);
}

/// Commands find the project's store and rules from any subfolder, and only `init` (or a path
/// the user names) creates a store, so a subfolder never gets an empty store of its own.
#[test]
fn subfolders_use_the_project_store_and_config_and_never_create_one() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("src/api")).unwrap();
    success(&dir, &["init"]);
    let rc = root.join(".memoryrc");
    let rules = std::fs::read_to_string(&rc)
        .unwrap()
        .replace("mode = \"flag\"", "mode = \"strict\"");
    std::fs::write(&rc, &rules).unwrap();
    success(
        &dir,
        &["add", "Releases are cut from main", "--layer", "project"],
    );
    let src = root.join("src/api");
    let in_src = |args: &[&str]| -> Value {
        let out = invoke_in(&src, args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };
    assert_eq!(
        in_src(&["query", "releases main"])["facts"][0],
        "Releases are cut from main"
    );
    // The repository's rules apply too: strict mode holds an untrusted source for review.
    assert_eq!(
        in_src(&["add", "Deploys pause on Fridays", "--source", "agent"])["status"],
        "quarantined"
    );
    in_src(&["export"]);
    assert!(root.join(".memctrl/memory.md").exists());
    assert!(!src.join(".memctrl").exists() && !root.join("src/.memctrl").exists());

    // init inside a project refuses; init at the root again keeps the rules.
    let nested = invoke_in(&src, &["init"]);
    assert!(!nested.status.success());
    assert!(String::from_utf8_lossy(&nested.stderr).contains("already a memctrl project"));
    assert_eq!(success(&dir, &["init"])["rules_written"], false);
    assert_eq!(std::fs::read_to_string(&rc).unwrap(), rules);

    // A rules file committed by a teammate: init keeps it and creates the store it names.
    let clone = TempDir::new().unwrap();
    let team_rules = "[memctrl]\ndb_path = \"data/team.db\"\n";
    std::fs::write(clone.path().join(".memoryrc"), team_rules).unwrap();
    assert_eq!(success(&clone, &["init"])["rules_written"], false);
    assert!(clone.path().join("data/team.db").exists());
    assert_eq!(
        std::fs::read_to_string(clone.path().join(".memoryrc")).unwrap(),
        team_rules
    );

    // Outside any project, commands explain and create nothing.
    let elsewhere = TempDir::new().unwrap();
    for args in [
        vec!["query", "releases"],
        vec!["add", "a note"],
        vec!["list"],
    ] {
        let message = failure(&elsewhere, &args);
        assert!(message.contains("memctrl init"), "{message}");
    }
    success(&elsewhere, &["install", "--tool", "codex", "--project"]);
    assert!(invoke(&elsewhere, &["context"]).status.success());
    assert!(!elsewhere.path().join(".memctrl").exists());

    // A path the user names may be created; doctor reports it as a stray store.
    let stray = root.join("src/.memctrl/memories.db");
    in_src(&["--db", stray.to_str().unwrap(), "add", "an old note"]);
    let doctor = success(&dir, &["doctor"]);
    let listed: Vec<String> = doctor["stray_stores"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().replace('\\', "/"))
        .collect();
    assert_eq!(listed, ["src/.memctrl/memories.db"]);
}

/// Accepting a disputed memory resolves the dispute (back to unverified, never supported), so
/// it leaves "Check before trusting".
#[test]
fn accept_resolves_a_dispute() {
    let dir = TempDir::new().unwrap();
    success(&dir, &["init"]);
    let id = success(&dir, &["add", "CI runs on Node 20", "--layer", "project"])["id"]
        .as_str()
        .unwrap()
        .to_string();
    success(&dir, &["dispute", &id]);
    let accepted = success(&dir, &["accept", &id[..8]]);
    assert_eq!(accepted["cleared"], "disputed");
    let memory = &success(&dir, &["show", &id])["memory"];
    assert_eq!(memory["verification_state"], "unverified");
    assert_eq!(memory["lifecycle_state"], "accepted");
}

/// `add` says what it redacted, so nobody assumes the stored text is what they typed.
#[test]
fn add_reports_what_it_redacted() {
    let dir = TempDir::new().unwrap();
    success(&dir, &["init"]);
    let added = success(
        &dir,
        &[
            "add",
            "Staging uses DB_PASSWORD=hunter3fake",
            "--layer",
            "project",
        ],
    );
    assert_eq!(added["redacted"], json!(["PASSWORD"]));
    let shown = success(&dir, &["show", added["id"].as_str().unwrap()]);
    assert_eq!(
        shown["memory"]["content"],
        "Staging uses DB_PASSWORD=[REDACTED_PASSWORD]"
    );
    let plain = success(&dir, &["add", "Releases are cut from main"]);
    assert!(plain.get("redacted").is_none());
}
