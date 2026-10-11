use memctrl::{
    heli,
    llm::LlmClient,
    model::{LifecycleState, TreeNode, VerificationState},
    store::MemoryStore,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn source_fixture(root: &Path) -> String {
    let task = root.join(".heli-harness/tasks/archive/task-101");
    fs::create_dir_all(&task).unwrap();
    let sha = "a1b2c3d4".repeat(5);
    fs::write(task.join("task.json"), json!({"title":"Hydration delay", "status":"working", "target":{"repositoryId":"auth-service"},"commitSha":sha,"password":"task-private-value"}).to_string()).unwrap();
    fs::write(
        task.join("current-task.md"),
        "Current status: complete\nTarget repo: auth-service\nRisk tier: S2\n",
    )
    .unwrap();
    fs::write(
        task.join("plan.md"),
        "## Step 1: verify cold start\nStatus: complete\nEvidence: hydration trace passed\n",
    )
    .unwrap();
    fs::write(task.join("decisions.md"), "- Verify provider hydration state before repeated authentication retries.\n- Refuted hypothesis that the network tunnel dropped WebSocket pings.\n").unwrap();
    fs::write(task.join("diagnosis.json"), json!({"active_hypothesis":"provider hydration delay", "closest_proven_boundary":"HTTP cold start", "contradicted_hypotheses":["network dropped pings"],"nested":{"api_key":"diagnosis-private-value", "password":"escaped-quote-\"-secret"}}).to_string()).unwrap();
    sha
}
fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, path: &Path, output: &mut Vec<(String, Vec<u8>)>) {
        for item in fs::read_dir(path).unwrap() {
            let path = item.unwrap().path();
            if path.is_dir() {
                walk(root, &path, output);
            } else {
                output.push((
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    fs::read(path).unwrap(),
                ));
            }
        }
    }
    let mut items = Vec::new();
    walk(root, root, &mut items);
    items.sort();
    items
}

#[test]
fn heli_nested_tasks_are_readonly_typed_and_revision_preserving() {
    let source = tempfile::tempdir().unwrap();
    let sha = source_fixture(source.path());
    let before = snapshot(source.path());
    let mut store = MemoryStore::open(":memory:").unwrap();
    let result = heli::review(&mut store, source.path(), false).unwrap();
    assert_eq!(result["tasks_loaded"], 1);
    assert_eq!(result["tasks"][0]["commit_sha"], sha);
    assert_eq!(
        result["tasks"][0]["plan_steps"][0]["evidence"],
        "hydration trace passed"
    );
    assert_eq!(result["proposals"].as_array().unwrap().len(), 3);
    assert!(store.list_memories(None, true).unwrap().is_empty());
    let serialized = result.to_string();
    for secret in [
        "task-private-value",
        "diagnosis-private-value",
        "escaped-quote",
    ] {
        assert!(!serialized.contains(secret));
    }
    assert_eq!(
        heli::review_filtered(&mut store, source.path(), false, 20, Some("other-repo")).unwrap()
            ["tasks_loaded"],
        0
    );
    let result = heli::review(&mut store, source.path(), true).unwrap();
    for id in result["created_ids"].as_array().unwrap() {
        let id = id.as_str().unwrap();
        let memory = store.get_memory(id, true).unwrap().unwrap();
        assert!(matches!(
            memory.lifecycle_state,
            LifecycleState::Candidate | LifecycleState::Rejected
        ));
        if memory.lifecycle_state == LifecycleState::Rejected {
            assert_eq!(memory.verification_state, VerificationState::Refuted);
        }
        let evidence = store.evidence(id).unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0]["source_revision"], sha);
        assert_eq!(evidence[0]["source_id"], "task-101");
    }
    assert_eq!(snapshot(source.path()), before);
}

#[test]
fn heli_evidence_failure_rolls_back_every_candidate_and_revision() {
    let source = tempfile::tempdir().unwrap();
    source_fixture(source.path());
    let mut store = MemoryStore::open(":memory:").unwrap();
    let revision = store.revision().unwrap();
    store.conn.execute_batch("CREATE TRIGGER reject_heli_evidence BEFORE INSERT ON memory_evidence WHEN (SELECT COUNT(*) FROM memory_evidence) > 0 BEGIN SELECT RAISE(ABORT,'injected evidence failure'); END;").unwrap();
    assert!(heli::review(&mut store, source.path(), true).is_err());
    assert!(store.list_memories(None, true).unwrap().is_empty());
    assert_eq!(store.revision().unwrap(), revision);
    store
        .conn
        .execute_batch("DROP TRIGGER reject_heli_evidence")
        .unwrap();
    assert!(heli::review(&mut store, source.path(), true).is_ok());
}

