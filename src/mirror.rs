//! Markdown mirror of shared memory: exported for review in pull requests, imported on other
//! machines. The text stays readable; a metadata comment after each entry carries identity,
//! states, citations and a content hash, so hand edits are detected rather than trusted.
//!
//! Import never raises trust on its own: anyone who can edit the file can write an entry and its
//! metadata, so new entries arrive as candidates until the person importing accepts them
//! (`accept`) after reading the changes. Verification is capped at `unverified` unless the
//! caller opts in, forgotten memories stay forgotten, text the local store already holds (in any
//! state, refuted included) is never added again, an id written with different texts is held for
//! review, and everything passes the write guard.
use crate::citations::{parse_spec, text_digest, Anchor};
use crate::guard::{self, Policy};
use crate::model::{ClaimType, LifecycleState, Memory, NewMemory, VerificationState};
use crate::sanitize::{sanitize_text, validate_identifier};
use crate::store::{AtomicWrite, MemoryStore};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub const DEFAULT_PATH: &str = ".memctrl/memory.md";
pub const DEFAULT_LAYERS: [&str; 1] = ["project"];
const HEADER: &str = "<!-- memctrl export v1. Review changes like code. Edit entry text freely \
(edited entries return as candidates on import) and add new `- ` bullets for teammates to review; \
keep the memctrl comments intact. -->";
const HEADER_PREFIX: &str = "<!-- memctrl export v";
/// Metadata comments sit at exactly two spaces of indentation.
const META_LINE: &str = "  <!-- memctrl {";
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    head_offset: Option<u32>,
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

/// Content lines that could be mistaken for markup (a backslash, or an HTML comment at any
/// indentation) get a leading backslash, which `unescape` removes.
fn escape(line: &str) -> String {
    if line.starts_with('\\') || line.trim_start().starts_with("<!--") {
        format!("\\{line}")
    } else {
        line.to_string()
    }
}

fn unescape(line: &str) -> String {
    line.strip_prefix('\\').unwrap_or(line).to_string()
}

/// Strips a byte-order mark and converts CRLF line endings, as editors and `core.autocrlf`
/// checkouts produce them.
pub fn normalize(text: &str) -> String {
    text.strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n")
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
                head_offset: c.head_offset,
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
            sha: text_digest(content),
        };
        out.push_str(&format!(
            "  <!-- memctrl {} -->\n",
            serde_json::to_string(&meta)?
        ));
    }
    Ok((out, memories.len()))
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
    let version = first
        .strip_prefix(HEADER_PREFIX)
        .map(|rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .and_then(|digits| digits.parse::<u32>().ok());
    match version {
        Some(1) => {}
        Some(newer) if newer > 1 => {
            bail!("written by a newer memctrl (export format v{newer}); upgrade memctrl to read it")
        }
        _ => bail!(
            "not a memctrl export (expected a first line starting with \"<!-- memctrl export v1\")"
        ),
    }
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
        if ["<<<<<<<", "|||||||", "=======", ">>>>>>>"]
            .iter()
            .any(|marker| line == *marker || line.starts_with(&format!("{marker} ")))
        {
            bail!("line {number}: unresolved merge conflict; keep both sides' entries, remove the markers, then import");
        }
        if let Some(entry) = open.as_mut() {
            if line.starts_with(META_LINE) && line.trim_end().ends_with(META_SUFFIX) {
                let trimmed = line.trim();
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
        } else if line.trim_start().starts_with(META_PREFIX) {
            bail!("line {number}: memctrl metadata without an entry above it");
        }
    }
    if let Some(entry) = open {
        finish(entry, &mut entries);
    }
    Ok(entries)
}

/// Citation digests are always 32 lowercase hex digits; anything else could never verify.
fn digest_like(value: &str) -> bool {
    value.len() == 32
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

fn anchor_from(cite: &CiteMeta) -> Result<Anchor> {
    let spec = match &cite.lines {
        Some(lines) => format!("{}:{lines}", cite.path),
        None => cite.path.clone(),
    };
    let parsed = parse_spec(&spec)?;
    ensure!(
        digest_like(&cite.file_hash),
        "citation file_hash must be 32 lowercase hex digits"
    );
    for hash in [&cite.span_hash, &cite.head_hash].into_iter().flatten() {
        ensure!(
            digest_like(hash),
            "citation hashes must be 32 lowercase hex digits"
        );
    }
    ensure!(
        parsed.line_start.is_some() == cite.span_hash.is_some(),
        "line-range citations need a span_hash"
    );
    if let (Some(offset), Some(start), Some(end)) =
        (cite.head_offset, parsed.line_start, parsed.line_end)
    {
        ensure!(
            offset <= end - start,
            "citation head_offset is outside its span"
        );
    }
    Ok(Anchor {
        path: parsed.path,
        line_start: parsed.line_start,
        line_end: parsed.line_end,
        file_hash: cite.file_hash.clone(),
        span_hash: cite.span_hash.clone(),
        head_hash: cite.head_hash.clone(),
        head_offset: cite.head_offset,
    })
}

/// How an import treats the file.
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// Layers to import, so a file can't plant memories in layers you didn't ask for. Entries in
    /// other layers are skipped; hand-written entries go to the first.
    pub layers: Vec<String>,
    pub dry_run: bool,
    /// Keep `supported` verification from the file. Off by default: anyone who can edit the
    /// file can write metadata, so imported claims arrive unverified.
    pub trust_verification: bool,
    /// The person importing has read the file's changes: new entries are accepted, and so are
    /// candidates an earlier import created from them. Without it, everything new arrives as a
    /// candidate. Edited and guard-flagged entries stay candidates either way.
    pub accept: bool,
}
impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            layers: vec!["project".into()],
            dry_run: false,
            trust_verification: false,
            accept: false,
        }
    }
}

