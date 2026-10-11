//! Explicit local tracing and OTLP JSON export. No global hooks or network calls.
use crate::{
    sanitize::{sanitize_text, sanitize_value, validate_identifier},
    store::MemoryStore,
};
use anyhow::{bail, Result};
use rusqlite::{params, types::ValueRef};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, time::Instant};
use uuid::Uuid;

const FIELDS: &[(&str, &str)] = &[
    ("memory_id", "gen_ai.memory.id"),
    ("layer", "gen_ai.memory.layer"),
    ("memory_type", "gen_ai.memory.type"),
    ("confidence", "gen_ai.memory.confidence"),
    ("query", "gen_ai.memory.query"),
    ("top_k", "gen_ai.memory.top_k"),
    ("results_count", "gen_ai.memory.results_count"),
    ("backend", "gen_ai.memory.backend"),
];

fn ensure(store: &MemoryStore) -> Result<()> {
    store.conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS otel_spans (
        id TEXT PRIMARY KEY, trace_id TEXT NOT NULL, span_id TEXT NOT NULL,
        operation TEXT NOT NULL, timestamp REAL NOT NULL, duration_ms REAL NOT NULL,
        memory_id TEXT, layer TEXT, memory_type TEXT, confidence REAL, query TEXT,
        top_k INTEGER, results_count INTEGER, status TEXT NOT NULL,
        error_message TEXT, attributes_json TEXT, service_name TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS idx_otel_spans_trace ON otel_spans(trace_id);
        CREATE INDEX IF NOT EXISTS idx_otel_spans_op ON otel_spans(operation);",
    )?;
    Ok(())
}

fn validate(operation: &str, attributes: &Value) -> Result<()> {
    validate_identifier(operation, "operation")?;
    if !attributes.is_object() {
        bail!("span attributes must be an object");
    }
    for key in [
        "layer",
        "memory_id",
        "memory_type",
        "trace_id",
        "span_id",
        "service_name",
    ] {
        if let Some(value) = attributes.get(key).filter(|v| !v.is_null()) {
            let value = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{key} must be a string"))?;
            validate_identifier(value, key)?;
        }
    }
    for key in [
        "gen_ai.memory.layer",
        "gen_ai.memory.id",
        "gen_ai.memory.type",
        "service.name",
    ] {
        if let Some(value) = attributes.get(key).filter(|v| !v.is_null()) {
            validate_identifier(
                value
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("{key} must be a string"))?,
                key,
            )?;
        }
    }
    if let Some(status) = attributes.get("status") {
        if status != "ok" && status != "error" {
            bail!("span status must be ok or error");
        }
    }
    if let Some(value) = attributes.get("confidence").filter(|v| !v.is_null()) {
        let confidence = value
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("confidence must be a number"))?;
        if !(0.0..=1.0).contains(&confidence) {
            bail!("confidence must be between zero and one");
        }
    }
    Ok(())
}

