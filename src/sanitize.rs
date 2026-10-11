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
        (r#"(?i)\b(api[_\-\s]?key["']?\s*[=:]\s*(?:\"[^\"]*\"|'[^']*'|\S+))"#,"API_KEY"),
        (r#"(?i)\b(?:bearer\s+\S+|(?:auth|access|refresh)[_-]?token["']?\s*[=:]\s*(?:\"[^\"]*\"|'[^']*'|\S+))"#,"TOKEN"),
        (r#"(?i)\b(?:password|passwd|passphrase)["']?\s*[=:]\s*(?:\"[^\"]*\"|'[^']*'|\S+)"#,"PASSWORD"),
        (r#"(?i)\b(?:secret|client[_-]?secret)["']?\s*[=:]\s*(?:\"[^\"]*\"|'[^']*'|\S+)"#,"SECRET"),
        (r"(?i)\b[a-z][a-z0-9+.-]*://[^\s/@:]+:[^\s/@]+@","CREDENTIAL"),
        (r"\b[A-Za-z0-9/+=]{40,}\b","TOKEN"),
        (r"(?i)\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[a-z]{2,}\b","EMAIL"),
        (r"\b\d{3}-\d{2}-\d{4}\b","SSN"),
        (r"\b\d{3}-\d{3}-\d{4}\b","PHONE"),
        (r"\b\d{10,12}\b","PHONE_INTL"),
    ].into_iter().map(|(p,label)|(Regex::new(p).expect("static redaction regex"),label)).collect())
}
fn uuid_pattern() -> &'static Regex {
    static UUID: OnceLock<Regex> = OnceLock::new();
    UUID.get_or_init(|| {
        Regex::new(r"[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}")
            .expect("static uuid regex")
    })
}
fn redact_raw(input: &str) -> String {
    // Canonical UUIDs are identities, not PII: a v4 id whose last group happens to be all
    // digits must not read as a phone number. Shield them from the patterns, then restore.
    // A UUID used as a secret value (`api_key=<uuid>`) is consumed with that value.
    let mut shielded = String::with_capacity(input.len());
    let mut uuids = Vec::new();
    let mut last = 0;
    for found in uuid_pattern().find_iter(input) {
        let before = input[..found.start()].chars().next_back();
        let after = input[found.end()..].chars().next();
        if before.is_some_and(|c| c.is_ascii_alphanumeric())
            || after.is_some_and(|c| c.is_ascii_alphanumeric())
        {
            continue;
        }
        shielded.push_str(&input[last..found.start()]);
        shielded.push_str(&format!("\u{E000}{}\u{E001}", uuids.len()));
        uuids.push(found.as_str());
        last = found.end();
    }
    shielded.push_str(&input[last..]);
    let redacted = patterns().iter().fold(shielded, |text, (pattern, label)| {
        pattern
            .replace_all(&text, format!("[REDACTED_{label}]"))
            .into_owned()
    });
    uuids
        .iter()
        .enumerate()
        .fold(redacted, |text, (index, uuid)| {
            text.replace(&format!("\u{E000}{index}\u{E001}"), uuid)
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
    // A JSON document inside text is user content, so its numbers are redacted too.
    match serde_json::from_str::<Value>(input) {
        Ok(value) if value.is_object() || value.is_array() => {
            sanitize_tree(&value, true).to_string()
        }
        _ => redact_raw(input),
    }
}
/// Sanitizes structured values. Numeric fields are kept: in structured output and metadata
/// they are schema values (timestamps, counts), while numbers inside text documents are
/// handled as content by `sanitize_text`.
pub fn sanitize_value(input: &Value) -> Value {
    sanitize_tree(input, false)
}
fn sanitize_tree(input: &Value, content: bool) -> Value {
    match input {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let safe = match key.as_str() {
                        "source_revision" | "commitSha" | "commit_sha" => match value {
                            Value::String(s) => Value::String(sanitize_revision(s)),
                            _ => sanitize_tree(value, content),
                        },
                        "source_revisions" => match value {
                            Value::Array(values) => Value::Array(
                                values
                                    .iter()
                                    .map(|v| match v {
                                        Value::String(s) => Value::String(sanitize_revision(s)),
                                        _ => sanitize_tree(v, content),
                                    })
                                    .collect(),
                            ),
                            _ => sanitize_tree(value, content),
                        },
                        _ => match sensitive_label(key) {
                            Some(label) => Value::String(format!("[REDACTED_{label}]")),
                            None => sanitize_tree(value, content),
                        },
                    };
                    (redact_raw(key), safe)
                })
                .collect(),
        ),
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| sanitize_tree(v, content)).collect())
        }
        Value::String(text) => Value::String(sanitize_text(text)),
        Value::Number(number) if content => {
            let text = number.to_string();
            let safe = redact_raw(&text);
            if safe == text {
                input.clone()
            } else {
                Value::String(safe)
            }
        }
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
        Ok(value) if value.is_object() || value.is_array() => sanitize_tree(&value, true) != value,
        _ => redact_raw(input) != input,
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
