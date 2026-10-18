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
    // Every read re-sanitizes stored text, so skip each rule cheaply when the text can't
    // contain what it looks for.
    let has_separator = text.contains([':', '=']);
    let text: std::borrow::Cow<'_, str> = if !has_separator {
        text.into()
    } else {
        key_value().replace_all(text, |c: &regex::Captures<'_>| {
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
        })
    };
    let [header, bearer] = authorization();
    let text: std::borrow::Cow<'_, str> =
        if has_separator && (text.contains("uthorization") || text.contains("UTHORIZATION")) {
            header
                .replace_all(&text, |c: &regex::Captures<'_>| {
                    if placeholder(&c[2]) {
                        c[0].to_string()
                    } else {
                        format!("{}[REDACTED_TOKEN]", &c[1])
                    }
                })
                .into_owned()
                .into()
        } else {
            text
        };
    if !(text.contains("earer") || text.contains("EARER") || text.contains("://")) {
        return text.into_owned();
    }
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
/// allowed), phone numbers in `(555) 123-4567` and `+66 81 234 5678` form, and Thai national ids
/// (see `redact_national_ids`).
fn personal_data() -> &'static [Regex; 4] {
    static PERSONAL: OnceLock<[Regex; 4]> = OnceLock::new();
    PERSONAL.get_or_init(|| {
        [
            Regex::new(r"\b(?:4|5[1-5]|3[47]|6011)(?:[ -]?\d){11,15}\b"),
            Regex::new(r"\(\d{3}\)\s?\d{3}[-.\s]\d{4}\b"),
            Regex::new(r"\+\d{1,3}(?:[\s.-]?\(?\d{1,4}\)?){2,5}\b"),
            Regex::new(r"[0-9][ -][0-9]{4}[ -][0-9]{5}[ -][0-9]{2}[ -][0-9]|[0-9]{13,}"),
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
/// The check digit of a Thai national id: the first twelve digits weighted 13 down to 2.
fn thai_id(digits: &[u32]) -> bool {
    if digits.len() != 13 || !(1..=8).contains(&digits[0]) {
        return false;
    }
    let sum: u32 = digits[..12]
        .iter()
        .zip((2..=13).rev())
        .map(|(d, w)| d * w)
        .sum();
    (11 - sum % 11) % 10 == digits[12]
}
/// Thai national ids written `1-2345-67890-12-3` or `1 2345 67890 12 3`, or as 13 digits in a row
/// when the check digit is right, which rules out nine in ten other 13-digit numbers. Thai text may
/// touch an id; letters, digits and `_-./` may not, so names, versions and paths keep their digits.
fn redact_national_ids(pattern: &Regex, text: &str) -> String {
    let joined = |c: char| c.is_ascii_alphanumeric() || "_-./".contains(c);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for found in pattern.find_iter(text) {
        if text[..found.start()]
            .chars()
            .next_back()
            .is_some_and(joined)
            || text[found.end()..].chars().next().is_some_and(joined)
        {
            continue;
        }
        let digits: Vec<u32> = found
            .as_str()
            .chars()
            .filter_map(|c| c.to_digit(10))
            .collect();
        let separated = found.as_str().contains([' ', '-']);
        if !separated && !thai_id(&digits) {
            continue;
        }
        out.push_str(&text[last..found.start()]);
        out.push_str("[REDACTED_NATIONAL_ID]");
        last = found.end();
    }
    out.push_str(&text[last..]);
    out
}
fn redact_personal(text: &str) -> String {
    let [card, phone, international, national_id] = personal_data();
    if text.bytes().filter(u8::is_ascii_digit).count() < 8 {
        return text.to_string();
    }
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
    redact_national_ids(national_id, &text)
}
/// A bare run of 10 to 12 digits is a phone number when it starts with 0 (0812345678) or a
/// phone word comes just before it ("call 5551234567", "โทร 0812345678"), and never right after
/// a word naming an id ("run 39057490023", "order ref 1234567890", "#1234567890"): without
/// context it is as likely a CI run, an order or a timestamp.
fn redact_bare_phones(text: &str) -> String {
    static BARE: OnceLock<[Regex; 3]> = OnceLock::new();
    let [bare, phone_word, id_word] = BARE.get_or_init(|| {
        [
            Regex::new(r"\b\d{10,12}\b"),
            Regex::new(
                r"(?i)(?:\b(?:phone|tel|telephone|call|mobile|cell|fax|whatsapp|sms|contact)(?:s|ed|ing)?\b|โทร|เบอร์|มือถือ|ติดต่อ)[^\n\d]{0,20}$",
            ),
            Regex::new(
                r"(?i)(?:\b(?:run|id|ref|order|pr|issue|build|job|ticket|invoice|pipeline|workflow|commit)s?|#)[\s:=#.-]*$",
            ),
        ]
        .map(|r| r.expect("static phone regex"))
    });
    bare.replace_all(text, |c: &regex::Captures<'_>| {
        let found = c.get(0).expect("whole match");
        let before = &text[..found.start()];
        let window = &before[before.char_indices().rev().nth(39).map_or(0, |(at, _)| at)..];
        let phone = !id_word.is_match(window)
            && (found.as_str().starts_with('0') || phone_word.is_match(window));
        if phone {
            "[REDACTED_PHONE_INTL]".to_string()
        } else {
            found.as_str().to_string()
        }
    })
    .into_owned()
}
/// All fixed patterns at once: one scan finds which ones can match, so the rest are skipped.
/// Every read re-sanitizes stored text, which made a pass per pattern the cost of a brief.
fn pattern_set() -> &'static regex::RegexSet {
    static SET: OnceLock<regex::RegexSet> = OnceLock::new();
    SET.get_or_init(|| {
        regex::RegexSet::new(patterns().iter().map(|(pattern, _)| pattern.as_str()))
            .expect("static redaction regex set")
    })
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
    let candidates = pattern_set().matches(&shielded);
    let redacted = patterns()
        .iter()
        .enumerate()
        .filter(|(index, _)| candidates.matched(*index))
        .map(|(_, rule)| rule)
        .fold(shielded, |text, (pattern, label)| {
            pattern
                .replace_all(&text, format!("[REDACTED_{label}]"))
                .into_owned()
        });
    // Last, so tokens made of digit groups (xoxb-000000000001-...) are redacted whole first.
    let redacted = redact_bare_phones(&redacted);
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
                        _ => match (sensitive_label(key), value) {
                            (Some(label), _) => Value::String(format!("[REDACTED_{label}]")),
                            (None, Value::Number(number)) if content => {
                                sanitize_number(number, &key_words(key))
                            }
                            (None, _) => sanitize_tree(value, content),
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
        Value::Number(number) if content => sanitize_number(number, ""),
        other => other.clone(),
    }
}
/// A number in JSON content, read with its key as context ("phone": 5551234567).
fn sanitize_number(number: &serde_json::Number, key: &str) -> Value {
    let text = number.to_string();
    let safe = redact_raw(&text);
    if safe != text {
        return Value::String(safe);
    }
    let in_context = format!("{key}: {text}");
    if redact_bare_phones(&in_context) == in_context {
        Value::Number(number.clone())
    } else {
        Value::String("[REDACTED_PHONE_INTL]".to_string())
    }
}
/// A JSON key as words: `run_id`, `run-id` and `runId` read as "run id".
fn key_words(key: &str) -> String {
    let mut words = String::new();
    let mut previous: Option<char> = None;
    for c in key.chars() {
        if matches!(c, '_' | '-' | '.') {
            words.push(' ');
        } else {
            if c.is_uppercase() && previous.is_some_and(char::is_lowercase) {
                words.push(' ');
            }
            words.push(c);
        }
        previous = Some(c);
    }
    words
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
