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
const HEADER: &str = "<!-- memctrl export v2. Review changes like code. Edit entry text freely \
(edited entries return as candidates on import) and add new `- ` bullets for teammates to review; \
keep the memctrl comments intact. -->";
/// The export format this version writes. It reads v1 and v2. v2 adds the Retracted section; a v1
/// reader would take its records for live entries, so the version had to change.
const FORMAT: u32 = 2;
const INTRO: &str = "Shared memory for this repository. After pulling, run `memctrl sync` \
(or `memctrl import`, then `memctrl export`), so your export keeps teammates' entries.";
/// Retraction records: refuted, superseded and forgotten memories, so corrections travel too.
const RETRACTED: &str = "Retracted";
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

/// How a memory was retracted. Merging only ever moves a memory to a stronger state:
/// active, then disputed (a live entry's verification), then superseded or refuted, then
/// forgotten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retracted {
    Superseded,
    Refuted,
    Forgotten,
}
impl Retracted {
    fn label(self) -> &'static str {
        match self {
            Self::Superseded => "superseded",
            Self::Refuted => "refuted",
            Self::Forgotten => "forgotten",
        }
    }
    fn rank(self) -> u8 {
        match self {
            Self::Superseded | Self::Refuted => 1,
            Self::Forgotten => 2,
        }
    }
}

/// Metadata of a record in the Retracted section. A forgotten record carries no text, so a
/// forgotten secret is never published again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Retraction {
    id: String,
    layer: String,
    state: Retracted,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    superseded_by: Option<String>,
    /// Digest of the memory's text, so the same text under another id stays out too.
    sha: String,
}

/// Export order, which is also the section a hand-written bullet is filed under.
fn section(memory: &Memory) -> (u8, &'static str) {
    section_of(memory.claim_type, memory.verification_state)
}

