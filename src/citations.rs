//! Citations tie a memory to the code it describes, so memctrl can tell when that code changed.
//!
//! A citation stores a path relative to the project root, an optional line range, and hashes of
//! the cited text. Checking is read-only and deterministic: no git, no network, no model.
use crate::sanitize::{has_secrets, sanitize_revision};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

/// Files above this size are not hashed; their citations report `unchecked`.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SPAN_LINES: u32 = 2000;
/// Upper bound on candidate positions verified when looking for a moved span.
const MAX_MOVE_CANDIDATES: usize = 64;

/// A citation as the user wrote it: `path`, `path:LINE` or `path:START-END`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationSpec {
    pub path: String,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
}

/// Hashes taken when a citation is created or refreshed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// Root-relative path with `/` separators.
    pub path: String,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub file_hash: String,
    pub span_hash: Option<String>,
    /// Hash of the span's first non-blank line, used to find the span again after it moves.
    pub head_hash: Option<String>,
    /// Position of that line within the span (0 = first line).
    pub head_offset: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Citation {
    pub id: String,
    pub memory_id: String,
    pub path: String,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub file_hash: String,
    pub span_hash: Option<String>,
    pub head_hash: Option<String>,
    pub head_offset: Option<u32>,
    /// Git commit checked out when the citation was taken, if known.
    pub commit_sha: Option<String>,
    pub created_at: String,
    pub refreshed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum Status {
    /// The cited text is unchanged.
    Fresh,
    /// The cited lines are unchanged but now start at `line_start`.
    Moved { line_start: u32 },
    /// The cited text changed.
    Changed,
    /// The cited file no longer exists.
    Missing,
    /// The path resolves outside the project root (for example through a symlink).
    Unresolved,
    /// The path is not a regular file or is too large to hash.
    Unchecked,
}
impl Status {
    pub fn is_stale(self) -> bool {
        matches!(self, Self::Changed | Self::Missing)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Moved { .. } => "moved",
            Self::Changed => "changed",
            Self::Missing => "missing",
            Self::Unresolved => "unresolved",
            Self::Unchecked => "unchecked",
        }
    }
}

/// Memory-level summary: `stale` if any citation changed or vanished, `fresh` if every
/// citation checked out, `unknown` otherwise. `None` when the memory has no citations.
pub fn freshness(statuses: &[Status]) -> Option<&'static str> {
    if statuses.is_empty() {
        None
    } else if statuses.iter().any(|s| s.is_stale()) {
        Some("stale")
    } else if statuses
        .iter()
        .all(|s| matches!(s, Status::Fresh | Status::Moved { .. }))
    {
        Some("fresh")
    } else {
        Some("unknown")
    }
}

/// Parses `path`, `path:12` or `path:12-40`. Paths must be relative, stay inside the project
/// (no `..`), and contain no credentials.
pub fn parse_spec(spec: &str) -> Result<CitationSpec> {
    let spec = spec.trim();
    ensure!(!spec.is_empty(), "citation must not be empty");
    let (path, lines) = match spec.rsplit_once(':') {
        Some((path, range))
            if !range.is_empty() && range.chars().all(|c| c.is_ascii_digit() || c == '-') =>
        {
            (path, Some(range))
        }
        _ => (spec, None),
    };
    let (line_start, line_end) = match lines {
        None => (None, None),
        Some(range) => {
            let (start, end) = match range.split_once('-') {
                Some((a, b)) => (a, b),
                None => (range, range),
            };
            let start: u32 = start.parse().context("citation line must be a number")?;
            let end: u32 = end.parse().context("citation line must be a number")?;
            ensure!(start >= 1, "citation lines start at 1");
            ensure!(end >= start, "citation line range must not be reversed");
            ensure!(
                end - start < MAX_SPAN_LINES,
                "citation spans at most {MAX_SPAN_LINES} lines"
            );
            (Some(start), Some(end))
        }
    };
    let normalized = path.replace('\\', "/");
    let normalized = normalized.trim_start_matches("./").to_string();
    ensure!(!normalized.is_empty(), "citation path must not be empty");
    ensure!(
        normalized.chars().count() <= 1024,
        "citation path is too long"
    );
    ensure!(
        !normalized.chars().any(char::is_control),
        "citation path must not contain control characters"
    );
    ensure!(
        !has_secrets(&normalized),
        "citation path must not contain credentials"
    );
    ensure!(
        !secret_file(&normalized),
        "refusing to cite {normalized}: it looks like a secrets file, and citation hashes are shared through exports"
    );
    let as_path = Path::new(&normalized);
    ensure!(
        !as_path.is_absolute()
            && !normalized.starts_with('/')
            && !(normalized.len() > 1 && normalized.as_bytes()[1] == b':'),
        "citation paths are relative to the project root"
    );
    ensure!(
        as_path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "citation paths must stay inside the project root"
    );
    Ok(CitationSpec {
        path: normalized,
        line_start,
        line_end,
    })
}

