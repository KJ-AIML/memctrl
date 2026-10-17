//! Provider presets, wire formats and retry policy for the optional LLM layer. Every request
//! goes to a scripted loopback server; no provider is ever contacted.
use memctrl::{
    llm::{LlmClient, LlmConfig, LlmSettings},
    model::TreeNode,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};

struct Captured {
    path: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

type Script = Vec<(u16, Vec<(&'static str, String)>, String)>;

/// Serves the scripted responses in order, one connection each, and returns what it received.
/// Stops early when no further request arrives within two seconds.
fn scripted(responses: Script) -> (String, thread::JoinHandle<Vec<Captured>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let handle = thread::spawn(move || {
        let mut captured = Vec::new();
        for (status, reply_headers, body) in responses {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() > deadline {
                            return captured;
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
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
                if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let head = String::from_utf8_lossy(&bytes[..body_start]).into_owned();
            let mut lines = head.lines();
            let path = lines
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_owned();
            let headers: BTreeMap<String, String> = lines
                .filter_map(|l| l.split_once(':'))
                .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
                .collect();
            let length: usize = headers
                .get("content-length")
                .map(|v| v.parse().unwrap())
                .unwrap_or(0);
            while bytes.len() - body_start < length {
                let n = stream.read(&mut buffer).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
            }
            let request: Value =
                serde_json::from_slice(&bytes[body_start..body_start + length]).unwrap();
            captured.push(Captured {
                path,
                headers,
                body: request,
            });
            let extra = headers_text(&reply_headers);
            write!(
                stream,
                "HTTP/1.1 {status} Scripted\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
        captured
    });
    (format!("http://{address}/v1"), handle)
}

fn headers_text(headers: &[(&'static str, String)]) -> String {
    headers
        .iter()
        .map(|(k, v)| format!("{k}: {v}\r\n"))
        .collect()
}

fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| {
        pairs
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
    }
}

fn settings(provider: &str, model: Option<&str>, base_url: Option<String>) -> LlmSettings {
    LlmSettings {
        provider: Some(provider.into()),
        model: model.map(str::to_owned),
        base_url,
        ..Default::default()
    }
}

fn extraction(content: &str) -> String {
    json!({"memories": [{"content": content, "confidence": 0.9, "tags": ["db"]}]}).to_string()
}

#[test]
fn presets_and_precedence_resolve_like_grok_build() {
    let openai = LlmConfig::resolve(
        &LlmSettings::default(),
        env(&[("OPENAI_API_KEY", "sk-test-openai-000000000000000")]),
    )
    .unwrap();
    let summary = openai.summary();
    assert_eq!(summary["provider"], "openai");
    assert_eq!(summary["backend"], "chat/completions");
    assert_eq!(summary["base_url"], "https://api.openai.com/v1");
    assert_eq!(summary["model"], "gpt-4o-mini");
    assert!(
        !summary.to_string().contains("sk-test"),
        "summary never carries the key"
    );

    let missing_model = LlmConfig::resolve(
        &LlmSettings::default(),
        env(&[("MEMCTRL_LLM_PROVIDER", "xai"), ("XAI_API_KEY", "k")]),
    );
    assert!(missing_model
        .err()
        .unwrap()
        .to_string()
        .contains("--llm-model"));
    let xai = LlmConfig::resolve(
        &settings("grok", Some("grok-test"), None),
        env(&[("XAI_API_KEY", "k")]),
    )
    .unwrap();
    assert_eq!(xai.summary()["backend"], "responses");
    assert_eq!(xai.summary()["base_url"], "https://api.x.ai/v1");

    let anthropic = LlmConfig::resolve(
        &settings("anthropic", Some("claude-test"), None),
        env(&[("ANTHROPIC_API_KEY", "k")]),
    )
    .unwrap();
    assert_eq!(anthropic.summary()["backend"], "messages");
    // OpenAI variables do not leak into other providers.
    let leaked = LlmConfig::resolve(
        &settings("anthropic", Some("claude-test"), None),
        env(&[("OPENAI_API_KEY", "k")]),
    );
    assert!(leaked
        .err()
        .unwrap()
        .to_string()
        .contains("ANTHROPIC_API_KEY"));

    // Model precedence: explicit > MEMCTRL_LLM_MODEL > OPENAI_MODEL > preset.
    let vars = [
        ("OPENAI_API_KEY", "k"),
        ("MEMCTRL_LLM_MODEL", "from-memctrl"),
        ("OPENAI_MODEL", "from-openai"),
    ];
    assert_eq!(
        LlmConfig::resolve(&settings("openai", Some("explicit"), None), env(&vars))
            .unwrap()
            .summary()["model"],
        "explicit"
    );
    assert_eq!(
        LlmConfig::resolve(&LlmSettings::default(), env(&vars))
            .unwrap()
            .summary()["model"],
        "from-memctrl"
    );

    // Keys by variable name; a named but unset variable is an error.
    let named = LlmSettings {
        api_key_env: Some("TEAM_KEY".into()),
        ..Default::default()
    };
    assert_eq!(
        LlmConfig::resolve(&named, env(&[("TEAM_KEY", "k")]))
            .unwrap()
            .summary()["has_api_key"],
        true
    );
    assert!(LlmConfig::resolve(&named, env(&[]))
        .err()
        .unwrap()
        .to_string()
        .contains("TEAM_KEY is not set"));

    // Compatible servers need a base URL; keyless is allowed only on loopback.
    assert!(LlmConfig::resolve(&settings("ollama", Some("llama3"), None), env(&[])).is_err());
    let local = LlmConfig::resolve(
        &settings(
            "ollama",
            Some("llama3"),
            Some("http://localhost:11434/v1".into()),
        ),
        env(&[]),
    )
    .unwrap();
    assert!(LlmClient::from_config(local).is_ok());
    let remote = LlmConfig::resolve(
        &settings(
            "compatible",
            Some("m"),
            Some("https://gateway.example.com/v1".into()),
        ),
        env(&[]),
    )
    .unwrap();
    assert!(
        LlmClient::from_config(remote).is_err(),
        "keyless requests only on loopback"
    );
    // An OpenAI key in the environment never goes to a compatible server; only a key you name does.
    let gateway = || {
        settings(
            "compatible",
            Some("m"),
            Some("https://gateway.example.com/v1".into()),
        )
    };
    let openai_only = LlmConfig::resolve(&gateway(), env(&[("OPENAI_API_KEY", "k")])).unwrap();
    assert!(LlmClient::from_config(openai_only)
        .err()
        .unwrap()
        .to_string()
        .contains("--llm-api-key-env"));
    let memctrl_key = LlmConfig::resolve(
        &gateway(),
        env(&[("OPENAI_API_KEY", "k"), ("MEMCTRL_LLM_API_KEY", "g")]),
    )
    .unwrap();
    assert!(LlmClient::from_config(memctrl_key).is_ok());
    let named = LlmSettings {
        api_key_env: Some("GATEWAY_KEY".into()),
        ..gateway()
    };
    let named = LlmConfig::resolve(&named, env(&[("GATEWAY_KEY", "g")])).unwrap();
    assert!(LlmClient::from_config(named).is_ok());

    assert!(LlmConfig::resolve(
        &LlmSettings::default(),
        env(&[("OPENAI_API_KEY", "k"), ("MEMCTRL_LLM_MAX_RETRIES", "lots")])
    )
    .is_err());
    assert_eq!(
        LlmConfig::resolve(
            &LlmSettings::default(),
            env(&[("OPENAI_API_KEY", "k"), ("MEMCTRL_LLM_MAX_RETRIES", "99")])
        )
        .unwrap()
        .summary()["max_retries"],
        5
    );
}

#[tokio::test]
async fn anthropic_messages_wire_format() {
    let reply =
        json!({"content": [{"type": "text", "text": extraction("Orders live in PostgreSQL")}]})
            .to_string();
    let (base, server) = scripted(vec![(200, vec![], reply)]);
    let config = LlmConfig::resolve(
        &settings("anthropic", Some("claude-test"), Some(base)),
        env(&[("ANTHROPIC_API_KEY", "anthropic-local-key")]),
    )
    .unwrap();
    let memories = LlmClient::from_config(config)
        .unwrap()
        .extract("Orders live in PostgreSQL", "project")
        .await
        .unwrap();
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 1);
    let r = &requests[0];
    assert_eq!(r.path, "/v1/messages");
    assert_eq!(r.headers["x-api-key"], "anthropic-local-key");
    assert_eq!(r.headers["anthropic-version"], "2023-06-01");
    assert!(!r.headers.contains_key("authorization"));
    assert_eq!(r.body["max_tokens"], 4096);
    assert!(r.body["system"].is_string());
    assert_eq!(r.body["messages"][0]["role"], "user");
    assert_eq!(r.body["output_config"]["format"]["type"], "json_schema");
    assert_eq!(memories[0].content, "Orders live in PostgreSQL");
}

#[tokio::test]
async fn xai_responses_wire_format() {
    let output = json!({"relevant_nodes": ["root"], "confidence": 0.8}).to_string();
    let reply = json!({"output": [{"type": "message", "content": [{"type": "output_text", "text": output}]}]}).to_string();
    let (base, server) = scripted(vec![(200, vec![], reply)]);
    let config = LlmConfig::resolve(
        &settings("xai", Some("grok-test"), Some(base)),
        env(&[("XAI_API_KEY", "xai-local-key")]),
    )
    .unwrap();
    let tree = TreeNode {
        id: "root".into(),
        ..TreeNode::default()
    };
    let selected = LlmClient::from_config(config)
        .unwrap()
        .select("storage", &tree)
        .await
        .unwrap();
    let requests = server.join().unwrap();
    let r = &requests[0];
    assert_eq!(r.path, "/v1/responses");
    assert_eq!(r.headers["authorization"], "Bearer xai-local-key");
    assert_eq!(r.body["store"], false);
    assert_eq!(r.body["input"][1]["role"], "user");
    assert_eq!(r.body["text"]["format"]["type"], "json_schema");
    assert_eq!(r.body["text"]["format"]["strict"], true);
    assert_eq!(selected, vec!["root".to_string()]);
}

#[tokio::test]
async fn openai_uses_strict_json_schema_and_local_servers_go_keyless() {
    let chat = |text: String| json!({"choices": [{"message": {"content": text}}]}).to_string();
    let (base, server) = scripted(vec![(
        200,
        vec![],
        chat(extraction("Deploys run on Fridays")),
    )]);
    let config = LlmConfig::resolve(
        &settings("openai", None, Some(base)),
        env(&[("OPENAI_API_KEY", "openai-local-key")]),
    )
    .unwrap();
    LlmClient::from_config(config)
        .unwrap()
        .extract("Deploys run on Fridays", "project")
        .await
        .unwrap();
    let r = &server.join().unwrap()[0];
    assert_eq!(r.path, "/v1/chat/completions");
    assert_eq!(r.body["response_format"]["type"], "json_schema");
    assert_eq!(r.body["response_format"]["json_schema"]["strict"], true);

    let (base, server) = scripted(vec![(200, vec![], chat(extraction("Ollama runs locally")))]);
    let config =
        LlmConfig::resolve(&settings("ollama", Some("llama3"), Some(base)), env(&[])).unwrap();
    LlmClient::from_config(config)
        .unwrap()
        .extract("Ollama runs locally", "session")
        .await
        .unwrap();
    let r = &server.join().unwrap()[0];
    assert!(
        !r.headers.contains_key("authorization"),
        "no key sent to a keyless local server"
    );
    assert_eq!(r.body["response_format"]["type"], "json_object");
}

#[tokio::test]
async fn structured_output_falls_back_once_when_a_provider_rejects_it() {
    let reply =
        json!({"content": [{"type": "text", "text": extraction("Fallback works")}]}).to_string();
    let (base, server) = scripted(vec![
        (
            400,
            vec![],
            json!({"type": "error", "error": {"type": "invalid_request_error"}}).to_string(),
        ),
        (200, vec![], reply),
    ]);
    let config = LlmConfig::resolve(
        &settings("anthropic", Some("claude-test"), Some(base)),
        env(&[("ANTHROPIC_API_KEY", "k")]),
    )
    .unwrap();
    let memories = LlmClient::from_config(config)
        .unwrap()
        .extract("Fallback works", "project")
        .await
        .unwrap();
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].body.get("output_config").is_some());
    assert!(requests[1].body.get("output_config").is_none());
    assert_eq!(memories[0].content, "Fallback works");
}

#[tokio::test]
async fn retries_follow_the_grok_build_policy() {
    let chat = |text: String| json!({"choices": [{"message": {"content": text}}]}).to_string();
    let client = |base: String| {
        LlmClient::new(&base, "local-model", "retry-test-key".into())
            .unwrap()
            .with_retry_policy(2, Duration::from_millis(5))
    };

    // 5xx is retried.
    let (base, server) = scripted(vec![
        (503, vec![], "{}".into()),
        (200, vec![], chat(extraction("Recovered after 503"))),
    ]);
    assert_eq!(
        client(base)
            .extract("Recovered after 503", "session")
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(server.join().unwrap().len(), 2);

    // Auth errors are not retried, and the error names type and request id but never the key.
    let (base, server) = scripted(vec![
        (
            401,
            vec![("request-id", "req_123".into())],
            json!({"error": {"type": "authentication_error"}}).to_string(),
        ),
        (200, vec![], chat(extraction("must not be reached"))),
    ]);
    let error = client(base)
        .extract("x", "session")
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.contains("HTTP 401")
            && error.contains("authentication_error")
            && error.contains("req_123"),
        "{error}"
    );
    assert!(!error.contains("retry-test-key"));
    assert_eq!(server.join().unwrap().len(), 1);

    // 429 without Retry-After (e.g. a spend cap) is final; with a short Retry-After it is retried once.
    let (base, server) = scripted(vec![
        (429, vec![], "{}".into()),
        (200, vec![], chat(extraction("no"))),
    ]);
    assert!(client(base).extract("x", "session").await.is_err());
    assert_eq!(server.join().unwrap().len(), 1);
    let (base, server) = scripted(vec![
        (429, vec![("retry-after", "0".into())], "{}".into()),
        (200, vec![], chat(extraction("Retried after 429"))),
    ]);
    assert_eq!(
        client(base)
            .extract("Retried after 429", "session")
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(server.join().unwrap().len(), 2);
}
