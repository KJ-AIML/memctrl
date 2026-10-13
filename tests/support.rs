use memctrl::{
    agent, doctor, installer, model::NewMemory, rules::Rules, span::SpanTracker,
    store::MemoryStore, telemetry,
};
use serde_json::json;

fn store() -> MemoryStore {
    MemoryStore::open(":memory:").unwrap()
}

#[test]
fn checkpoint_upserts_without_becoming_a_memory_and_redacts_json() {
    let mut s = store();
    agent::put_checkpoint(
        &mut s,
        "thread-1",
        json!({"values":{"password":"secret-one","nested":{"api_key":"credential-two"}},"step":1}),
    )
    .unwrap();
    agent::put_checkpoint(
        &mut s,
        "thread-1",
        json!({"values":{"password":"secret-three"},"step":2}),
    )
    .unwrap();
    let v = agent::get_checkpoint(&s, "thread-1").unwrap().unwrap();
    assert_eq!(v["step"], 2);
    assert!(!v.to_string().contains("secret-three"));
    assert_eq!(agent::list_checkpoints(&s, 10).unwrap().len(), 1);
    assert!(s.list_memories(None, true).unwrap().is_empty());
    assert!(agent::put_checkpoint(&mut s, "password=unsafe-password", json!({})).is_err());
    assert_eq!(agent::list_checkpoints(&s, 10).unwrap().len(), 1);
}

#[test]
fn ordinary_agent_message_is_retained_and_queries_have_trace() {
    let mut s = store();
    let rules = Rules::default();
    let out=agent::memory_node(&mut s,json!({"messages":[{"content":"The overnight shipment arrived at the northern warehouse."}],"memory_query":"shipment warehouse"}),&rules,true).unwrap();
    assert_eq!(s.list_memories(None, false).unwrap().len(), 1);
    assert!(!out["memory_facts"].as_array().unwrap().is_empty());
    assert!(out["memory_trace"].is_array());
    agent::memory_node(
        &mut s,
        json!({"messages":[{"content":"short"}]}),
        &rules,
        true,
    )
    .unwrap();
    assert_eq!(s.list_memories(None, false).unwrap().len(), 1);
}

#[test]
fn agent_rejects_invalid_optional_types_before_extracting() {
    let mut s = store();
    let state = json!({"messages":[{"content":"The overnight shipment arrived at the northern warehouse."}],"memory_query":42});
    assert!(agent::memory_node(&mut s, state, &Rules::default(), true).is_err());
    assert!(s.list_memories(None, true).unwrap().is_empty());
}