/// Tag on candidates created from new entries, removed when they are accepted on import.
const NEW_TAG: &str = "import:new";

enum Skip {
    Forgotten,
    DuplicateId,
    DuplicateContent,
    Layer,
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
    /// An earlier import's candidate for this exact entry, accepted now.
    Promote {
        id: String,
        content: String,
    },
    Unchanged,
    KeptLocal,
    Skipped(Skip),
}

/// The canonical spelling of a memory id. Uppercase, braced, `urn:uuid:` and unhyphenated
/// spellings of one UUID are the same memory, not new ones.
fn canonical_id(raw: &str) -> Result<String> {
    Ok(uuid::Uuid::parse_str(raw)
        .context("memory id must be a UUID")?
        .hyphenated()
        .to_string())
}

/// `created_at` from the file, normalized as the store keeps it. A future time is clamped to
/// now: it would otherwise win every "newest first" tie in the brief.
fn import_time(raw: &str) -> Result<String> {
    let time = crate::model::parse_time(raw).context("invalid created_at")?;
    Ok(time
        .min(chrono::Utc::now())
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
}

/// Imports an export atomically; see the module docs for the trust rules.
pub fn import(
    store: &MemoryStore,
    text: &str,
    options: &ImportOptions,
    policy: &Policy,
) -> Result<Value> {
    import_with(store, text, options, policy, &mut |_| Ok(true))
}

/// `import`, asking `approve` before anything is accepted. With `accept`, it receives the text
/// of every entry the import would accept (after planning, before writing); returning false
/// cancels the import and changes nothing.
pub fn import_with(
    store: &MemoryStore,
    text: &str,
    options: &ImportOptions,
    policy: &Policy,
    approve: &mut dyn FnMut(&[String]) -> Result<bool>,
) -> Result<Value> {
    ensure!(
        !options.layers.is_empty(),
        "import needs at least one layer"
    );
    for layer in &options.layers {
        validate_identifier(layer, "layer")?;
    }
    let text = normalize(text);
    let entries = parse(&text)?;
    // Plan and write under the writer lock, so nothing changes between checking and writing. A
    // dry run takes the same path and rolls back, so it fails exactly when the import would.
    let lock = AtomicWrite::new(&store.conn)?;
    let memories = store.list_memories(None, true)?;
    let mut known: HashSet<String> = memories.iter().map(|m| m.content.clone()).collect();
    // Candidates an earlier import created from new entries, which `accept` promotes.
    let promotable = |memory: &Memory| {
        memory.lifecycle_state == LifecycleState::Candidate
            && memory.tags.iter().any(|t| t == NEW_TAG)
            && !memory.tags.iter().any(|t| t.starts_with("guard:"))
    };
    let mut pending: HashMap<String, String> = memories
        .iter()
        .filter(|m| promotable(m))
        .map(|m| (m.content.clone(), m.id.clone()))
        .collect();
    let (forgotten_ids, forgotten_text) = store.tombstones()?;
    // An id that appears with different texts can't be trusted in any copy: a forged duplicate
    // placed first would otherwise win.
    let mut texts_by_id: HashMap<String, HashSet<String>> = HashMap::new();
    for entry in &entries {
        if let Some(id) = entry.meta.as_ref().and_then(|m| canonical_id(&m.id).ok()) {
            texts_by_id
                .entry(id)
                .or_default()
                .insert(sanitize_text(&entry.content));
        }
    }
    let conflicted: HashSet<String> = texts_by_id
        .into_iter()
        .filter(|(_, texts)| texts.len() > 1)
        .map(|(id, _)| id)
        .collect();
    let new_state = if options.accept {
        LifecycleState::Accepted
    } else {
        LifecycleState::Candidate
    };
    let mut seen_ids = HashSet::new();
    let mut capped = 0;
    let mut plan = Vec::new();
    for entry in &entries {
        let content = sanitize_text(&entry.content);
        let digest = text_digest(&content);
        let blocked = known.contains(&content) || forgotten_text.contains(&digest);
        let context = |e: anyhow::Error| e.context(format!("entry at line {}", entry.line));
        let action = match &entry.meta {
            Some(meta) => {
                let id = canonical_id(&meta.id).map_err(context)?;
                validate_identifier(&meta.layer, "layer").map_err(context)?;
                validate_identifier(&meta.source, "source").map_err(context)?;
                let created_at = import_time(&meta.created_at).map_err(context)?;
                let anchors = meta
                    .cite
                    .iter()
                    .map(anchor_from)
                    .collect::<Result<Vec<_>>>()
                    .map_err(context)?;
                let edited = text_digest(&entry.content) != meta.sha;
                let mut verification = meta.verification_state;
                let base = |verification| NewMemory {
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
                    lifecycle_state: LifecycleState::Candidate,
                    verification_state: verification,
                };
                let held = |tag: &str| {
                    let mut memory = base(VerificationState::Unverified);
                    memory.tags.push(tag.into());
                    Action::Candidate {
                        memory,
                        anchors: anchors.clone(),
                    }
                };
                if !options.layers.contains(&meta.layer) {
                    Action::Skipped(Skip::Layer)
                } else if forgotten_ids.contains(&id) || forgotten_text.contains(&digest) {
                    Action::Skipped(Skip::Forgotten)
                } else if conflicted.contains(&id) {
                    if blocked {
                        Action::Unchanged
                    } else {
                        held("import:conflict")
                    }
                } else if !seen_ids.insert(id.clone()) {
                    Action::Skipped(Skip::DuplicateId)
                } else if edited {
                    if blocked {
                        Action::Unchanged
                    } else {
                        held("import:edited")
                    }
                } else if let Some(local) = store.get_memory(&id, true)? {
                    if local.content != content {
                        Action::KeptLocal
                    } else if options.accept && promotable(&local) {
                        pending.remove(&content);
                        Action::Promote {
                            id,
                            content: content.clone(),
                        }
                    } else {
                        Action::Unchanged
                    }
                } else if blocked {
                    // The same text is already stored under another id, possibly refuted.
                    Action::Skipped(Skip::DuplicateContent)
                } else {
                    if verification == VerificationState::Supported && !options.trust_verification {
                        verification = VerificationState::Unverified;
                        capped += 1;
                    }
                    let mut memory = base(verification);
                    memory.lifecycle_state = new_state;
                    if !options.accept {
                        memory.tags.push(NEW_TAG.into());
                    }
                    Action::Keep {
                        id,
                        created_at,
                        memory,
                        anchors,
                    }
                }
            }
            None if blocked => match options.accept.then(|| pending.remove(&content)) {
                Some(Some(id)) => Action::Promote {
                    id,
                    content: content.clone(),
                },
                _ => Action::Unchanged,
            },
            None => Action::Candidate {
                memory: NewMemory {
                    layer: options.layers[0].clone(),
                    content: content.clone(),
                    source: "import".into(),
                    claim_type: claim_for_heading(&entry.heading),
                    lifecycle_state: new_state,
                    tags: if options.accept {
                        Vec::new()
                    } else {
                        vec![NEW_TAG.into()]
                    },
                    ..Default::default()
                },
                anchors: Vec::new(),
            },
        };
        if let Action::Keep { memory, .. } | Action::Candidate { memory, .. } = &action {
            memory.validate().map_err(context)?;
            known.insert(memory.content.clone());
        }
        plan.push((entry.line, action));
    }
    if options.accept {
        let accepting: Vec<String> = plan
            .iter()
            .filter_map(|(_, action)| match action {
                Action::Keep { memory, .. } | Action::Candidate { memory, .. }
                    if memory.lifecycle_state == LifecycleState::Accepted =>
                {
                    Some(memory.content.clone())
                }
                Action::Promote { content, .. } => Some(content.clone()),
                _ => None,
            })
            .collect();
        if !accepting.is_empty() && !approve(&accepting)? {
            bail!("import cancelled; nothing was changed");
        }
    }
    let (mut imported, mut candidates, mut quarantined) = (Vec::new(), Vec::new(), 0);
    let (mut unchanged, mut kept_local) = (0, 0);
    let (mut forgotten, mut duplicate_ids, mut duplicate_content, mut other_layers) = (0, 0, 0, 0);
    for (line, action) in plan {
        let context = |e: anyhow::Error| e.context(format!("entry at line {line}"));
        match action {
            Action::Unchanged => unchanged += 1,
            Action::KeptLocal => kept_local += 1,
            Action::Skipped(Skip::Forgotten) => forgotten += 1,
            Action::Skipped(Skip::DuplicateId) => duplicate_ids += 1,
            Action::Skipped(Skip::DuplicateContent) => duplicate_content += 1,
            Action::Skipped(Skip::Layer) => other_layers += 1,
            Action::Promote { id, .. } => {
                store.accept_imported(&id).map_err(context)?;
                imported.push(id);
            }
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
                let state = memory.lifecycle_state;
                store
                    .insert_memory_as(memory, &id, Some(&created_at))
                    .map_err(context)?;
                for anchor in &anchors {
                    store.add_citation(&id, anchor, None).map_err(context)?;
                }
                if state == LifecycleState::Candidate {
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
                let state = memory.lifecycle_state;
                let id = store
                    .insert_memory_with_citations(memory, &anchors, None)
                    .map_err(context)?;
                let id = if options.dry_run {
                    "(dry run)".to_string()
                } else {
                    id
                };
                if state == LifecycleState::Candidate {
                    candidates.push(id);
                } else {
                    imported.push(id);
                }
            }
        }
    }
    if !options.dry_run {
        lock.commit()?;
    }
    let mut report = json!({
        "entries": entries.len(),
        "imported": imported,
        "candidates": candidates,
        "unchanged": unchanged,
        "kept_local": kept_local,
        "skipped": {"forgotten": forgotten, "duplicate_id": duplicate_ids, "duplicate_content": duplicate_content, "other_layers": other_layers},
        "verification_capped": capped,
        "quarantined": quarantined,
        "dry_run": options.dry_run,
    });
    if !candidates.is_empty() {
        report["note"] = json!(if options.accept {
            "edited, conflicting or guard-flagged entries stay candidates: review them with `memctrl candidates`"
        } else {
            "new entries are candidates until reviewed: read the file's changes, then run `memctrl import --accept <file>`, or review them one by one with `memctrl candidates`"
        });
    }
    Ok(report)
}

/// An entry in an existing export that the store hasn't decided on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Unmerged {
    pub line: usize,
    /// The start of the entry's text.
    pub preview: String,
    /// "not in your store", or "waiting for review" when only a candidate holds it.
    pub reason: &'static str,
}