fn section_of(claim: ClaimType, verification: VerificationState) -> (u8, &'static str) {
    if verification == VerificationState::Disputed {
        return (4, "Disputed");
    }
    match claim {
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

/// The bullet lines of an entry's text (continuation lines indented, markup-like lines escaped).
fn bullet(content: &str) -> String {
    let mut out = String::new();
    for (index, line) in content.trim_end().split('\n').enumerate() {
        let line = escape(line.trim_end());
        if index == 0 {
            out.push_str(&format!("- {line}\n"));
        } else if line.is_empty() {
            out.push('\n');
        } else {
            out.push_str(&format!("  {line}\n"));
        }
    }
    out
}

fn comment(json: &str) -> String {
    format!("  <!-- memctrl {json} -->\n")
}

/// One live entry in the export, before ordering.
struct Item {
    section: (u8, &'static str),
    /// Entries with an id sort by it within their section, so two pull requests that each add
    /// an entry rarely touch the same lines. Hand-written bullets (no id) follow in file order.
    id: Option<String>,
    order: usize,
    block: String,
}

/// The export text for these live entries and retraction records.
fn assemble(mut items: Vec<Item>, mut retracted: Vec<(String, String)>) -> String {
    items.sort_by(|a, b| {
        a.section
            .0
            .cmp(&b.section.0)
            .then_with(|| match (&a.id, &b.id) {
                (Some(x), Some(y)) => x.cmp(y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.order.cmp(&b.order),
            })
    });
    let mut out = format!("{HEADER}\n# Project memory\n\n{INTRO}\n");
    let mut current = None;
    for item in &items {
        if current != Some(item.section.0) {
            out.push_str(&format!("\n## {}\n\n", item.section.1));
            current = Some(item.section.0);
        }
        out.push_str(&item.block);
    }
    if !retracted.is_empty() {
        retracted.sort();
        out.push_str(&format!("\n## {RETRACTED}\n\n"));
        for (_, block) in &retracted {
            out.push_str(block);
        }
    }
    out
}

fn store_meta(store: &MemoryStore, memory: &Memory) -> Result<Meta> {
    let content = memory.content.trim_end();
    let cite = store
        .citations(&memory.id)?
        .into_iter()
        // Hashes of a secrets file must never be published (2.1.0 could store them through a
        // symlink); the citation stays local.
        .filter(|c| !crate::citations::secret_file(&c.path))
        .map(|c| CiteMeta {
            lines: lines_label(c.line_start, c.line_end),
            path: c.path,
            file_hash: c.file_hash,
            span_hash: c.span_hash,
            head_hash: c.head_hash,
            head_offset: c.head_offset,
        })
        .collect();
    Ok(Meta {
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
    })
}

fn retraction_block(record: &Retraction, text: Option<&str>) -> Result<(String, String)> {
    // A forgotten record never repeats the text: it may be what had to be forgotten.
    let shown = match (record.state, text) {
        (Retracted::Forgotten, _) | (_, None) => format!("({})", record.state.label()),
        (_, Some(text)) => text.to_string(),
    };
    Ok((
        record.id.clone(),
        format!(
            "{}{}",
            bullet(&shown),
            comment(&serde_json::to_string(record)?)
        ),
    ))
}

/// The reason a memory was refuted, from its evidence (the latest one).
fn refute_reason(store: &MemoryStore, id: &str) -> Result<Option<String>> {
    Ok(store
        .evidence(id)?
        .iter()
        .rev()
        .find(|e| e["source_id"] == "refute")
        .and_then(|e| e["metadata"]["reason"].as_str().map(String::from)))
}

/// Retraction records this store knows for `layers`: its refuted and superseded memories, and
/// its tombstones. Tombstones written before 2.2 have no layer; they are included when `mentioned`
/// (the file being merged names the id).
fn store_retractions(
    store: &MemoryStore,
    layers: &[String],
    mentioned: &HashSet<String>,
) -> Result<HashMap<String, (Retraction, Option<String>)>> {
    let mut records = HashMap::new();
    for tombstone in store.tombstone_records()? {
        let layer = match tombstone.layer {
            Some(layer) if layers.contains(&layer) => layer,
            None if mentioned.contains(&tombstone.id) => layers[0].clone(),
            _ => continue,
        };
        let state = match tombstone.state.as_str() {
            "refuted" => Retracted::Refuted,
            "superseded" => Retracted::Superseded,
            _ => Retracted::Forgotten,
        };
        records.insert(
            tombstone.id.clone(),
            (
                Retraction {
                    id: tombstone.id,
                    layer,
                    state,
                    reason: tombstone.reason,
                    superseded_by: None,
                    sha: tombstone.digest,
                },
                None,
            ),
        );
    }
    for layer in layers {
        for memory in store.list_memories(Some(layer), true)? {
            let state = if memory.verification_state == VerificationState::Refuted {
                Retracted::Refuted
            } else if memory.lifecycle_state == LifecycleState::Superseded {
                Retracted::Superseded
            } else {
                continue;
            };
            if records
                .get(&memory.id)
                .is_some_and(|(r, _): &(Retraction, _)| r.state.rank() > state.rank())
            {
                continue;
            }
            let superseded_by = if state == Retracted::Superseded {
                store.relations(&memory.id)?.iter().find_map(|r| {
                    (r["relation_type"] == "supersedes" && r["to_memory_id"] == memory.id.as_str())
                        .then(|| r["from_memory_id"].as_str().map(String::from))
                        .flatten()
                })
            } else {
                None
            };
            let reason = if state == Retracted::Refuted {
                refute_reason(store, &memory.id)?
            } else {
                None
            };
            records.insert(
                memory.id.clone(),
                (
                    Retraction {
                        id: memory.id.clone(),
                        layer: memory.layer.clone(),
                        state,
                        reason,
                        superseded_by,
                        sha: text_digest(memory.content.trim_end()),
                    },
                    Some(memory.content.clone()),
                ),
            );
        }
    }
    Ok(records)
}

/// Renders the export from the store. Deterministic for a given store: no timestamps, no access
/// counts, entries ordered by id.
pub fn render(store: &MemoryStore, layers: &[String]) -> Result<(String, usize)> {
    render_merged(store, layers, None)
}

/// Renders the export, merging `existing` (the file on disk) when given: entries the store
/// hasn't decided on (a teammate's entries, hand-written bullets, edits waiting for review) are
/// carried over verbatim instead of dropped, and retraction records from both sides are kept.
/// Anything retracted on either side leaves the live sections. Returns the text and the number
/// of live entries.
pub fn render_merged(
    store: &MemoryStore,
    layers: &[String],
    existing: Option<&str>,
) -> Result<(String, usize)> {
    for layer in layers {
        validate_identifier(layer, "layer")?;
    }
    ensure!(!layers.is_empty(), "export needs at least one layer");
    let parsed = match existing {
        Some(text) => parse(&normalize(text))?,
        None => Vec::new(),
    };
    let mentioned: HashSet<String> = parsed
        .iter()
        .filter_map(|e| {
            e.meta
                .as_ref()
                .map(|m| m.id.clone())
                .or_else(|| e.retraction.as_ref().map(|r| r.id.clone()))
        })
        .collect();
    let mut retracted = store_retractions(store, layers, &mentioned)?;
    // The file's retraction records travel on, whatever this store knows.
    for entry in &parsed {
        if let Some(record) = &entry.retraction {
            let stronger = retracted
                .get(&record.id)
                .is_none_or(|(known, _)| record.state.rank() > known.state.rank());
            if stronger {
                let text = (record.state != Retracted::Forgotten).then(|| entry.content.clone());
                retracted.insert(record.id.clone(), (record.clone(), text));
            } else if let Some((known, text)) = retracted.get_mut(&record.id) {
                // Keep the text a teammate's record carries when this store has none.
                if text.is_none() && known.state != Retracted::Forgotten {
                    *text = Some(entry.content.clone());
                }
            }
        }
    }
    let retracted_digests: HashSet<String> =
        retracted.values().map(|(r, _)| r.sha.clone()).collect();
    let mut items = Vec::new();
    let mut from_store = HashSet::new();
    for layer in layers {
        for memory in store.list_memories(Some(layer), false)? {
            if retracted.contains_key(&memory.id) {
                continue;
            }
            let meta = store_meta(store, &memory)?;
            from_store.insert(memory.id.clone());
            items.push(Item {
                section: section(&memory),
                id: Some(memory.id.clone()),
                order: 0,
                block: format!(
                    "{}{}",
                    bullet(&memory.content),
                    comment(&serde_json::to_string(&meta)?)
                ),
            });
        }
    }
    if existing.is_some() {
        for (order, carried) in carried_entries(store, &parsed)?.into_iter().enumerate() {
            let id = carried.meta.as_ref().map(|m| m.id.clone());
            if id.as_ref().is_some_and(|id| retracted.contains_key(id))
                || retracted_digests.contains(&text_digest(&sanitize_text(&carried.content)))
            {
                continue;
            }
            // An edit of an entry the store renders keeps its text but loses the metadata, so
            // the file never holds one id twice.
            let meta = carried
                .meta
                .as_ref()
                .filter(|m| !from_store.contains(&m.id));
            let section = match meta {
                Some(m) => section_of(m.claim_type, m.verification_state),
                None => section_of(
                    claim_for_heading(&carried.heading),
                    VerificationState::Unverified,
                ),
            };
            let block = match meta {
                Some(m) => format!(
                    "{}{}",
                    bullet(&carried.content),
                    comment(&serde_json::to_string(m)?)
                ),
                None => bullet(&carried.content),
            };
            items.push(Item {
                section,
                id: meta.map(|m| m.id.clone()),
                order,
                block,
            });
        }
    }
    let count = items.len();
    let records = retracted
        .values()
        .map(|(record, text)| retraction_block(record, text.as_deref()))
        .collect::<Result<Vec<_>>>()?;
    Ok((assemble(items, records), count))
}

/// Ensures a `.gitignore` next to an export inside `.memctrl/`, so committing the folder never
/// commits the database.
pub fn ensure_gitignore(export: &std::path::Path) -> Result<()> {
    let Some(dir) = export.parent() else {
        return Ok(());
    };
    if dir.file_name().is_some_and(|n| n == ".memctrl") {
        std::fs::create_dir_all(dir)?;
        // create_new never follows a symlink planted at the path, dangling or not.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(".gitignore"))
        {
            Ok(mut file) => std::io::Write::write_all(&mut file, GITIGNORE.as_bytes())?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
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
    /// Set for records in the Retracted section (format v2).
    retraction: Option<Retraction>,
}

fn parse(text: &str) -> Result<Vec<Entry>> {
    Ok(parse_versioned(text)?.1)
}

/// The export's format version and its entries.
fn parse_versioned(text: &str) -> Result<(u32, Vec<Entry>)> {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let version = first
        .strip_prefix(HEADER_PREFIX)
        .map(|rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .and_then(|digits| digits.parse::<u32>().ok());
    let version = match version {
        Some(known @ 1..=FORMAT) => known,
        Some(newer) if newer > FORMAT => {
            bail!("written by a newer memctrl (export format v{newer}); upgrade memctrl to read it")
        }
        _ => bail!(
            "not a memctrl export (expected a first line starting with \"<!-- memctrl export v\")"
        ),
    };
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
                if entry.heading == RETRACTED {
                    let record: Retraction = serde_json::from_str(json)
                        .with_context(|| format!("line {number}: invalid retraction record"))?;
                    entry.retraction = Some(record);
                } else {
                    let meta: Meta = serde_json::from_str(json)
                        .with_context(|| format!("line {number}: invalid memctrl metadata"))?;
                    entry.meta = Some(meta);
                }
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
                retraction: None,
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
    if let Some(entry) = entries
        .iter()
        .find(|e| e.heading == RETRACTED && e.retraction.is_none())
    {
        bail!(
            "line {}: an entry under \"## {RETRACTED}\" needs its memctrl record",
            entry.line
        );
    }
    Ok((version, entries))
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
    /// A retraction record from the file, applied only if it lowers the memory's state.
    Retract(Retraction),
    /// A known memory the file marks disputed.
    Dispute(String),
}

/// Applies a teammate's retraction: a known memory moves to the stronger state, never back; an
/// unknown id is remembered, so no older export can bring it in. Returns what happened.
fn apply_retraction(store: &MemoryStore, record: &Retraction) -> Result<&'static str> {
    let reason = record.reason.as_deref();
    let Some(local) = store.get_memory(&record.id, true)? else {
        store.record_tombstone(
            &record.id,
            &record.sha,
            &record.layer,
            record.state.label(),
            reason,
        )?;
        return Ok("tombstoned");
    };
    Ok(match record.state {
        Retracted::Refuted if local.verification_state == VerificationState::Refuted => "unchanged",
        Retracted::Refuted => {
            store.refute_memory(
                &record.id,
                reason.unwrap_or("refuted in a teammate's export"),
                None,
            )?;
            "refuted"
        }
        Retracted::Superseded
            if local.verification_state == VerificationState::Refuted
                || matches!(
                    local.lifecycle_state,
                    LifecycleState::Superseded
                        | LifecycleState::Rejected
                        | LifecycleState::Archived
                ) =>
        {
            "unchanged"
        }
        Retracted::Superseded => {
            store.update_lifecycle(&record.id, LifecycleState::Superseded)?;
            if let Some(by) = record
                .superseded_by
                .as_deref()
                .filter(|by| *by != record.id)
            {
                if store.get_memory(by, true)?.is_some() {
                    store.add_relation(
                        by,
                        &record.id,
                        crate::model::RelationType::Supersedes,
                        json!({"source": "import"}),
                    )?;
                }
            }
            "superseded"
        }
        Retracted::Forgotten => {
            // Archived, not deleted: a teammate's forget can be undone here.
            if local.lifecycle_state != LifecycleState::Archived {
                store.update_lifecycle(&record.id, LifecycleState::Archived)?;
            }
            store.record_tombstone(
                &record.id,
                &text_digest(&local.content),
                &record.layer,
                "forgotten",
                reason,
            )?;
            "forgotten"
        }
    })
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
    let mut redacted = std::collections::BTreeSet::new();
    let mut oversized = Vec::new();
    let mut plan = Vec::new();
    // Ids the file itself retracts: their live copies (from an older export, say) are skipped.
    let retracted_here: HashSet<String> = entries
        .iter()
        .filter_map(|e| e.retraction.as_ref())
        .filter_map(|r| canonical_id(&r.id).ok())
        .collect();
    for entry in &entries {
        if let Some(record) = &entry.retraction {
            let context = |e: anyhow::Error| e.context(format!("entry at line {}", entry.line));
            let mut record = record.clone();
            record.id = canonical_id(&record.id).map_err(context)?;
            validate_identifier(&record.layer, "layer").map_err(context)?;
            ensure!(
                digest_like(&record.sha),
                "entry at line {}: a retraction needs the 32-digit sha of its text",
                entry.line
            );
            let action = if options.layers.contains(&record.layer) {
                Action::Retract(record)
            } else {
                Action::Skipped(Skip::Layer)
            };
            plan.push((entry.line, action));
            continue;
        }
        // Too large to be a memory: skipped and reported, never cut or fatal to the import.
        if entry.content.len() > crate::model::MAX_MEMORY_BYTES {
            oversized.push(entry.line);
            continue;
        }
        let content = sanitize_text(&entry.content);
        if content != entry.content {
            redacted.extend(crate::sanitize::redactions(&entry.content));
        }
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
                } else if forgotten_ids.contains(&id)
                    || forgotten_text.contains(&digest)
                    || retracted_here.contains(&id)
                {
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
                    } else if meta.verification_state == VerificationState::Disputed
                        && local.lifecycle_state == LifecycleState::Accepted
                        && matches!(
                            local.verification_state,
                            VerificationState::Unverified | VerificationState::Supported
                        )
                    {
                        // A teammate disputed it: that doubt travels, like a retraction.
                        Action::Dispute(id)
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
    let mut retractions: std::collections::BTreeMap<&'static str, usize> = Default::default();
    for (line, action) in plan {
        let context = |e: anyhow::Error| e.context(format!("entry at line {line}"));
        match action {
            Action::Retract(record) => {
                let outcome = apply_retraction(store, &record).map_err(context)?;
                if outcome != "unchanged" {
                    *retractions.entry(outcome).or_default() += 1;
                }
            }
            Action::Dispute(id) => {
                store
                    .update_verification(&id, VerificationState::Disputed)
                    .map_err(context)?;
                *retractions.entry("disputed").or_default() += 1;
            }
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
        "skipped": {"forgotten": forgotten, "duplicate_id": duplicate_ids, "duplicate_content": duplicate_content, "other_layers": other_layers, "oversized": oversized},
        "verification_capped": capped,
        "quarantined": quarantined,
        "dry_run": options.dry_run,
    });
    if !retractions.is_empty() {
        report["retracted"] = json!(retractions);
    }
    if !redacted.is_empty() {
        report["redacted"] = json!(redacted);
    }
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
    let pending = pending_entries(store, &entries)?;
    Ok(pending
        .into_iter()
        .map(|(entry, reason)| {
            let content = sanitize_text(&entry.content);
            let first = content.lines().next().unwrap_or_default();
            let mut preview: String = first.chars().take(60).collect();
            if preview.len() < content.len() {
                preview.push('…');
            }
            Unmerged {
                line: entry.line,
                preview,
                reason,
            }
        })
        .collect())
}

/// Live entries of a parsed export that an export from this store must carry over.
fn carried_entries(store: &MemoryStore, entries: &[Entry]) -> Result<Vec<Entry>> {
    Ok(pending_entries(store, entries)?
        .into_iter()
        .map(|(entry, _)| entry)
        .collect())
}

fn pending_entries(store: &MemoryStore, entries: &[Entry]) -> Result<Vec<(Entry, &'static str)>> {
    let memories = store.list_memories(None, true)?;
    let (forgotten_ids, forgotten_text) = store.tombstones()?;
    let decided = |memory: &Memory| memory.lifecycle_state != LifecycleState::Candidate;
    let mut found = Vec::new();
    for entry in entries.iter().filter(|e| e.retraction.is_none()) {
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
        found.push((entry.clone(), reason));
    }
    Ok(found)
}

/// What `check` found in an export, without a store.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CheckReport {
    pub format: u32,
    pub entries: usize,
    pub retracted: usize,
    /// Problems that make the file unsafe or unreadable for teammates.
    pub errors: Vec<String>,
    /// Things a reviewer should know: edited or hand-written entries, an old format, an order
    /// export would rewrite.
    pub warnings: Vec<String>,
}

/// Validates an export without a database, for CI: the header and merge markers, that every id
/// is canonical and appears once, every entry's sha, and that the file is in the layout export
/// writes. With `base` (the same file at the base commit), it also fails when a retraction record
/// was removed or a retracted memory is live again: memctrl 2.1 rewrote the file from its store
/// and could silently drop them.
pub fn check(text: &str, base: Option<&str>) -> Result<CheckReport> {
    let text = normalize(text);
    let (format, entries) = parse_versioned(&text)?;
    let mut report = CheckReport {
        format,
        ..CheckReport::default()
    };
    if format < FORMAT {
        report.warnings.push(format!(
            "written in export format v{format}; run `memctrl export` to upgrade it to v{FORMAT}"
        ));
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut live = HashSet::new();
    let mut retracted = HashSet::new();
    for entry in &entries {
        let raw = match (&entry.meta, &entry.retraction) {
            (Some(meta), _) => Some(meta.id.as_str()),
            (_, Some(record)) => Some(record.id.as_str()),
            _ => None,
        };
        let Some(raw) = raw else {
            report.entries += 1;
            report.warnings.push(format!(
                "line {}: hand-written entry; teammates import it as a candidate for review",
                entry.line
            ));
            continue;
        };
        let id = match canonical_id(raw) {
            Ok(id) => id,
            Err(error) => {
                report.errors.push(format!("line {}: {error}", entry.line));
                continue;
            }
        };
        if id != raw {
            report.errors.push(format!(
                "line {}: id {raw} is not written as memctrl writes it ({id})",
                entry.line
            ));
        }
        if let Some(first) = seen.insert(id.clone(), entry.line) {
            report.errors.push(format!(
                "line {}: id {} also appears at line {first}",
                entry.line,
                &id[..8]
            ));
        }
        match (&entry.meta, &entry.retraction) {
            (Some(meta), _) => {
                report.entries += 1;
                live.insert(id);
                if text_digest(&entry.content) != meta.sha {
                    report.warnings.push(format!(
                        "line {}: edited since it was exported; teammates import it as a candidate",
                        entry.line
                    ));
                }
            }
            (_, Some(record)) => {
                report.retracted += 1;
                retracted.insert(id);
                if !digest_like(&record.sha) {
                    report.errors.push(format!(
                        "line {}: the retraction record's sha must be 32 hex digits",
                        entry.line
                    ));
                }
            }
            _ => {}
        }
    }
    if format == FORMAT && report.errors.is_empty() {
        let mut items = Vec::new();
        let mut records = Vec::new();
        for (order, entry) in entries.iter().enumerate() {
            match (&entry.meta, &entry.retraction) {
                (Some(meta), _) => items.push(Item {
                    section: section_of(meta.claim_type, meta.verification_state),
                    id: Some(meta.id.clone()),
                    order,
                    block: format!(
                        "{}{}",
                        bullet(&entry.content),
                        comment(&serde_json::to_string(meta)?)
                    ),
                }),
                (_, Some(record)) => {
                    let text =
                        (record.state != Retracted::Forgotten).then_some(entry.content.as_str());
                    records.push(retraction_block(record, text)?);
                }
                _ => items.push(Item {
                    section: section_of(
                        claim_for_heading(&entry.heading),
                        VerificationState::Unverified,
                    ),
                    id: None,
                    order,
                    block: bullet(&entry.content),
                }),
            }
        }
        if assemble(items, records) != text {
            report.warnings.push(
                "not in the layout export writes (order, sections or spacing); run `memctrl export` to rewrite it"
                    .into(),
            );
        }
    }
    if let Some(base) = base {
        let (_, before) =
            parse_versioned(&normalize(base)).context("the base version of the file")?;
        for record in before.iter().filter_map(|e| e.retraction.as_ref()) {
            let Ok(id) = canonical_id(&record.id) else {
                continue;
            };
            if live.contains(&id) {
                report.errors.push(format!(
                    "{} was {} and is live again; an export from memctrl 2.1 or a hand edit may have restored it",
                    &id[..8],
                    record.state.label()
                ));
            } else if !retracted.contains(&id) {
                report.errors.push(format!(
                    "the retraction record of {} ({}) was removed; teammates' stores would keep it",
                    &id[..8],
                    record.state.label()
                ));
            }
        }
    }
    Ok(report)
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