#[test]
fn legacy_span_dedicated_fields_survive_export_and_secrets_do_not() {
    let mut s = store();
    telemetry::record(&mut s, "store", 1.0, json!({})).unwrap();
    s.conn.execute("INSERT INTO otel_spans (id,trace_id,span_id,operation,timestamp,duration_ms,memory_id,layer,memory_type,confidence,query,top_k,results_count,status,error_message,attributes_json,service_name) VALUES ('legacy','0123456789abcdef0123456789abcdef','0123456789abcdef','search',1700000000,12,'memory-1','project','semantic',0.8,'warehouse',7,3,'error','password=leak-secret','{\"custom\":true}', 'legacy-service')",[]).unwrap();
    let rows = telemetry::spans(&s, 10).unwrap();
    let legacy = rows.iter().find(|x| x["id"] == "legacy").unwrap();
    assert_eq!(legacy["attributes"]["gen_ai.memory.id"], "memory-1");
    let payload = telemetry::otlp_json(&rows);
    let resources = payload["resourceSpans"].as_array().unwrap();
    let resource = resources
        .iter()
        .find(|x| x["resource"]["attributes"][0]["value"]["stringValue"] == "legacy-service")
        .unwrap();
    let span = &resource["scopeSpans"][0]["spans"][0];
    for (key, value) in [
        ("gen_ai.memory.layer", json!({"stringValue":"project"})),
        ("gen_ai.memory.type", json!({"stringValue":"semantic"})),
        ("gen_ai.memory.confidence", json!({"doubleValue":0.8})),
        ("gen_ai.memory.top_k", json!({"intValue":"7"})),
        ("gen_ai.memory.results_count", json!({"intValue":"3"})),
    ] {
        assert!(span["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["key"] == key && x["value"] == value));
    }
    assert_eq!(span["status"]["code"], 2);
    assert!(!span["status"]["message"].as_str().unwrap().is_empty());
    assert!(!payload.to_string().contains("leak-secret"));
}

#[test]
fn traced_records_success_and_error_without_global_effects() {
    let mut s = store();
    assert!(telemetry::record(&mut s, "api_key=bad-operation", 1.0, json!({})).is_err());
    assert!(telemetry::spans(&s, 10).unwrap().is_empty());
    let v = telemetry::traced(&mut s, "store", json!({"password":"hidden"}), |s| {
        s.insert_memory(NewMemory {
            content: "Shipment details recorded".into(),
            ..Default::default()
        })
    })
    .unwrap();
    assert!(!v.is_empty());
    let failure: anyhow::Result<()> = telemetry::traced(&mut s, "search", json!({}), |_| {
        anyhow::bail!("password=hidden-error")
    });
    assert!(failure.is_err());
    let spans = telemetry::spans(&s, 10).unwrap();
    assert_eq!(spans.len(), 2);
    assert_eq!(telemetry::stats(&s).unwrap()["error_count"], 1);
    assert!(!serde_json::to_string(&spans)
        .unwrap()
        .contains("hidden-error"));
    s.insert_memory(NewMemory {
        content: "untraced separate memory".into(),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(telemetry::spans(&s, 10).unwrap().len(), 2);
}

#[test]
fn tracker_restores_outer_scope_and_bounds_operations_and_history() {
    let mut tracker = SpanTracker::new(1);
    tracker
        .with_span("outer", json!({"password":"hidden"}), |t| {
            t.record_operation("store", json!({"layer":"session"}))?;
            t.with_span("inner", json!({}), |t| {
                t.record_operation("search", json!({}))?;
                Ok(())
            })?;
            assert_eq!(t.active().unwrap()["name"], "outer");
            for _ in 0..1200 {
                t.record_operation("retrieve", json!({}))?;
            }
            Ok(())
        })
        .unwrap();
    assert!(tracker.active().is_none());
    assert_eq!(tracker.completed().len(), 1);
    assert!(
        tracker.completed()[0]["operations"]
            .as_array()
            .unwrap()
            .len()
            <= 1000
    );
    assert!(!tracker.completed()[0].to_string().contains("hidden"));
    let error: anyhow::Result<()> =
        tracker.with_span("fails", json!({}), |_| anyhow::bail!("failed"));
    assert!(error.is_err());
    assert!(tracker.active().is_none());
    assert_eq!(tracker.stats()["total_spans"], 1);
}

#[test]
fn installer_places_six_tool_skills_under_explicit_scope() {
    let dir = tempfile::tempdir().unwrap();
    for tool in ["claude_code", "cursor", "codex", "axga", "pi", "kimi"] {
        for project in [true, false] {
            let path = installer::install(tool, project, dir.path()).unwrap();
            assert!(path.starts_with(dir.path()));
            let content = std::fs::read_to_string(path).unwrap();
            assert!(content.contains("memctrl query"));
            assert!(content.contains("Never accept a quarantined memory yourself"));
        }
    }
    assert!(installer::install("../escape", true, dir.path()).is_err());
}

#[test]
fn doctor_reports_exposure_low_confidence_and_telemetry_errors() {
    let mut s = store();
    s.insert_memory(NewMemory {
        content: "Shipment delay expected".into(),
        source: "unknown-origin".into(),
        confidence: 0.2,
        ..Default::default()
    })
    .unwrap();
    telemetry::record(
        &mut s,
        "search",
        4.0,
        json!({"status":"error","error_message":"broken"}),
    )
    .unwrap();
    let r = doctor::report(&s).unwrap();
    assert_eq!(r["status"], "warn");
    assert_eq!(r["low_confidence_count"], 1);
    assert_eq!(r["risky_source_count"], 1);
    assert_eq!(r["opentelemetry"]["error_spans"], 1);
    assert_eq!(r["retrieval_exposure"]["exposure_rate"], 0.0);
}

#[test]
fn short_pattern_preference_is_extracted_without_fallback_length_requirement() {
    let mut s = store();
    agent::memory_node(
        &mut s,
        json!({"messages":["I prefer Rust."]}),
        &Rules::default(),
        true,
    )
    .unwrap();
    let memories = s.list_memories(None, false).unwrap();
    assert_eq!(memories.len(), 1);
    assert!(memories[0].content.to_lowercase().contains("prefer rust"));
}

#[test]
fn checkpoint_write_failure_preserves_previous_state() {
    let mut s = store();
    agent::put_checkpoint(&mut s, "thread-atomic", json!({"step":1})).unwrap();
    s.conn.execute_batch("CREATE TRIGGER checkpoint_failure BEFORE UPDATE ON agent_checkpoints BEGIN SELECT RAISE(ABORT,'simulated failure'); END;").unwrap();
    assert!(agent::put_checkpoint(&mut s, "thread-atomic", json!({"step":2})).is_err());
    assert_eq!(
        agent::get_checkpoint(&s, "thread-atomic").unwrap().unwrap()["step"],
        1
    );
}

#[test]
fn legacy_nullable_span_fields_have_safe_defaults() {
    let mut s = store();
    telemetry::record(&mut s, "search", 0.0, json!({})).unwrap();
    s.conn
        .execute(
            "UPDATE otel_spans SET status='',service_name='',attributes_json=NULL,query=NULL",
            [],
        )
        .unwrap();
    let rows = telemetry::spans(&s, 10).unwrap();
    assert_eq!(rows[0]["status"], "ok");
    assert_eq!(rows[0]["service_name"], "memctrl");
    assert_eq!(
        telemetry::otlp_json(&rows)["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["status"]
            ["code"],
        1
    );
}

#[test]
fn scoped_tracker_recovers_outer_scope_after_inner_panic() {
    let mut t = SpanTracker::new(10);
    t.with_span("outer", json!({}), |t| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: anyhow::Result<()> = t.with_span("inner", json!({}), |_| panic!("inner panic"));
        }));
        assert!(result.is_err());
        assert_eq!(t.active().unwrap()["name"], "outer");
        t.record_operation("retrieve", json!({}))?;
        Ok(())
    })
    .unwrap();
    assert!(t.active().is_none());
    assert_eq!(t.completed().len(), 2);
    assert_eq!(t.completed()[0]["status"], "error");
    assert_eq!(t.completed()[1]["operations"].as_array().unwrap().len(), 1);
}

#[cfg(unix)]
#[test]
fn installer_rejects_tool_directory_symlink_outside_scope() {
    let scope = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), scope.path().join(".agents")).unwrap();
    assert!(installer::install("codex", true, scope.path()).is_err());
    assert!(!outside.path().join("skills").exists());
}
