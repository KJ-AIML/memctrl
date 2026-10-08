//! Bounded, explicit scopes. Nested scopes restore their parent on errors or panics.
use crate::sanitize::{sanitize_text, sanitize_value, validate_identifier};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    time::Instant,
};
use uuid::Uuid;

const MAX_OPERATIONS: usize = 1000;
const MAX_DEPTH: usize = 64;
#[derive(Debug)]
pub struct SpanTracker {
    active: Vec<Value>,
    completed: VecDeque<Value>,
    max_completed: usize,
}
impl Default for SpanTracker {
    fn default() -> Self {
        Self::new(50)
    }
}
impl SpanTracker {
    pub fn new(max_completed: usize) -> Self {
        Self {
            active: Vec::new(),
            completed: VecDeque::new(),
            max_completed: max_completed.min(10000),
        }
    }
    pub fn active(&self) -> Option<&Value> {
        self.active.last()
    }
    pub fn completed(&self) -> Vec<Value> {
        self.completed.iter().cloned().collect()
    }
    pub fn with_span<T>(
        &mut self,
        name: &str,
        metadata: Value,
        action: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        validate_identifier(name, "span name")?;
        if !metadata.is_object() {
            bail!("span metadata must be an object");
        }
        if self.active.len() >= MAX_DEPTH {
            bail!("span nesting limit exceeded");
        }
        self.active.push(json!({"name":name,"span_id":Uuid::new_v4().simple().to_string()[..16].to_owned(),"start_time":chrono::Utc::now().timestamp_micros() as f64/1_000_000.0,"metadata":sanitize_value(&metadata),"operations":[],"dropped_operations":0}));
        let started = Instant::now();
        let outcome = catch_unwind(AssertUnwindSafe(|| action(self)));
        let mut span = self.active.pop().expect("scope owns an active span");
        span["duration_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
        span["end_time"] = json!(chrono::Utc::now().timestamp_micros() as f64 / 1_000_000.0);
        match &outcome {
            Ok(Ok(_)) => span["status"] = json!("ok"),
            Ok(Err(error)) => {
                span["status"] = json!("error");
                span["error_message"] = json!(sanitize_text(&error.to_string()));
            }
            Err(_) => {
                span["status"] = json!("error");
                span["error_message"] = json!("scope panicked");
            }
        }
        let mut operations = BTreeMap::<String, usize>::new();
        let mut layers = BTreeMap::<String, usize>::new();
        for op in span["operations"].as_array().unwrap() {
            *operations
                .entry(op["operation"].as_str().unwrap_or("unknown").into())
                .or_default() += 1;
            if let Some(layer) = op["layer"].as_str() {
                *layers.entry(layer.into()).or_default() += 1;
            }
        }
        span["operation_counts"] = json!(operations);
        span["layer_access_counts"] = json!(layers);
        if self.max_completed > 0 {
            self.completed.push_back(span);
            while self.completed.len() > self.max_completed {
                self.completed.pop_front();
            }
        }
        match outcome {
            Ok(result) => result,
            Err(payload) => resume_unwind(payload),
        }
    }
    pub fn record_operation(&mut self, operation: &str, details: Value) -> Result<()> {
        validate_identifier(operation, "operation")?;
        if !details.is_object() {
            bail!("operation details must be an object");
        }
        for key in ["layer", "memory_id"] {
            if let Some(v) = details.get(key).filter(|v| !v.is_null()) {
                validate_identifier(
                    v.as_str()
                        .ok_or_else(|| anyhow::anyhow!("{key} must be a string"))?,
                    key,
                )?;
            }
        }
        if let Some(span) = self.active.last_mut() {
            if span["operations"].as_array().unwrap().len() >= MAX_OPERATIONS {
                span["dropped_operations"] =
                    json!(span["dropped_operations"].as_u64().unwrap_or(0) + 1);
                return Ok(());
            }
            let mut details = sanitize_value(&details);
            details["operation"] = json!(operation);
            details["timestamp"] =
                json!(chrono::Utc::now().timestamp_micros() as f64 / 1_000_000.0);
            span["operations"].as_array_mut().unwrap().push(details);
        }
        Ok(())
    }
    pub fn stats(&self) -> Value {
        let count = self.completed.len();
        let ops = self
            .completed
            .iter()
            .map(|v| v["operations"].as_array().unwrap().len())
            .sum::<usize>();
        let duration = self
            .completed
            .iter()
            .map(|v| v["duration_ms"].as_f64().unwrap_or(0.0))
            .sum::<f64>();
        json!({"total_spans":count,"total_operations":ops,"avg_operations_per_span":if count>0{ops as f64/count as f64}else{0.0},"total_duration_ms":duration})
    }
    /// Clear completed history. Active scopes remain owned by their closures.
    pub fn clear(&mut self) {
        self.completed.clear();
    }
}