/// Files that hold credentials. Their line hashes would be shared through `memctrl export`, and a
/// short secret can be recovered from an unsalted hash by guessing.
pub fn secret_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    if lower
        .split('/')
        .any(|part| matches!(part, ".ssh" | ".aws" | ".gnupg" | ".docker" | ".kube"))
    {
        return true;
    }
    let name = lower.rsplit('/').next().unwrap_or("");
    if [".example", ".sample", ".template", ".dist", ".pub"]
        .iter()
        .any(|s| name.ends_with(s))
    {
        return false;
    }
    name == ".env"
        || name.starts_with(".env.")
        || name.ends_with(".env")
        || [
            ".pem",
            ".key",
            ".p12",
            ".pfx",
            ".keystore",
            ".jks",
            ".tfstate",
            ".tfvars",
        ]
        .iter()
        .any(|s| name.ends_with(s))
        || ["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"]
            .iter()
            .any(|s| name.starts_with(s))
        || matches!(
            name,
            ".npmrc"
                | ".pypirc"
                | ".netrc"
                | ".git-credentials"
                | ".htpasswd"
                | "credentials"
                | "credentials.json"
                | "secrets.json"
                | "secrets.yaml"
                | "secrets.yml"
                | "service-account.json"
        )
}

/// The project root: `MEMCTRL_PROJECT_ROOT`, else the configured root, else the nearest
/// ancestor of the working directory containing `.git`, else the working directory.
pub fn project_root(configured: Option<&Path>) -> Result<PathBuf> {
    project_root_in(configured, None)
}

/// The project root for these rules: as `project_root`, but the search for `.git` starts at the
/// project folder (where `.memoryrc` is), so an MCP server started elsewhere still finds it.
pub fn root_for(rules: &crate::rules::Rules) -> Result<PathBuf> {
    project_root_in(
        rules.citations_root.as_deref(),
        rules.project_dir.as_deref(),
    )
}

fn project_root_in(configured: Option<&Path>, project_dir: Option<&Path>) -> Result<PathBuf> {
    if let Some(root) = std::env::var_os("MEMCTRL_PROJECT_ROOT").filter(|r| !r.is_empty()) {
        return PathBuf::from(root)
            .canonicalize()
            .context("MEMCTRL_PROJECT_ROOT does not exist");
    }
    if let Some(root) = configured {
        return root
            .canonicalize()
            .with_context(|| format!("citations root {} does not exist", root.display()));
    }
    let start = match project_dir {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir()?,
    };
    for dir in start.ancestors() {
        if dir.join(".git").exists() {
            return Ok(dir.canonicalize()?);
        }
    }
    Ok(start.canonicalize()?)
}

fn normalize(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .split('\n')
        .map(|line| line.trim_end().to_string())
        .collect()
}