/// Record a sanitized span, retaining the last 10,000 spans.
pub fn record(
    store: &mut MemoryStore,
    operation: &str,
    duration_ms: f64,
    attributes: Value,
) -> Result<String> {
    validate(operation, &attributes)?;
    if !duration_ms.is_finite() || duration_ms < 0.0 {
        bail!("duration must be finite and nonnegative");
    }
    let attributes = sanitize_value(&attributes);
    let id = Uuid::new_v4().to_string();
    let trace_id = attributes["trace_id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let span_id = attributes["span_id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string()[..16].to_owned());
    let pick = |field: &str, semantic: &str| {
        attributes
            .get(field)
            .or_else(|| attributes.get(semantic))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let text = |field: &str, semantic: &str| pick(field, semantic).as_str().map(str::to_owned);
    let confidence = pick("confidence", "gen_ai.memory.confidence").as_f64();
    let top_k = pick("top_k", "gen_ai.memory.top_k").as_i64();
    let results_count = pick("results_count", "gen_ai.memory.results_count").as_i64();
    ensure(store)?;
    // Composes with a caller's transaction (nested savepoint) instead of failing on BEGIN.
    let guard = crate::store::AtomicWrite::new(&store.conn)?;
    store.conn.execute("INSERT INTO otel_spans (id,trace_id,span_id,operation,timestamp,duration_ms,memory_id,layer,memory_type,confidence,query,top_k,results_count,status,error_message,attributes_json,service_name) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",params![id,trace_id,span_id,operation,chrono::Utc::now().timestamp_micros() as f64/1_000_000.0,duration_ms,text("memory_id","gen_ai.memory.id"),text("layer","gen_ai.memory.layer"),text("memory_type","gen_ai.memory.type"),confidence,text("query","gen_ai.memory.query"),top_k,results_count,attributes["status"].as_str().unwrap_or("ok"),attributes["error_message"].as_str(),attributes.to_string(),attributes["service_name"].as_str().unwrap_or("memctrl")])?;
    store.conn.execute("DELETE FROM otel_spans WHERE id NOT IN (SELECT id FROM otel_spans ORDER BY timestamp DESC, rowid DESC LIMIT 10000)",[])?;
    guard.commit()?;
    Ok(id)
}

fn normalized(mut row: Value) -> Value {
    let mut attrs = row
        .get("attributes")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(encoded) = row["attributes_json"].as_str() {
        if let Ok(Value::Object(extra)) = serde_json::from_str::<Value>(encoded) {
            attrs.extend(extra);
        }
    }
    attrs.entry("gen_ai.system").or_insert(json!("memctrl"));
    attrs
        .entry("gen_ai.operation.name")
        .or_insert(row["operation"].clone());
    attrs
        .entry("gen_ai.memory.operation")
        .or_insert(row["operation"].clone());
    for (field, key) in FIELDS {
        if let Some(v) = row
            .get(*field)
            .filter(|v| !v.is_null() && v.as_str() != Some(""))
        {
            attrs.entry(*key).or_insert(v.clone());
        }
    }
    row["attributes"] = Value::Object(attrs);
    if row["status"].as_str().unwrap_or("").is_empty() {
        row["status"] = json!("ok");
    }
    if row["service_name"].as_str().unwrap_or("").is_empty() {
        row["service_name"] = json!("memctrl");
    }
    row.as_object_mut().unwrap().remove("attributes_json");
    sanitize_value(&row)
}

/// Read all dedicated legacy fields, even when attributes_json is empty.
pub fn spans(store: &MemoryStore, limit: usize) -> Result<Vec<Value>> {
    let exists: bool = store.conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='otel_spans')",
        [],
        |r| r.get(0),
    )?;
    if !exists || limit == 0 {
        return Ok(vec![]);
    }
    let mut statement = store
        .conn
        .prepare("SELECT * FROM otel_spans ORDER BY timestamp DESC, rowid DESC LIMIT ?1")?;
    let names = statement
        .column_names()
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let rows = statement
        .query_map([limit.min(i64::MAX as usize) as i64], |r| {
            let mut object = Map::new();
            for (index, name) in names.iter().enumerate() {
                let value = match r.get_ref(index)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(n) => json!(n),
                    ValueRef::Real(n) => json!(n),
                    ValueRef::Text(v) => json!(String::from_utf8_lossy(v)),
                    ValueRef::Blob(_) => Value::Null,
                };
                object.insert(name.clone(), value);
            }
            Ok(normalized(Value::Object(object)))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn stats(store: &MemoryStore) -> Result<Value> {
    let rows = spans(store, usize::MAX)?;
    let mut by_operation = BTreeMap::<String, usize>::new();
    let mut by_layer = BTreeMap::<String, usize>::new();
    let mut duration = 0.0;
    let mut errors = 0;
    for row in &rows {
        *by_operation
            .entry(row["operation"].as_str().unwrap_or("unknown").to_owned())
            .or_default() += 1;
        if let Some(layer) = row["layer"].as_str() {
            *by_layer.entry(layer.into()).or_default() += 1;
        }
        duration += row["duration_ms"].as_f64().unwrap_or(0.0);
        if row["status"] == "error" {
            errors += 1;
        }
    }
    let total = rows.len();
    Ok(
        json!({"total_spans":total,"by_operation":by_operation,"by_layer":by_layer,"total_duration_ms":duration,"avg_duration_ms":if total>0 {duration/total as f64}else{0.0},"error_count":errors,"error_rate":if total>0 {errors as f64/total as f64}else{0.0}}),
    )
}

fn any_value(value: &Value) -> Value {
    match value {
        Value::Bool(v) => json!({"boolValue":v}),
        Value::Number(v) if v.is_i64() || v.is_u64() => json!({"intValue":v.to_string()}),
        Value::Number(v) => json!({"doubleValue":v}),
        Value::String(v) => json!({"stringValue":v}),
        Value::Array(v) => {
            json!({"arrayValue":{"values":v.iter().map(any_value).collect::<Vec<_>>()}})
        }
        Value::Object(v) => {
            json!({"kvlistValue":{"values":v.iter().map(|(k,v)|json!({"key":k,"value":any_value(v)})).collect::<Vec<_>>()}})
        }
        Value::Null => json!({"stringValue":""}),
    }
}

fn hex_id(value: &Value, digits: usize) -> String {
    let candidate = value.as_str().unwrap_or("").replace('-', "");
    if candidate.len() == digits
        && candidate.bytes().all(|b| b.is_ascii_hexdigit())
        && candidate.bytes().any(|b| b != b'0')
    {
        return candidate.to_ascii_lowercase();
    }
    // Stable identifiers for imported spans with arbitrary legacy IDs.
    let mut bytes = vec![0u8; digits / 2];
    for (i, b) in candidate.bytes().enumerate() {
        let n = i % bytes.len();
        bytes[n] = bytes[n].wrapping_mul(31).wrapping_add(b);
    }
    if bytes.iter().all(|b| *b == 0) {
        bytes[0] = 1;
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn otlp_json(rows: &[Value]) -> Value {
    let mut services = BTreeMap::<String, Vec<Value>>::new();
    for row in rows {
        let row = normalized(row.clone());
        let start = (row["timestamp"].as_f64().unwrap_or(0.0).max(0.0) * 1_000_000_000.0) as u64;
        let duration = (row["duration_ms"].as_f64().unwrap_or(0.0).max(0.0) * 1_000_000.0) as u64;
        let attrs = row["attributes"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| json!({"key":k,"value":any_value(v)}))
            .collect::<Vec<_>>();
        let service = row["service_name"].as_str().unwrap_or("memctrl").to_owned();
        let span = json!({"traceId":hex_id(&row["trace_id"],32),"spanId":hex_id(&row["span_id"],16),"name":format!("gen_ai.memory.{}",row["operation"].as_str().unwrap_or("unknown")),"kind":1,"startTimeUnixNano":start.to_string(),"endTimeUnixNano":start.saturating_add(duration).to_string(),"attributes":attrs,"status":{"code":if row["status"]=="error" {2}else{1},"message":row["error_message"].as_str().unwrap_or("")}});
        services.entry(service).or_default().push(span);
    }
    json!({"resourceSpans":services.into_iter().map(|(service,spans)|json!({"resource":{"attributes":[{"key":"service.name","value":{"stringValue":service}},{"key":"telemetry.sdk.name","value":{"stringValue":"memctrl"}},{"key":"telemetry.sdk.language","value":{"stringValue":"rust"}}]},"scopeSpans":[{"scope":{"name":"memctrl","version":env!("CARGO_PKG_VERSION")},"spans":spans}]})).collect::<Vec<_>>()})
}

/// Trace only this explicit closure. Errors are returned unchanged after recording.
pub fn traced<T>(
    store: &mut MemoryStore,
    operation: &str,
    attributes: Value,
    action: impl FnOnce(&mut MemoryStore) -> Result<T>,
) -> Result<T> {
    validate(operation, &attributes)?;
    let start = Instant::now();
    let result = action(store);
    let mut attrs = attributes;
    attrs["status"] = json!(if result.is_ok() { "ok" } else { "error" });
    if let Err(error) = &result {
        attrs["error_message"] = json!(sanitize_text(&error.to_string()));
    }
    let tracing = record(
        store,
        operation,
        start.elapsed().as_secs_f64() * 1000.0,
        attrs,
    );
    match result {
        Ok(value) => {
            tracing?;
            Ok(value)
        }
        Err(error) => Err(error),
    }
}
