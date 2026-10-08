//! Shared structured and textual secret/PII redaction boundary.
use anyhow::{bail, Result};
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;
fn patterns() -> &'static Vec<(Regex, &'static str)> {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    PATTERNS.get_or_init(|| [
        (r"(?is)-----BEGIN (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----.*?(?:-----END (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----|\z)","PRIVATE_KEY"),
        (r"(?i)\bsk-[a-z0-9_-]{20,}\b","API_KEY"),
        (r"(?i)\bgh[pousr]_[a-z0-9]{36,}\b","GITHUB_TOKEN"),
        (r"(?i)\bgithub_pat_[a-z0-9_]{20,}\b","GITHUB_TOKEN"),
        (r"(?i)\bAKIA[0-9A-Z]{16}\b","AWS_KEY"),
        (r#"(?i)\b(api[_\-\s]?key["']?\s*[=:]\s*(?:\"[^\"]*\"|'[^']*'|[^\s,;}]+))"#,"API_KEY"),
        (r#"(?i)\b(?:bearer\s+[^\s,;}]+|(?:auth|access|refresh)[_-]?token["']?\s*[=:]\s*(?:\"[^\"]*\"|'[^']*'|[^\s,;}]+))"#,"TOKEN"),
        (r#"(?i)\b(?:password|passwd|passphrase)["']?\s*[=:]\s*(?:\"[^\"]*\"|'[^']*'|[^\s,;}]+)"#,"PASSWORD"),
        (r#"(?i)\b(?:secret|client[_-]?secret)["']?\s*[=:]\s*(?:\"[^\"]*\"|'[^']*'|[^\s,;}]+)"#,"SECRET"),
        (r"(?i)\b[a-z][a-z0-9+.-]*://[^\s/@:]+:[^\s/@]+@","CREDENTIAL"),
        (r"\b[A-Za-z0-9/+=]{40,}\b","TOKEN"),
        (r"(?i)\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[a-z]{2,}\b","EMAIL"),
        (r"\b\d{3}-\d{2}-\d{4}\b","SSN"),
        (r"\b\d{3}-\d{3}-\d{4}\b","PHONE"),
        (r"\b\d{10,12}\b","PHONE_INTL"),
    ].into_iter().map(|(p,label)|(Regex::new(p).expect("static redaction regex"),label)).collect())
}
fn redact_raw(input: &str) -> String {
    patterns()
        .iter()
        .fold(input.to_owned(), |text, (pattern, label)| {
            pattern
                .replace_all(&text, format!("[REDACTED_{label}]"))
                .into_owned()
        })
}
fn sensitive_label(key: &str) -> Option<&'static str> {
    let key: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    if key.ends_with("password") || key.ends_with("passwd") || key.ends_with("passphrase") {
        Some("PASSWORD")
    } else if key.ends_with("apikey") {
        Some("API_KEY")
    } else if key.ends_with("authorization")
        || key == "authorizationheader"
        || key.ends_with("token")
        || key.ends_with("credential")
        || key.ends_with("credentials")
        || key == "cookie"
        || key == "setcookie"
    {
        Some("TOKEN")
    } else if key.ends_with("privatekey") {
        Some("PRIVATE_KEY")
    } else if key.ends_with("secret") || key == "secrets" || key == "awssecretaccesskey" {
        Some("SECRET")
    } else {
        None
    }
}
pub fn sanitize_text(input: &str) -> String {
    // Parsing first prevents the textual matcher from leaving JSON secret values behind.
    match serde_json::from_str::<Value>(input) {
        Ok(value) if value.is_object() || value.is_array() => sanitize_value(&value).to_string(),
        _ => redact_raw(input),
    }
}
pub fn sanitize_value(input: &Value) -> Value {
    match input {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let safe = match key.as_str() {
                        "source_revision" | "commitSha" | "commit_sha" => match value {
                            Value::String(s) => Value::String(sanitize_revision(s)),
                            _ => sanitize_value(value),
                        },
                        "source_revisions" => match value {
                            Value::Array(values) => Value::Array(
                                values
                                    .iter()
                                    .map(|v| match v {
                                        Value::String(s) => Value::String(sanitize_revision(s)),
                                        _ => sanitize_value(v),
                                    })
                                    .collect(),
                            ),
                            _ => sanitize_value(value),
                        },
                        _ => match sensitive_label(key) {
                            Some(label) => Value::String(format!("[REDACTED_{label}]")),
                            None => sanitize_value(value),
                        },
                    };
                    (redact_raw(key), safe)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(sanitize_value).collect()),
        Value::String(text) => Value::String(sanitize_text(text)),
        other => other.clone(),
    }
}
pub fn sanitize_revision(input: &str) -> String {
    if matches!(input.len(), 40 | 64) && input.chars().all(|c| c.is_ascii_hexdigit()) {
        input.into()
    } else {
        sanitize_text(input)
    }
}
pub fn has_secrets(input: &str) -> bool {
    match serde_json::from_str::<Value>(input) {
        Ok(value) if value.is_object() || value.is_array() => sanitize_value(&value) != value,
        _ => patterns()
            .iter()
            .any(|(pattern, _)| pattern.is_match(input)),
    }
}
pub fn validate_identifier(input: &str, label: &str) -> Result<()> {
    if input.trim().is_empty()
        || input.chars().any(char::is_control)
        || has_secrets(input)
        || redact_raw(input) != input
    {
        bail!("invalid {label}: identifiers must be nonempty and contain no credentials or control characters")
    }
    Ok(())
}
