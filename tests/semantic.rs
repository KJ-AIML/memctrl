//! Semantic search end to end, against a local stand-in for an embedding server: a toy model
//! whose vectors count topic words, so "which JWT library" is close to "we verify tokens with
//! jose" without sharing a word. No network beyond loopback, no paid provider.
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};
use tempfile::TempDir;

const TOPICS: &[&[&str]] = &[
    &["jwt", "jose", "token", "tokens", "signing", "auth"],
    &["release", "releases", "ship", "deploy", "rollout"],
    &["haiku", "poem", "autumn"],
];

fn toy(text: &str) -> Vec<f64> {
    let mut vector = vec![0.0; TOPICS.len() + 1];
    // A small shared component keeps every vector nonzero.
    vector[TOPICS.len()] = 0.1;
    for word in text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        if let Some(topic) = TOPICS.iter().position(|t| t.contains(&word)) {
            vector[topic] += 1.0;
        }
    }
    vector
}

fn answer(stream: &mut TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line.trim().is_empty() {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let request: Value = serde_json::from_slice(&body).unwrap_or_default();
    let data: Vec<Value> = request["input"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, text)| json!({"index": i, "embedding": toy(text.as_str().unwrap_or(""))}))
        .collect();
    let reply = json!({"object": "list", "data": data}).to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    )
}

/// A loopback embedding server; counts the requests it answered.
fn embedder() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            count.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || {
                let _ = answer(&mut stream);
            });
        }
    });
    (port, requests)
}

/// A user config folder whose `[semantic]` points at `port`.
fn config(dir: &TempDir, port: u16) -> PathBuf {
    let home = dir.path().join("config");
    std::fs::create_dir_all(home.join("memctrl")).unwrap();
    std::fs::write(
        home.join("memctrl/config.toml"),
        format!(
            "[semantic]\nmodel = \"toy\"\nbase_url = \"http://127.0.0.1:{port}/v1\"\nthreshold = 0.5\nrecall_threshold = 0.7\n"
        ),
    )
    .unwrap();
    home
}

fn command(dir: &Path, home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_memctrl"));
    command
        .current_dir(dir)
        .env_remove("MEMCTRL_DB_PATH")
        .env_remove("MEMCTRL_SEMANTIC")
        .env("XDG_CONFIG_HOME", home)
        .env("APPDATA", home);
    command
}

fn run(dir: &Path, home: &Path, args: &[&str]) -> Output {
    command(dir, home).args(args).output().unwrap()
}

fn ok(dir: &Path, home: &Path, args: &[&str]) -> Value {
    let out = run(dir, home, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout must contain only JSON")
}

fn project() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    (dir, project)
}

