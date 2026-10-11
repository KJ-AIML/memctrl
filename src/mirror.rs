//! Markdown mirror of shared memory: exported for review in pull requests, imported on other
//! machines. The text stays readable; a metadata comment after each entry carries identity,
//! states, citations and a content hash, so hand edits are detected rather than trusted.
use crate::citations::{digest, parse_spec, Anchor};
use crate::guard::{self, Policy};
use crate::model::{ClaimType, LifecycleState, Memory, NewMemory, VerificationState};
use crate::sanitize::{sanitize_text, validate_identifier};
use crate::store::{AtomicWrite, MemoryStore};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

pub const DEFAULT_PATH: &str = ".memctrl/memory.md";
pub const DEFAULT_LAYERS: [&str; 1] = ["project"];
const HEADER: &str = "<!-- memctrl export v1. Review changes like code. Edit entry text freely \
(edited entries return as candidates on import) and add new `- ` bullets for teammates to review; \
keep the memctrl comments intact. -->";
const HEADER_PREFIX: &str = "<!-- memctrl export v1";
const META_PREFIX: &str = "<!-- memctrl {";
const META_SUFFIX: &str = "} -->";
const GITIGNORE: &str =
    "# memctrl: the database stays local; memory.md is meant to be committed.\n\
*.db\n*.db-wal\n*.db-shm\n*.db-journal\n";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct CiteMeta {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lines: Option<String>,
    file_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    span_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    head_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Meta {
    id: String,
    layer: String,
    claim_type: ClaimType,
    verification_state: VerificationState,
    confidence: f64,
    source: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    observed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    valid_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    valid_until: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    cite: Vec<CiteMeta>,
    /// Digest of the entry text as exported.
    sha: String,
}

/// Export order, which is also the section a hand-written bullet is filed under.
fn section(memory: &Memory) -> (u8, &'static str) {
    if memory.verification_state == VerificationState::Disputed {
        return (4, "Disputed");
    }
    match memory.claim_type {
        ClaimType::Decision => (0, "Decisions"),
        ClaimType::Assertion | ClaimType::Observation => (1, "Facts and conventions"),
        ClaimType::DerivedLesson => (2, "Lessons"),
        ClaimType::Hypothesis => (3, "Open hypotheses"),
    }
}

fn claim_for_heading(heading: &str) -> ClaimType {
    let heading = heading.to_lowercase();
    if heading.contains("decision") {
        ClaimType::Decision
    } else if heading.contains("hypothes") {
        ClaimType::Hypothesis
    } else {
        ClaimType::Assertion
    }
}

fn escape(line: &str) -> String {
    if line.starts_with('\\') || line.starts_with("<!--") {
        format!("\\{line}")
    } else {
        line.to_string()
    }
}

fn unescape(line: &str) -> String {
    line.strip_prefix('\\').unwrap_or(line).to_string()
}

fn lines_label(start: Option<u32>, end: Option<u32>) -> Option<String> {
    match (start, end) {
        (Some(a), Some(b)) if a == b => Some(a.to_string()),
        (Some(a), Some(b)) => Some(format!("{a}-{b}")),
        _ => None,
    }
}

/// Renders the export. Deterministic for a given store: no timestamps, no access counts.
pub fn render(store: &MemoryStore, layers: &[String]) -> Result<(String, usize)> {
    let mut memories = Vec::new();
    for layer in layers {
        validate_identifier(layer, "layer")?;
        memories.extend(store.list_memories(Some(layer), false)?);
    }
    memories.sort_by(|a, b| {
        section(a)
            .0
            .cmp(&section(b).0)
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut out = format!(
        "{HEADER}\n# Project memory\n\n{} memories from {}. Import into a local store with \
         `memctrl import <this file>`.\n",
        memories.len(),
        layers.join(", ")
    );
    let mut current = None;
    for memory in &memories {
        let (rank, heading) = section(memory);
        if current != Some(rank) {
            out.push_str(&format!("\n## {heading}\n\n"));
            current = Some(rank);
        }
        let content = memory.content.trim_end();
        for (index, line) in content.split('\n').enumerate() {
            let line = escape(line.trim_end());
            if index == 0 {
                out.push_str(&format!("- {line}\n"));
            } else if line.is_empty() {
                out.push('\n');
            } else {
                out.push_str(&format!("  {line}\n"));
            }
        }
        let cite = store
            .citations(&memory.id)?
            .into_iter()
            .map(|c| CiteMeta {
                lines: lines_label(c.line_start, c.line_end),
                path: c.path,
                file_hash: c.file_hash,
                span_hash: c.span_hash,
                head_hash: c.head_hash,
            })
            .collect();
        let meta = Meta {
            id: memory.id.clone(),
            layer: memory.layer.clone(),
            claim_type: memory.claim_type,
            verification_state: memory.verification_state,
            confidence: (memory.confidence * 100.0).round() / 100.0,
            source: memory.source.clone(),
            tags: memory.tags.clone(),
            created_at: memory.created_at.clone(),
            expires_at: memory.expires_at.clone(),
            observed_at: memory.observed_at.clone(),
            valid_from: memory.valid_from.clone(),
            valid_until: memory.valid_until.clone(),
            cite,
            sha: digest(&canonical(content)),
        };
        out.push_str(&format!(
            "  <!-- memctrl {} -->\n",
            serde_json::to_string(&meta)?
        ));
    }
    Ok((out, memories.len()))
}

/// The text form hashed for edit detection: trailing whitespace per line removed.
fn canonical(content: &str) -> String {
    content
        .trim_end()
        .split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Ensures a `.gitignore` next to an export inside `.memctrl/`, so committing the folder never
/// commits the database.
pub fn ensure_gitignore(export: &std::path::Path) -> Result<()> {
    let Some(dir) = export.parent() else {
        return Ok(());
    };
    if dir.file_name().is_some_and(|n| n == ".memctrl") {
        let ignore = dir.join(".gitignore");
        if !ignore.exists() {
            std::fs::create_dir_all(dir)?;
            std::fs::write(ignore, GITIGNORE)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    line: usize,
    content: String,
    heading: String,
    meta: Option<Meta>,
}

fn parse(text: &str) -> Result<Vec<Entry>> {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    ensure!(
        first.starts_with(HEADER_PREFIX),
        "not a memctrl export (expected a first line starting with {HEADER_PREFIX:?})"
    );
    let mut entries = Vec::new();
    let mut heading = String::new();
    let mut open: Option<Entry> = None;
    let finish = |entry: Entry, entries: &mut Vec<Entry>| {
        let content = entry.content.trim_end().to_string();
        if !content.trim().is_empty() {
            entries.push(Entry { content, ..entry });
        }
    };
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let trimmed = line.trim();
        if let Some(entry) = open.as_mut() {
            if trimmed.starts_with(META_PREFIX) && trimmed.ends_with(META_SUFFIX) {
                let json = &trimmed["<!-- memctrl ".len()..trimmed.len() - " -->".len()];
                let meta: Meta = serde_json::from_str(json)
                    .with_context(|| format!("line {number}: invalid memctrl metadata"))?;
                entry.meta = Some(meta);
                finish(open.take().expect("entry is open"), &mut entries);
                continue;
            }
            if line.is_empty() {
                entry.content.push('\n');
                continue;
            }
            if let Some(rest) = line.strip_prefix("  ") {
                entry.content.push('\n');
                entry.content.push_str(&unescape(rest));
                continue;
            }
            // Anything else ends a hand-written entry, which has no metadata.
            finish(open.take().expect("entry is open"), &mut entries);
        }
        if let Some(rest) = line.strip_prefix("- ") {
            open = Some(Entry {
                line: number,
                content: unescape(rest),
                heading: heading.clone(),
                meta: None,
            });
        } else if let Some(title) = line.strip_prefix("## ") {
            heading = title.trim().to_string();
        } else if trimmed.starts_with(META_PREFIX) {
            bail!("line {number}: memctrl metadata without an entry above it");
        }
    }
    if let Some(entry) = open {
        finish(entry, &mut entries);
    }
    Ok(entries)
}

fn anchor_from(cite: &CiteMeta) -> Result<Anchor> {
    let spec = match &cite.lines {
        Some(lines) => format!("{}:{lines}", cite.path),
        None => cite.path.clone(),
    };
    let parsed = parse_spec(&spec)?;
    let hex = |h: &str| h.len() <= 64 && !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit());
    ensure!(hex(&cite.file_hash), "citation file_hash must be hex");
    for hash in [&cite.span_hash, &cite.head_hash].into_iter().flatten() {
        ensure!(hex(hash), "citation hashes must be hex");
    }
    ensure!(
        parsed.line_start.is_some() == cite.span_hash.is_some(),
        "line-range citations need a span_hash"
    );
    Ok(Anchor {
        path: parsed.path,
        line_start: parsed.line_start,
        line_end: parsed.line_end,
        file_hash: cite.file_hash.clone(),
        span_hash: cite.span_hash.clone(),
        head_hash: cite.head_hash.clone(),
    })
}

enum Action {
    Keep {
        id: String,
        created_at: String,
        memory: NewMemory,
        anchors: Vec<Anchor>,
    },
    Candidate {
        memory: NewMemory,
        anchors: Vec<Anchor>,
    },
    Unchanged,
    KeptLocal,
}

/// Imports an export. Atomic, and never overwrites local state:
/// - an unknown id with untouched text is inserted with its original identity and states;
/// - a known id is left alone (reported as unchanged or kept_local);
/// - edited text and hand-written bullets become new candidates for review;
/// - every entry passes the write guard, and text already stored anywhere is skipped.
pub fn import(
    store: &MemoryStore,
    text: &str,
    default_layer: &str,
    policy: &Policy,
    dry_run: bool,
) -> Result<Value> {
    validate_identifier(default_layer, "layer")?;
    let entries = parse(text)?;
    let mut known: HashSet<String> = store
        .list_memories(None, true)?
        .into_iter()
        .map(|m| m.content)
        .collect();
    let mut plan = Vec::new();
    for entry in &entries {
        let content = sanitize_text(&entry.content);
        let context = |e: anyhow::Error| e.context(format!("entry at line {}", entry.line));
        let action = match &entry.meta {
            Some(meta) => {
                uuid::Uuid::parse_str(&meta.id)
                    .context("memory id must be a UUID")
                    .map_err(context)?;
                validate_identifier(&meta.layer, "layer").map_err(context)?;
                validate_identifier(&meta.source, "source").map_err(context)?;
                let anchors = meta
                    .cite
                    .iter()
                    .map(anchor_from)
                    .collect::<Result<Vec<_>>>()
                    .map_err(context)?;
                let edited = digest(&canonical(&entry.content)) != meta.sha;
                let base = NewMemory {
                    layer: meta.layer.clone(),
                    content: content.clone(),
                    source: meta.source.clone(),
                    confidence: meta.confidence,
                    tags: meta.tags.clone(),
                    expires_at: meta.expires_at.clone(),
                    observed_at: meta.observed_at.clone(),
                    valid_from: meta.valid_from.clone(),
                    valid_until: meta.valid_until.clone(),
                    claim_type: meta.claim_type,
                    lifecycle_state: LifecycleState::Accepted,
                    verification_state: meta.verification_state,
                };
                let local = store.get_memory(&meta.id, true)?;
                if edited {
                    if known.contains(&content) {
                        Action::Unchanged
                    } else {
                        let mut memory = base;
                        memory.lifecycle_state = LifecycleState::Candidate;
                        memory.verification_state = VerificationState::Unverified;
                        memory.tags.push("import:edited".into());
                        Action::Candidate { memory, anchors }
                    }
                } else if let Some(local) = local {
                    if local.content == content {
                        Action::Unchanged
                    } else {
                        Action::KeptLocal
                    }
                } else {
                    Action::Keep {
                        id: meta.id.clone(),
                        created_at: meta.created_at.clone(),
                        memory: base,
                        anchors,
                    }
                }
            }
            None if known.contains(&content) => Action::Unchanged,
            None => Action::Candidate {
                memory: NewMemory {
                    layer: default_layer.to_string(),
                    content: content.clone(),
                    source: "import".into(),
                    claim_type: claim_for_heading(&entry.heading),
                    lifecycle_state: LifecycleState::Candidate,
                    tags: vec!["import:new".into()],
                    ..Default::default()
                },
                anchors: Vec::new(),
            },
        };
        if let Action::Keep { memory, .. } | Action::Candidate { memory, .. } = &action {
            memory.validate().map_err(context)?;
            known.insert(memory.content.clone());
        }
        plan.push(action);
    }
    let guard_write = if dry_run {
        None
    } else {
        Some(AtomicWrite::new(&store.conn)?)
    };
    let (mut imported, mut candidates, mut quarantined) = (Vec::new(), Vec::new(), 0);
    let (mut unchanged, mut kept_local) = (0, 0);
    for action in plan {
        match action {
            Action::Unchanged => unchanged += 1,
            Action::KeptLocal => kept_local += 1,
            Action::Keep {
                id,
                created_at,
                mut memory,
                anchors,
            } => {
                let verdict = guard::inspect(&memory.content, "import", policy);
                if verdict.quarantined() {
                    guard::quarantine(&mut memory, &verdict);
                    quarantined += 1;
                }
                if !dry_run {
                    store.insert_memory_as(memory.clone(), &id, Some(&created_at))?;
                    for anchor in &anchors {
                        store.add_citation(&id, anchor, None)?;
                    }
                }
                if memory.lifecycle_state == LifecycleState::Candidate {
                    candidates.push(id);
                } else {
                    imported.push(id);
                }
            }
            Action::Candidate {
                mut memory,
                anchors,
            } => {
                let verdict = guard::inspect(&memory.content, "import", policy);
                if verdict.quarantined() {
                    guard::quarantine(&mut memory, &verdict);
                    quarantined += 1;
                }
                let id = if dry_run {
                    "(dry run)".to_string()
                } else {
                    store.insert_memory_with_citations(memory, &anchors, None)?
                };
                candidates.push(id);
            }
        }
    }
    if let Some(guard_write) = guard_write {
        guard_write.commit()?;
    }
    Ok(json!({
        "entries": entries.len(),
        "imported": imported,
        "candidates": candidates,
        "unchanged": unchanged,
        "kept_local": kept_local,
        "quarantined": quarantined,
        "dry_run": dry_run,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_lines_that_look_like_markup_survive_a_round_trip() {
        for content in [
            "plain",
            "<!-- memctrl {\"id\":\"x\"} -->",
            "\\leading backslash",
            "first\n\nsecond paragraph\n- not a bullet",
            "two\n\n\nblank lines",
        ] {
            let mut text = format!("{HEADER}\n");
            for (i, line) in content.split('\n').enumerate() {
                let line = escape(line);
                if i == 0 {
                    text.push_str(&format!("- {line}\n"));
                } else if line.is_empty() {
                    text.push('\n');
                } else {
                    text.push_str(&format!("  {line}\n"));
                }
            }
            let entries = parse(&text).unwrap();
            assert_eq!(entries.len(), 1, "{content:?}");
            assert_eq!(entries[0].content, content);
        }
    }

    #[test]
    fn parse_rejects_foreign_files_and_orphan_metadata() {
        assert!(parse("# Notes\n- a\n").is_err());
        assert!(parse(&format!("{HEADER}\n<!-- memctrl {{\"id\":1}} -->\n")).is_err());
        let hand = parse(&format!(
            "{HEADER}\n## Decisions\n- We use Bun\n\nprose\n- Another\n"
        ))
        .unwrap();
        assert_eq!(hand.len(), 2);
        assert_eq!(hand[0].heading, "Decisions");
        assert!(hand.iter().all(|e| e.meta.is_none()));
    }
}