/// First 32 hex digits (128 bits) of SHA-256: ample for change detection, and short enough
/// that output sanitizing never mistakes it for a credential.
pub fn digest(text: &str) -> String {
    let hash = Sha256::digest(text.as_bytes());
    hash.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// Digest of memory text for edit detection and tombstones: trailing whitespace per line and
/// trailing blank lines are ignored, so editors that trim whitespace don't count as edits.
pub fn text_digest(content: &str) -> String {
    let canonical = content
        .trim_end()
        .split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    digest(&canonical)
}

fn span(lines: &[String], start: u32, end: u32) -> Option<String> {
    let (start, end) = (start as usize, end as usize);
    (end <= lines.len()).then(|| lines[start - 1..end].join("\n"))
}

/// Reads `path` under `root`, refusing anything that resolves outside it.
fn read_scoped(root: &Path, path: &str) -> std::result::Result<String, Status> {
    let root = root.canonicalize().map_err(|_| Status::Unresolved)?;
    let full = root.join(path);
    let resolved = match full.canonicalize() {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Status::Missing),
        Err(_) => return Err(Status::Unresolved),
    };
    if !resolved.starts_with(&root) {
        return Err(Status::Unresolved);
    }
    // Never hash a secrets file, even for a citation stored before this check existed.
    let inside = resolved
        .strip_prefix(&root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    if secret_file(&inside) {
        return Err(Status::Unchecked);
    }
    let meta = std::fs::metadata(&resolved).map_err(|_| Status::Missing)?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return Err(Status::Unchecked);
    }
    let bytes = std::fs::read(&resolved).map_err(|_| Status::Unchecked)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Resolves a user-supplied spec against the working directory first (so `--cite` works from
/// subdirectories), then the root, and hashes the cited text.
pub fn anchor(root: &Path, cwd: &Path, spec: &CitationSpec) -> Result<Anchor> {
    let root = root.canonicalize()?;
    let existing: Vec<PathBuf> = [cwd.join(&spec.path), root.join(&spec.path)]
        .iter()
        .filter_map(|p| p.canonicalize().ok().filter(|p| p.is_file()))
        .collect();
    ensure!(
        !existing.is_empty(),
        "cited file {} does not exist",
        spec.path
    );
    let resolved = existing
        .iter()
        .find(|p| p.starts_with(&root))
        .with_context(|| format!("cited file {} is outside the project root", spec.path))?;
    let relative = resolved
        .strip_prefix(&root)
        .expect("resolved paths start with the root");
    let relative = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    // The typed path was checked in parse_spec; a symlink can still lead to a secrets file.
    ensure!(
        !secret_file(&relative),
        "refusing to cite {}: it resolves to {relative}, which looks like a secrets file, and citation hashes are shared through exports",
        spec.path
    );
    let text = match read_scoped(&root, &relative) {
        Ok(text) => text,
        Err(Status::Unchecked) => bail!(
            "cited file {relative} is not a regular file or exceeds {} MB",
            MAX_FILE_BYTES / 1024 / 1024
        ),
        Err(_) => bail!("cited file {relative} cannot be read inside the project root"),
    };
    let lines = normalize(&text);
    let (span_hash, head_hash, head_offset) = match (spec.line_start, spec.line_end) {
        (Some(start), Some(end)) => {
            let cited = span(&lines, start, end).with_context(|| {
                format!(
                    "{relative} has {} lines; cannot cite {start}-{end}",
                    lines.len()
                )
            })?;
            // Anchor on the first non-blank line: blank lines would match almost anywhere.
            let head = (start..=end)
                .map(|n| (n - start, &lines[n as usize - 1]))
                .find(|(_, line)| !line.trim().is_empty());
            match head {
                Some((offset, line)) => (Some(digest(&cited)), Some(digest(line)), Some(offset)),
                None => (Some(digest(&cited)), None, None),
            }
        }
        _ => (None, None, None),
    };
    Ok(Anchor {
        path: relative,
        line_start: spec.line_start,
        line_end: spec.line_end,
        file_hash: digest(&lines.join("\n")),
        span_hash,
        head_hash,
        head_offset,
    })
}

/// Compares a citation with the file as it is now. Never writes.
pub fn check(root: &Path, citation: &Citation) -> Status {
    let text = match read_scoped(root, &citation.path) {
        Ok(text) => text,
        Err(status) => return status,
    };
    let lines = normalize(&text);
    let (Some(start), Some(end), Some(span_hash)) =
        (citation.line_start, citation.line_end, &citation.span_hash)
    else {
        return if digest(&lines.join("\n")) == citation.file_hash {
            Status::Fresh
        } else {
            Status::Changed
        };
    };
    if span(&lines, start, end).is_some_and(|s| &digest(&s) == span_hash) {
        return Status::Fresh;
    }
    // Look for the same lines elsewhere. Each line is hashed at most once; only positions whose
    // anchor line matches (non-blank, so candidates are rare) are verified, and at most
    // MAX_MOVE_CANDIDATES of them.
    let width = (end - start + 1) as usize;
    let offset = citation.head_offset.unwrap_or(0) as usize;
    if let Some(head) = &citation.head_hash {
        let mut verified = 0;
        for (index, line) in lines.iter().enumerate() {
            if index < offset || line.trim().is_empty() || &digest(line) != head {
                continue;
            }
            let begin = index - offset;
            if begin + width > lines.len() {
                continue;
            }
            verified += 1;
            if digest(&lines[begin..begin + width].join("\n")) == *span_hash {
                return Status::Moved {
                    line_start: begin as u32 + 1,
                };
            }
            if verified == MAX_MOVE_CANDIDATES {
                break;
            }
        }
    }
    Status::Changed
}

/// Where a changed block starts now, judged by its first non-blank line: `Some` only when that
/// line occurs exactly once in the file, so the block can be followed without guessing.
fn unique_head(root: &Path, citation: &Citation) -> Option<u32> {
    let head = citation.head_hash.as_ref()?;
    let offset = citation.head_offset.unwrap_or(0) as usize;
    let lines = normalize(&read_scoped(root, &citation.path).ok()?);
    let mut found = lines
        .iter()
        .enumerate()
        .filter(|(index, line)| {
            *index >= offset && !line.trim().is_empty() && &digest(line) == head
        })
        .map(|(index, _)| index - offset);
    let begin = found.next()?;
    found.next().is_none().then_some(begin as u32 + 1)
}

fn range_label(start: Option<u32>, end: Option<u32>) -> Option<String> {
    start.map(|s| match end {
        Some(e) if e != s => format!("{s}-{e}"),
        _ => s.to_string(),
    })
}

/// The commit checked out in `root`, when git is available. Optional context only.
pub fn current_commit(root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let sha = String::from_utf8(output.stdout).ok()?.trim().to_string();
    let valid = output.status.success()
        && matches!(sha.len(), 40 | 64)
        && sha.chars().all(|c| c.is_ascii_hexdigit());
    valid.then(|| sanitize_revision(&sha))
}

/// A short human note for the first stale citation, e.g. "src/db.rs changed (+1 more)".
pub fn stale_note(checked: &[(Citation, Status)]) -> Option<String> {
    let stale: Vec<&(Citation, Status)> = checked.iter().filter(|(_, s)| s.is_stale()).collect();
    let (first, status) = stale.first()?;
    let more = if stale.len() > 1 {
        format!(" (+{} more)", stale.len() - 1)
    } else {
        String::new()
    };
    Some(format!("{} {}{more}", first.path, status.label()))
}

/// Notes for memories whose cited code changed, keyed by memory id. Best effort: a session brief
/// must still render when the project root can't be resolved.
pub fn stale_notes(
    store: &crate::store::MemoryStore,
    rules: &crate::rules::Rules,
) -> std::collections::HashMap<String, String> {
    let Ok(root) = root_for(rules) else {
        return Default::default();
    };
    let Ok(ids) = store.cited_memory_ids() else {
        return Default::default();
    };
    check_memories(store, &root, &ids)
        .map(|checked| {
            checked
                .into_iter()
                .filter_map(|(id, list)| stale_note(&list).map(|note| (id, note)))
                .collect()
        })
        .unwrap_or_default()
}

/// Each listed memory's citations with their current status. Memories without citations are
/// absent from the map.
pub fn check_memories(
    store: &crate::store::MemoryStore,
    root: &Path,
    memory_ids: &[String],
) -> Result<std::collections::HashMap<String, Vec<(Citation, Status)>>> {
    let mut checked = std::collections::HashMap::new();
    for id in memory_ids {
        let citations = store.citations(id)?;
        if !citations.is_empty() {
            let statuses = citations
                .into_iter()
                .map(|c| {
                    let status = check(root, &c);
                    (c, status)
                })
                .collect();
            checked.insert(id.clone(), statuses);
        }
    }
    Ok(checked)
}

/// Sets `why.freshness` on every cited fact. Runs after the cache, so it always reflects the
/// files as they are now.
pub fn annotate(
    store: &crate::store::MemoryStore,
    root: &Path,
    result: &mut crate::model::RetrievalResult,
) -> Result<()> {
    let checked = check_memories(store, root, &result.memory_ids)?;
    for why in &mut result.why {
        if let Some(list) = checked.get(&why.memory_id) {
            let statuses: Vec<Status> = list.iter().map(|(_, s)| *s).collect();
            why.freshness = freshness(&statuses).map(String::from);
        }
    }
    Ok(())
}

fn citation_json(citation: &Citation, status: Status) -> serde_json::Value {
    let lines = match (citation.line_start, citation.line_end) {
        (Some(a), Some(b)) if a == b => Some(a.to_string()),
        (Some(a), Some(b)) => Some(format!("{a}-{b}")),
        _ => None,
    };
    let mut value = serde_json::json!({
        "id": citation.id,
        "path": citation.path,
        "lines": lines,
        "status": status.label(),
        "commit_sha": citation.commit_sha,
        "created_at": citation.created_at,
        "refreshed_at": citation.refreshed_at,
    });
    if let Status::Moved { line_start } = status {
        value["now_at"] = serde_json::json!(line_start);
    }
    value
}

/// A memory's citations with live status, for `show`.
pub fn describe(
    store: &crate::store::MemoryStore,
    root: &Path,
    memory_id: &str,
) -> Result<serde_json::Value> {
    let citations: Vec<serde_json::Value> = store
        .citations(memory_id)?
        .iter()
        .map(|c| citation_json(c, check(root, c)))
        .collect();
    Ok(serde_json::json!(citations))
}

/// Active memories whose cited code changed or disappeared (all cited memories with
/// `include_fresh`).
pub fn stale_report(
    store: &crate::store::MemoryStore,
    root: &Path,
    layer: Option<&str>,
    include_fresh: bool,
) -> Result<serde_json::Value> {
    if let Some(layer) = layer {
        crate::sanitize::validate_identifier(layer, "layer")?;
    }
    let mut entries = Vec::new();
    let mut checked = 0;
    for id in store.cited_memory_ids()? {
        let Some(memory) = store.get_memory(&id, false)? else {
            continue;
        };
        if layer.is_some_and(|l| l != memory.layer) {
            continue;
        }
        checked += 1;
        let statuses: Vec<(Citation, Status)> = store
            .citations(&id)?
            .into_iter()
            .map(|c| {
                let status = check(root, &c);
                (c, status)
            })
            .collect();
        let summary: Vec<Status> = statuses.iter().map(|(_, s)| *s).collect();
        let state = freshness(&summary).unwrap_or("uncited");
        if include_fresh || state == "stale" {
            entries.push(serde_json::json!({
                "memory": {
                    "id": memory.id,
                    "content": memory.content,
                    "layer": memory.layer,
                    "claim_type": memory.claim_type,
                    "verification_state": memory.verification_state,
                },
                "freshness": state,
                "citations": statuses.iter().map(|(c, s)| citation_json(c, *s)).collect::<Vec<_>>(),
            }));
        }
    }
    Ok(serde_json::json!({"stale": entries, "checked": checked, "root": root}))
}

/// Re-anchors a memory's citations after a person re-checked the memory. A block that moved is
/// followed: exactly, or by its first line when that line now occurs once. When the block can't
/// be found unambiguously, nothing changes and the caller is asked for the range with `--cite`;
/// refresh never re-hashes the old line numbers onto unrelated code. `overrides` give explicit
/// ranges for paths the memory already cites.
pub fn refresh(
    store: &crate::store::MemoryStore,
    root: &Path,
    memory_id: &str,
    overrides: &[CitationSpec],
) -> Result<serde_json::Value> {
    let citations = store.citations(memory_id)?;
    ensure!(!citations.is_empty(), "memory has no citations to refresh");
    let short: String = memory_id.chars().take(8).collect();
    for spec in overrides {
        match citations.iter().filter(|c| c.path == spec.path).count() {
            0 => bail!(
                "this memory doesn't cite {}; add it with `memctrl cite {short} {}`",
                spec.path,
                spec.path
            ),
            1 => {}
            _ => bail!(
                "this memory cites {} more than once; re-cite it with `memctrl cite`",
                spec.path
            ),
        }
    }
    let mut plans = Vec::new();
    for citation in &citations {
        let status = check(root, citation);
        let explicit = overrides.iter().find(|s| s.path == citation.path);
        let (line_start, line_end) = match (explicit, status, citation.line_start, citation.line_end)
        {
            (Some(spec), ..) => (spec.line_start, spec.line_end),
            (_, Status::Missing, ..) => bail!(
                "{} no longer exists; supersede or forget this memory instead of refreshing it",
                citation.path
            ),
            (_, Status::Unresolved | Status::Unchecked, ..) => {
                bail!(
                    "{} cannot be checked inside the project root",
                    citation.path
                )
            }
            (_, Status::Moved { line_start }, Some(start), Some(end)) => {
                (Some(line_start), Some(line_start + (end - start)))
            }
            (_, Status::Changed, Some(start), Some(end)) => match unique_head(root, citation) {
                Some(begin) => (Some(begin), Some(begin + (end - start))),
                None => bail!(
                    "{}:{} changed and the cited block can't be found unambiguously, so refreshing could anchor this memory to unrelated code. Check the code, then run `memctrl refresh {short} --cite {}:<start>-<end>`",
                    citation.path,
                    range_label(Some(start), Some(end)).unwrap_or_default(),
                    citation.path
                ),
            },
            (_, _, start, end) => (start, end),
        };
        let spec = CitationSpec {
            path: citation.path.clone(),
            line_start,
            line_end,
        };
        let anchor = anchor(root, root, &spec).with_context(|| {
            format!(
                "cannot re-anchor {}; cite the current lines with `memctrl cite`",
                citation.path
            )
        })?;
        let from = range_label(citation.line_start, citation.line_end);
        plans.push((
            citation.id.clone(),
            status,
            from,
            explicit.is_some(),
            anchor,
        ));
    }
    let commit = current_commit(root);
    let guard = crate::store::AtomicWrite::new(&store.conn)?;
    let mut refreshed = Vec::new();
    for (id, before, from, explicit, anchor) in &plans {
        store.reanchor_citation(id, anchor, commit.as_deref())?;
        let lines = range_label(anchor.line_start, anchor.line_end);
        let mut entry =
            serde_json::json!({"path": anchor.path, "was": before.label(), "lines": lines});
        if lines != *from {
            entry["from"] = serde_json::json!(from);
            entry["followed"] = serde_json::json!(!explicit);
        }
        refreshed.push(entry);
    }
    let moves: Vec<String> = plans
        .iter()
        .filter(|(_, _, from, _, a)| *from != range_label(a.line_start, a.line_end))
        .map(|(_, _, from, _, a)| {
            format!(
                "{} {} → {}",
                a.path,
                from.clone().unwrap_or_default(),
                range_label(a.line_start, a.line_end).unwrap_or_default()
            )
        })
        .collect();
    store.log_event(
        "citation_refresh",
        &if moves.is_empty() {
            format!("re-anchored {} citation(s) after review", plans.len())
        } else {
            format!(
                "re-anchored {} citation(s) after review: {}",
                plans.len(),
                moves.join(", ")
            )
        },
        &[memory_id.to_string()],
    )?;
    guard.commit()?;
    Ok(serde_json::json!({"id": memory_id, "refreshed": refreshed}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn citation(anchor: &Anchor) -> Citation {
        Citation {
            id: "c".into(),
            memory_id: "m".into(),
            path: anchor.path.clone(),
            line_start: anchor.line_start,
            line_end: anchor.line_end,
            file_hash: anchor.file_hash.clone(),
            span_hash: anchor.span_hash.clone(),
            head_hash: anchor.head_hash.clone(),
            head_offset: anchor.head_offset,
            commit_sha: None,
            created_at: "now".into(),
            refreshed_at: None,
        }
    }

    #[test]
    fn specs_parse_lines_and_reject_escapes() {
        assert_eq!(
            parse_spec("src/db.rs:12-40").unwrap(),
            CitationSpec {
                path: "src/db.rs".into(),
                line_start: Some(12),
                line_end: Some(40)
            }
        );
        assert_eq!(parse_spec("./docs/a.md:7").unwrap().line_end, Some(7));
        assert_eq!(parse_spec("Makefile").unwrap().line_start, None);
        assert_eq!(parse_spec("weird:name.md").unwrap().path, "weird:name.md");
        for allowed in [".env.example", "deploy/id_rsa.pub", "config/app.toml"] {
            assert!(parse_spec(allowed).is_ok(), "{allowed}");
        }
        for bad in [
            ".env",
            "config/.env.production",
            "certs/server.pem",
            "home/.ssh/config",
            "infra/terraform.tfstate",
            ".npmrc",
            "",
            "/etc/passwd",
            "../outside.rs",
            "src/../../x",
            "C:/Windows/x",
            "a.rs:0",
            "a.rs:9-3",
            "a.rs:1-5000",
        ] {
            assert!(parse_spec(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn freshness_tracks_edits_moves_and_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let file = root.join("lib.rs");
        std::fs::write(
            &file,
            "fn a() {}\nfn get_by_id() {\n    None\n}\nfn z() {}\n",
        )
        .unwrap();
        let spec = parse_spec("lib.rs:2-4").unwrap();
        let anchor = anchor(root, root, &spec).unwrap();
        let c = citation(&anchor);
        assert_eq!(check(root, &c), Status::Fresh);
        // Trailing whitespace and CRLF don't count as changes.
        std::fs::write(
            &file,
            "fn a() {}\r\nfn get_by_id() {  \r\n    None\r\n}\r\nfn z() {}\r\n",
        )
        .unwrap();
        assert_eq!(check(root, &c), Status::Fresh);
        // An edit elsewhere leaves a line-range citation fresh.
        std::fs::write(
            &file,
            "fn a() { 1 }\nfn get_by_id() {\n    None\n}\nfn z() {}\n",
        )
        .unwrap();
        assert_eq!(check(root, &c), Status::Fresh);
        // The same lines pushed down by new code: moved, not stale.
        std::fs::write(
            &file,
            "use x;\n\nfn a() {}\nfn get_by_id() {\n    None\n}\n",
        )
        .unwrap();
        assert_eq!(check(root, &c), Status::Moved { line_start: 4 });
        // The cited body changed.
        std::fs::write(&file, "fn a() {}\nfn get_by_id() {\n    Some(1)\n}\n").unwrap();
        assert_eq!(check(root, &c), Status::Changed);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(check(root, &c), Status::Missing);
        assert_eq!(freshness(&[Status::Fresh, Status::Missing]), Some("stale"));
        assert_eq!(
            freshness(&[Status::Fresh, Status::Moved { line_start: 2 }]),
            Some("fresh")
        );
        assert_eq!(freshness(&[Status::Unchecked]), Some("unknown"));
        assert_eq!(freshness(&[]), None);
    }

    #[test]
    fn spans_starting_with_blank_lines_are_found_after_moving() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let body = "\n\nfn retry() {\n    backoff();\n}\n";
        std::fs::write(root.join("r.rs"), format!("// a\n{body}")).unwrap();
        let c = citation(&anchor(root, root, &parse_spec("r.rs:2-6").unwrap()).unwrap());
        assert_eq!(
            c.head_offset,
            Some(2),
            "anchored on the first non-blank line"
        );
        let filler = "\n".repeat(5000);
        std::fs::write(root.join("r.rs"), format!("// a\n// b\n{filler}{body}")).unwrap();
        assert_eq!(check(root, &c), Status::Moved { line_start: 5003 });
    }

    #[test]
    fn whole_file_citations_change_with_any_edit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("Makefile"), "ci:\n\tcargo test\n").unwrap();
        let c = citation(&anchor(root, root, &parse_spec("Makefile").unwrap()).unwrap());
        assert_eq!(check(root, &c), Status::Fresh);
        std::fs::write(root.join("Makefile"), "ci:\n\tcargo nextest run\n").unwrap();
        assert_eq!(check(root, &c), Status::Changed);
    }

    #[test]
    fn anchors_resolve_from_subdirectories_and_refuse_outside_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/db")).unwrap();
        std::fs::write(root.join("src/db/mod.rs"), "pub fn connect() {}\n").unwrap();
        let from_sub =
            anchor(root, &root.join("src"), &parse_spec("db/mod.rs:1").unwrap()).unwrap();
        assert_eq!(from_sub.path, "src/db/mod.rs");
        assert!(anchor(root, root, &parse_spec("src/db/missing.rs").unwrap()).is_err());
        assert!(anchor(root, root, &parse_spec("src/db/mod.rs:5").unwrap()).is_err());
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("x.rs"), "x").unwrap();
            std::os::unix::fs::symlink(outside.path().join("x.rs"), root.join("link.rs")).unwrap();
            assert!(anchor(root, root, &parse_spec("link.rs").unwrap()).is_err());
            let mut c =
                citation(&anchor(root, root, &parse_spec("src/db/mod.rs").unwrap()).unwrap());
            c.path = "link.rs".into();
            assert_eq!(check(root, &c), Status::Unresolved);
        }
    }

    #[test]
    fn digests_survive_output_sanitizing() {
        let d = digest("anything");
        assert_eq!(d.len(), 32);
        assert_eq!(crate::sanitize::sanitize_text(&d), d);
    }
}