fn add(project: &Path, home: &Path, content: &str) -> String {
    ok(project, home, &["add", content, "--layer", "project"])["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn queries_find_memories_by_meaning_and_still_answer_vague_ones_with_nothing() {
    let (port, requests) = embedder();
    let (dir, project) = project();
    let home = config(&dir, port);
    ok(&project, &home, &["init"]);
    let jose = add(&project, &home, "We verify tokens with jose");
    add(&project, &home, "Releases go out from main every Tuesday");
    assert_eq!(
        requests.load(Ordering::SeqCst),
        0,
        "adding never calls the model"
    );

    let found = ok(&project, &home, &["query", "which JWT library"]);
    assert_eq!(found["memory_ids"][0], jose.as_str(), "{found}");
    assert!(found["why"][0]["matched"][0]
        .as_str()
        .unwrap()
        .starts_with("similar meaning"));
    assert!(found["trace"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t == "semantic"));
    let off = command(&project, &home)
        .env("MEMCTRL_SEMANTIC", "off")
        .args(["query", "which JWT library"])
        .output()
        .unwrap();
    let off: Value = serde_json::from_slice(&off.stdout).unwrap();
    assert!(
        off["memory_ids"].as_array().unwrap().is_empty(),
        "lexical alone can't"
    );
    let vague = ok(&project, &home, &["query", "write a haiku about autumn"]);
    assert!(
        vague["memory_ids"].as_array().unwrap().is_empty(),
        "{vague}"
    );

    let status = ok(&project, &home, &["semantic", "status"]);
    assert_eq!(status["enabled"], true);
    assert_eq!(status["reachable"], true);
    assert_eq!(status["dimensions"], 4);
    assert_eq!(
        status["indexed"], 2,
        "the first query embedded both memories"
    );
    assert_eq!(status["eligible"], 2);

    // A forgotten memory leaves no vector behind.
    ok(&project, &home, &["forget", &jose]);
    assert_eq!(
        ok(&project, &home, &["semantic", "status"])["stored_vectors"],
        1
    );
    assert_eq!(ok(&project, &home, &["semantic", "clear"])["cleared"], 1);
    assert_eq!(
        ok(&project, &home, &["semantic", "status"])["stored_vectors"],
        0
    );
    let index = ok(&project, &home, &["semantic", "index"]);
    assert_eq!(index["indexed"], 1);
}

#[test]
fn only_your_config_turns_it_on_and_an_unreachable_model_falls_back_to_lexical() {
    let (dir, project) = project();
    // A port with nothing listening.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let home = config(&dir, port);
    ok(&project, &home, &["init"]);
    let release = add(&project, &home, "Releases go out from main every Tuesday");
    let found = ok(&project, &home, &["query", "when do releases go out"]);
    assert_eq!(found["memory_ids"][0], release.as_str());
    assert!(
        found["trace"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t.as_str().unwrap().starts_with("semantic_unavailable")),
        "{found}"
    );
    let status = ok(&project, &home, &["semantic", "status"]);
    assert_eq!(status["reachable"], false);

    // A remote server needs your explicit allow_remote.
    std::fs::write(
        home.join("memctrl/config.toml"),
        "[semantic]\nmodel = \"toy\"\nbase_url = \"https://embeddings.example.com/v1\"\nthreshold = 0.5\n",
    )
    .unwrap();
    let refused = run(&project, &home, &["query", "releases"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("allow_remote"));

    // A repository's .memoryrc can't turn it on.
    std::fs::remove_file(home.join("memctrl/config.toml")).unwrap();
    let rc = std::fs::read_to_string(project.join(".memoryrc")).unwrap();
    std::fs::write(
        project.join(".memoryrc"),
        format!("{rc}\n[semantic]\nmodel = \"toy\"\nbase_url = \"http://127.0.0.1:{port}/v1\"\nthreshold = 0.1\n"),
    )
    .unwrap();
    let out = run(&project, &home, &["query", "releases"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("ignored [semantic]"));
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!result["trace"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t.as_str().unwrap().starts_with("semantic")));
    assert_eq!(
        ok(&project, &home, &["semantic", "status"])["enabled"],
        false
    );
}

#[test]
fn the_prompt_hook_eval_and_mcp_server_use_it() {
    let (port, _) = embedder();
    let (dir, project) = project();
    let home = config(&dir, port);
    ok(&project, &home, &["init"]);
    let jose = add(&project, &home, "We verify tokens with jose");

    let mut hook = command(&project, &home)
        .args(["recall", "--format", "hook"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let prompt = json!({"session_id": "s-1", "prompt": "Which JWT library should the new middleware use? Keep it short."});
    hook.stdin
        .take()
        .unwrap()
        .write_all(prompt.to_string().as_bytes())
        .unwrap();
    let out = hook.wait_with_output().unwrap();
    let context: Value = serde_json::from_slice(&out.stdout).expect("hook JSON");
    assert!(context["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .contains(&jose[..8]));

    let suite = dir.path().join("suite.json");
    std::fs::write(
        &suite,
        json!({"name": "toy", "memories": [
            {"key": "jose", "content": "We verify tokens with jose"},
            {"key": "release", "content": "Releases go out from main every Tuesday"}],
          "cases": [{"query": "which JWT library", "expect": ["jose"]},
                    {"query": "write a haiku about autumn", "expect": []}]})
        .to_string(),
    )
    .unwrap();
    let suite = suite.to_str().unwrap();
    for retriever in ["semantic", "recall"] {
        let report = ok(
            &project,
            &home,
            &["eval", "--file", suite, "--retriever", retriever],
        );
        assert_eq!(report["results"][retriever]["hit_at_k"], 1.0, "{report}");
        assert_eq!(report["results"][retriever]["negative_accuracy"], 1.0);
    }
    let lexical = ok(
        &project,
        &home,
        &["eval", "--file", suite, "--retriever", "lexical"],
    );
    assert_eq!(lexical["results"]["lexical"]["hit_at_k"], 0.0);

    // The MCP server runs searches inside its async runtime; the model request must work there.
    let mut server = command(&project, &home)
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = server.stdin.take().unwrap();
    let output = server.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut request = |id: u64, method: &str, params: Value| -> Value {
        writeln!(
            input,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )
        .unwrap();
        input.flush().unwrap();
        loop {
            let line = lines
                .recv_timeout(Duration::from_secs(20))
                .expect("MCP response timed out");
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == id {
                if method == "initialize" {
                    writeln!(
                        input,
                        "{}",
                        json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
                    )
                    .unwrap();
                }
                return value;
            }
        }
    };
    request(
        1,
        "initialize",
        json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}),
    );
    let response = request(
        2,
        "tools/call",
        json!({"name": "memctrl_query", "arguments": {"query": "which JWT library"}}),
    );
    let _ = server.kill();
    let _ = server.wait();
    let result: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["memory_ids"][0], jose.as_str(), "{result}");
}
