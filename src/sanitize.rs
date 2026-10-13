//! Shared structured and textual secret/PII redaction boundary.
use anyhow::{bail, Result};
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;
fn patterns() -> &'static Vec<(Regex, &'static str)> {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    PATTERNS.get_or_init(|| [
        (r"(?is)-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|\z)","PRIVATE_KEY"),
        (r"(?i)\bsk-[a-z0-9_-]{20,}\b","API_KEY"),
        (r"(?i)\bgh[pousr]_[a-z0-9]{36,}\b","GITHUB_TOKEN"),
        (r"(?i)\bgithub_pat_[a-z0-9_]{20,}\b","GITHUB_TOKEN"),
        (r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b","AWS_KEY"),
        (r"\bgl(?:pat|dt|rt|ptt|cbt|oas)-[A-Za-z0-9_-]{20,}","GITLAB_TOKEN"),
        (r"\bxox[abposr]-[A-Za-z0-9-]{10,}","SLACK_TOKEN"),
        (r"\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}|\bwhsec_[A-Za-z0-9]{24,}","STRIPE_KEY"),
        (r"\bnpm_[A-Za-z0-9]{30,}","NPM_TOKEN"),
        (r"\bAIza[0-9A-Za-z_-]{30,}","GOOGLE_API_KEY"),
        (r"\beyJ[A-Za-z0-9_-]{5,}\.eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]*","JWT"),
        (r"\b[A-Za-z0-9/+=]{40,}\b","TOKEN"),
        (r"(?i)\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[a-z]{2,}\b","EMAIL"),
        (r"\b\d{3}-\d{2}-\d{4}\b","SSN"),
        (r"\b\d{3}-\d{3}-\d{4}\b","PHONE"),
        (r"\b\d{10,12}\b","PHONE_INTL"),
    ].into_iter().map(|(p,label)|(Regex::new(p).expect("static redaction regex"),label)).collect())
}
/// A secret-looking key followed by `=` or `:` and a value: `password=…`, `DB_PASSWORD=…`,
/// `stripe.api_key: …`, `"pwd": …`. The whole key is matched, so a prefix like `DB_` no longer
/// hides it (`\b` never fires between `_` and a letter).
fn key_value() -> &'static Regex {
    static KEY_VALUE: OnceLock<Regex> = OnceLock::new();
    KEY_VALUE.get_or_init(|| {
        Regex::new(r#"(?i)(^|[^a-z0-9_.-])((?:[a-z0-9_.-]*(?:password|passwd|passphrase|secret|token|api[_.\s-]?key|access[_.-]?key|private[_.-]?key|credentials?)|(?:[a-z0-9_.-]*[_.-])?(?:pass|pwd)))(["']?\s*[:=]\s*)("[^"]*"|'[^']*'|<[^>\n]{1,40}>|\S+)"#)
            .expect("static key-value regex")
    })
}
/// An `Authorization` header (any scheme) or a standalone bearer token.
fn authorization() -> &'static [Regex; 2] {
    static AUTHORIZATION: OnceLock<[Regex; 2]> = OnceLock::new();
    AUTHORIZATION.get_or_init(|| {
        [
            Regex::new(r#"(?i)\b((?:proxy-)?authorization["']?\s*[:=]\s*)(?:(?:bearer|basic|token|digest|bot)\s+)?("[^"]*"|'[^']*'|\S+)"#),
            Regex::new(r"(?i)\b(bearer\s+)([A-Za-z0-9._~+/-]+=*)"),
        ]
        .map(|r| r.expect("static authorization regex"))
    })
}
fn url_pattern() -> &'static Regex {
    static URL: OnceLock<Regex> = OnceLock::new();
    URL.get_or_init(|| Regex::new(r"(?i)\b([a-z][a-z0-9+.-]*://)(\S+)").expect("static url regex"))
}
/// Values that only stand in for a secret: `${DB_PASSWORD}`, `<your key>`, `true`, `***`, `4096`.
fn placeholder(value: &str) -> bool {
    let v = value.trim_matches(|c| c == '"' || c == '\'');
    let lower = v.to_ascii_lowercase();
    v.is_empty()
        || matches!(
            lower.as_str(),
            "true" | "false" | "null" | "none" | "nil" | "empty"
        )
        || v.starts_with("${")
        || (v.len() > 1
            && v.starts_with('$')
            && v[1..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'))
        || (v.starts_with('<') && v.ends_with('>'))
        || v.starts_with("[REDACTED_")
        || v.chars().all(|c| c == '*')
        || (v.len() <= 4 && v.chars().all(|c| c.is_ascii_digit()))
}
/// `user:password@` in a URL. The password may contain `@` or `/`, so the host starts after the
/// last `@` that is followed by something host-like; `host:8080/path` is a port, not a password.
fn redact_url_credentials(scheme: &str, rest: &str) -> Option<String> {
    let colon = rest.find(':')?;
    let user = &rest[..colon];
    if user.is_empty() || user.contains(['/', '@']) {
        return None;
    }
    let after = &rest[colon + 1..];
    if after
        .split('/')
        .next()
        .is_some_and(|port| !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()))
        && after.contains('/')
    {
        return None;
    }
    let at = rest
        .char_indices()
        .rev()
        .filter(|(_, c)| *c == '@')
        .map(|(i, _)| i)
        .find(|&i| {
            i > colon
                && rest[i + 1..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '[')
        })?;
    Some(format!("{scheme}[REDACTED_CREDENTIAL]@{}", &rest[at + 1..]))
}
fn redact_contextual(text: &str) -> String {
    let text = key_value().replace_all(text, |c: &regex::Captures<'_>| {
        let (before, key, separator, value) = (&c[1], &c[2], &c[3], &c[4]);
        let quoted = value.starts_with(['"', '\'']);
        // "Session token: rotate weekly" is prose. A plain word, a colon and an ordinary word
        // need a digit, a symbol or some length before they read as a credential.
        let prose = separator.contains(':')
            && !key.contains(['_', '.', '-'])
            && key.chars().any(|ch| ch.is_ascii_lowercase())
            && !quoted
            && value.chars().count() < 12
            && value.chars().all(|ch| ch.is_alphabetic());
        if placeholder(value) || prose {
            return c[0].to_string();
        }
        let label = sensitive_label(key).unwrap_or(
            if key.to_ascii_lowercase().contains("pass")
                || key.to_ascii_lowercase().ends_with("pwd")
            {
                "PASSWORD"
            } else {
                "SECRET"
            },
        );
        format!("{before}{key}{separator}[REDACTED_{label}]")
    });
    let [header, bearer] = authorization();
    let text = header.replace_all(&text, |c: &regex::Captures<'_>| {
        if placeholder(&c[2]) {
            c[0].to_string()
        } else {
            format!("{}[REDACTED_TOKEN]", &c[1])
        }
    });
    let text = bearer.replace_all(&text, |c: &regex::Captures<'_>| {
        let value = &c[2];
        let long =
            value.len() >= 16 || (value.len() >= 8 && value.chars().any(|ch| ch.is_ascii_digit()));
        if long && !placeholder(value) {
            format!("{}[REDACTED_TOKEN]", &c[1])
        } else {
            c[0].to_string()
        }
    });
    url_pattern()
        .replace_all(&text, |c: &regex::Captures<'_>| {
            redact_url_credentials(&c[1], &c[2]).unwrap_or_else(|| c[0].to_string())
        })
        .into_owned()
}
/// Payment cards (Visa, Mastercard, Amex, Discover prefixes, Luhn-checked, spaces or dashes
/// allowed), phone numbers in `(555) 123-4567` and `+66 81 234 5678` form, and Thai national ids.
fn personal_data() -> &'static [Regex; 4] {
    static PERSONAL: OnceLock<[Regex; 4]> = OnceLock::new();
    PERSONAL.get_or_init(|| {
        [
            Regex::new(r"\b(?:4|5[1-5]|3[47]|6011)(?:[ -]?\d){11,15}\b"),
            Regex::new(r"\(\d{3}\)\s?\d{3}[-.\s]\d{4}\b"),
            Regex::new(r"\+\d{1,3}(?:[\s.-]?\(?\d{1,4}\)?){2,5}\b"),
            Regex::new(r"\b\d-\d{4}-\d{5}-\d{2}-\d\b"),
        ]
        .map(|r| r.expect("static personal-data regex"))
    })
}
fn luhn(digits: &[u32]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 1 {
                if d * 2 > 9 {
                    d * 2 - 9
                } else {
                    d * 2
                }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}
fn redact_personal(text: &str) -> String {
    let [card, phone, international, national_id] = personal_data();
    let text = card.replace_all(text, |c: &regex::Captures<'_>| {
        let digits: Vec<u32> = c[0].chars().filter_map(|ch| ch.to_digit(10)).collect();
        if (15..=16).contains(&digits.len()) && luhn(&digits) {
            "[REDACTED_CARD]".to_string()
        } else {
            c[0].to_string()
        }
    });
    let text = phone.replace_all(&text, "[REDACTED_PHONE]");
    let text = international.replace_all(&text, |c: &regex::Captures<'_>| {
        let digits = c[0].chars().filter(char::is_ascii_digit).count();
        if (8..=15).contains(&digits) {
            "[REDACTED_PHONE_INTL]".to_string()
        } else {
            c[0].to_string()
        }
    });
    national_id
        .replace_all(&text, "[REDACTED_NATIONAL_ID]")
        .into_owned()
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
    let shielded = redact_personal(&redact_contextual(&shielded));
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
    if key.ends_with("password")
        || key.ends_with("passwd")
        || key.ends_with("passphrase")
        || key == "pass"
        || key.ends_with("pwd")
    {
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
    } else if key.ends_with("secret")
        || key == "secrets"
        || key == "awssecretaccesskey"
        || key == "secretkeybase"
    {
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
/// The kinds of secret or PII `sanitize_text` removes from `input`, e.g. `["PASSWORD"]`, so a
/// write can tell the user what it changed.
pub fn redactions(input: &str) -> Vec<String> {
    static LABEL: OnceLock<Regex> = OnceLock::new();
    let label = LABEL.get_or_init(|| Regex::new(r"\[REDACTED_([A-Z_]+)\]").expect("static regex"));
    let count = |text: &str| {
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        for c in label.captures_iter(text) {
            *counts.entry(c[1].to_string()).or_default() += 1;
        }
        counts
    };
    let before = count(input);
    count(&sanitize_text(input))
        .into_iter()
        .filter(|(kind, n)| before.get(kind).is_none_or(|b| n > b))
        .map(|(kind, _)| kind)
        .collect()
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