/// Entries of `existing` (an export on disk) that the store has never seen, or holds only as
/// candidates. Writing a new export over them would drop a teammate's entries or hand edits.
/// Entries the store has decided on in any way (kept, rejected, refuted, forgotten) are fine:
/// dropping those is the point.
pub fn unmerged(store: &MemoryStore, existing: &str) -> Result<Vec<Unmerged>> {
    let entries = parse(&normalize(existing))?;
    let memories = store.list_memories(None, true)?;
    let (forgotten_ids, forgotten_text) = store.tombstones()?;
    let decided = |memory: &Memory| memory.lifecycle_state != LifecycleState::Candidate;
    let mut found = Vec::new();
    for entry in &entries {
        let content = sanitize_text(&entry.content);
        if forgotten_text.contains(&text_digest(&content)) {
            continue;
        }
        let by_id = match &entry.meta {
            Some(meta) if text_digest(&entry.content) == meta.sha => match canonical_id(&meta.id) {
                Ok(id) if forgotten_ids.contains(&id) => continue,
                Ok(id) => memories.iter().find(|m| m.id == id),
                Err(_) => None,
            },
            _ => None,
        };
        let reason = match by_id {
            Some(memory) if decided(memory) => continue,
            Some(_) => "waiting for review",
            None => {
                let mut same = memories.iter().filter(|m| m.content == content).peekable();
                if same.peek().is_none() {
                    "not in your store"
                } else if same.any(decided) {
                    continue;
                } else {
                    "waiting for review"
                }
            }
        };
        let first = content.lines().next().unwrap_or_default();
        let mut preview: String = first.chars().take(60).collect();
        if preview.len() < content.len() {
            preview.push('…');
        }
        found.push(Unmerged {
            line: entry.line,
            preview,
            reason,
        });
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_entry(content: &str) -> String {
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
        text
    }

    #[test]
    fn content_lines_that_look_like_markup_survive_a_round_trip() {
        for content in [
            "plain",
            "<!-- memctrl {\"id\":\"x\"} -->",
            "first\n   <!-- memctrl {\"id\":\"x\"} -->",
            "\\leading backslash",
            "first\n\nsecond paragraph\n- not a bullet",
            "two\n\n\nblank lines",
        ] {
            let entries = parse(&render_entry(content)).unwrap();
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

    #[test]
    fn byte_order_marks_and_crlf_are_normalized() {
        let text = format!("\u{feff}{HEADER}\r\n- hello\r\n");
        assert_eq!(parse(&normalize(&text)).unwrap()[0].content, "hello");
    }
}