fn mock_response(message: Value) -> (LlmClient, mpsc::Receiver<Value>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "No local LLM request received");
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("{e}"),
            }
        };
        // macOS/BSD sockets inherit O_NONBLOCK from the listener (Linux does not); without
        // this, reads race the client and fail with WouldBlock under load.
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 4096];
        let body_start = loop {
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buffer[..n]);
            if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let header = String::from_utf8_lossy(&bytes[..body_start]);
        let length: usize = header
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|s| s.trim().parse().unwrap())
            })
            .unwrap();
        while bytes.len() - body_start < length {
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buffer[..n]);
        }
        let request: Value =
            serde_json::from_slice(&bytes[body_start..body_start + length]).unwrap();
        let _ = sender.send(request);
        let response = json!({"choices":[{"message":{"content":message.to_string()}}]}).to_string();
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).unwrap();
    });
    (
        LlmClient::new(
            &format!("http://{address}/v1"),
            "mock-model",
            "local-test-key".into(),
        )
        .unwrap(),
        receiver,
        handle,
    )
}

#[tokio::test]
async fn llm_structured_prompt_and_response_are_sanitized() {
    let (client, requests, handle) = mock_response(
        json!({"memories":[{"content":"{\"password\":\"output-private-value\",\"fact\":\"use SQLite\"}","confidence":0.7,"tags":["database"]}]}),
    );
    let memories=client.extract(r#"{"password":"input-private-value","nested":{"api_key":"escaped-\"-private-key"},"fact":"use SQLite"}"#,"project").await.unwrap();
    let request = requests.recv().unwrap();
    handle.join().unwrap();
    let sent = request.to_string();
    assert!(!sent.contains("input-private-value"));
    assert!(!sent.contains("private-key"));
    let input: Value =
        serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(input["text"]["fact"], "use SQLite");
    assert_eq!(memories.len(), 1);
    assert!(!memories[0].content.contains("output-private-value"));
}

#[tokio::test]
async fn llm_rejects_bad_confidence_and_invented_node_ids() {
    let (client, _, handle) =
        mock_response(json!({"memories":[{"content":"SQLite is used","confidence":1.2}]}));
    assert!(client.extract("SQLite is used", "project").await.is_err());
    handle.join().unwrap();
    let tree = TreeNode {
        id: "root".into(),
        ..TreeNode::default()
    };
    let (client, _, handle) =
        mock_response(json!({"relevant_nodes":["invented"],"confidence":0.9}));
    assert!(client.select("database", &tree).await.is_err());
    handle.join().unwrap();
    let (client, _, handle) = mock_response(json!({"relevant_nodes":["root"],"confidence":"high"}));
    assert!(client.select("database", &tree).await.is_err());
    handle.join().unwrap();
}

#[tokio::test]
async fn llm_summary_preserves_membership_and_tree_shape() {
    let tree = TreeNode {
        id: "root".into(),
        memory_ids: vec!["m1".into(), "m2".into()],
        children: vec![TreeNode {
            id: "child".into(),
            memory_ids: vec!["m2".into()],
            ..TreeNode::default()
        }],
        ..TreeNode::default()
    };
    let (client, _, handle) =
        mock_response(json!({"summaries":[{"id":"child","summary":"Database decisions"}]}));
    let output = client.summarize_tree(&tree).await.unwrap();
    handle.join().unwrap();
    assert_eq!(output.memory_ids, tree.memory_ids);
    assert_eq!(output.children[0].memory_ids, tree.children[0].memory_ids);
    assert_eq!(output.children[0].id, "child");
    assert_eq!(output.children[0].summary, "Database decisions");
    let (client, _, handle) = mock_response(json!({"summaries":[{"id":"invented","summary":"x"}]}));
    assert!(client.summarize_tree(&tree).await.is_err());
    handle.join().unwrap();
}

#[test]
fn llm_remote_plaintext_and_credential_urls_are_rejected() {
    assert!(LlmClient::new("http://example.com/v1", "model", "key".into()).is_err());
    assert!(LlmClient::new(
        "https://user:password@example.com/v1",
        "model",
        "key".into()
    )
    .is_err());
    assert!(LlmClient::new(
        "https://example.com/v1?api_key=private",
        "model",
        "key".into()
    )
    .is_err());
    assert!(LlmClient::new("https://example.com/v1", "password=private", "key".into()).is_err());
}

#[test]
fn heli_historical_twenty_task_corpus_preserves_lineage_and_refuted_history() {
    let tasks: Vec<heli::HeliTaskRecord> =
        serde_json::from_str(include_str!("fixtures/heli-history.json")).unwrap();
    assert_eq!(tasks.len(), 20);
    let proposals = heli::distill(&tasks);
    assert!(proposals.len() >= 20);
    assert!(proposals.iter().any(|p| !p.counterexamples.is_empty()));
    let mut store = MemoryStore::open(":memory:").unwrap();
    let ids = heli::persist_proposals(&mut store, &proposals).unwrap();
    assert_eq!(ids.len(), proposals.len());
    for (id, proposal) in ids.iter().zip(&proposals) {
        let memory = store.get_memory(id, true).unwrap().unwrap();
        assert!(!memory.eligible());
        let evidence = store.evidence(id).unwrap();
        assert_eq!(evidence[0]["source_id"], proposal.source_tasks[0]);
        assert_eq!(evidence[0]["source_revision"], proposal.source_revisions[0]);
        if proposal.claim.to_lowercase().contains("refuted") {
            assert_eq!(memory.lifecycle_state, LifecycleState::Rejected);
            assert_eq!(memory.verification_state, VerificationState::Refuted);
        }
    }
}
