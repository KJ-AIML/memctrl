//! TOML retention configuration and deterministic extraction/trigger rules.
use crate::model::NewMemory;
use crate::sanitize::{has_secrets, sanitize_text, validate_identifier};
use crate::store::{AtomicWrite, MemoryStore};
use anyhow::{bail, Context, Result};
use chrono::{Duration, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

pub const DEFAULT_RULES: &str = r#"[layers]
project = "architecture decisions, tech stack, ADRs, why we chose X"
session = "current task, WIP, what was done this session"
user = "preferences, working style, patterns, personal rules"
[triggers]
on_commit = "consolidate session -> project"
on_session_end = "summarize session -> user"
'on_file "docs/ADR-*.md"' = "extract -> project"
'on_file "*.md"' = "extract -> project if contains decision"
[forget]
never = ["passwords", "keys", "PII", "secrets", "api_key", "token", "secret", "password"]
after_days = { session = 7, user = 90 }
[extract]
confidence = { explicit = 1.0, inferred = 0.7, mentioned = 0.5 }
[retrieval]
# Words that should find each other. Built-ins cover db/database, prod/production, k8s/kubernetes...
builtin_aliases = true
aliases = []   # e.g. [["orm", "sqlx", "diesel"], ["ci", "github actions"]]
[guard]
# Hidden characters or instruction-like text make a new memory a candidate for review.
# "strict" also holds everything from sources not listed below; "off" disables the guard.
mode = "flag"
trusted_sources = ["manual", "user", "cli"]
"#;

/// Synonym groups developers use interchangeably. A query word in a group also matches the
/// group's other members. Disable with `[retrieval] builtin_aliases = false`.
pub const BUILTIN_ALIASES: &[&[&str]] = &[
    &["db", "database"],
    &["auth", "authentication"],
    &["prod", "production"],
    &["k8s", "kubernetes"],
    &["repo", "repository"],
    &["config", "configuration"],
    &["env", "environment"],
    &["deps", "dependencies"],
    &["js", "javascript"],
    &["ts", "typescript"],
    &["pg", "postgres", "postgresql"],
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rules {
    pub layers: BTreeMap<String, String>,
    pub triggers: BTreeMap<String, String>,
    pub forget_never: Vec<String>,
    pub forget_after_days: BTreeMap<String, i64>,
    pub confidence: BTreeMap<String, f64>,
    pub db_path: Option<String>,
    /// Custom synonym groups from `[retrieval] aliases`.
    pub aliases: Vec<Vec<String>>,
    pub builtin_aliases: bool,
    /// `[citations] root`, resolved against the rules file's directory.
    pub citations_root: Option<std::path::PathBuf>,
    /// `[guard]`: how suspicious writes are held for review.
    pub guard: crate::guard::Policy,
    /// The project folder the command runs in (set by the caller, never read from the file).
    #[serde(skip)]
    pub project_dir: Option<std::path::PathBuf>,
    /// Settings from the rules file that were ignored because they would loosen defenses.
    #[serde(skip)]
    pub warnings: Vec<String>,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            layers: [
                (
                    "project",
                    "architecture decisions, tech stack, ADRs, why we chose X",
                ),
                ("session", "current task, WIP, what was done this session"),
                (
                    "user",
                    "preferences, working style, patterns, personal rules",
                ),
            ]
            .into_iter()
            .map(|(a, b)| (a.into(), b.into()))
            .collect(),
            triggers: [
                ("on_commit", "consolidate session -> project"),
                ("on_session_end", "summarize session -> user"),
                ("on_file \"docs/ADR-*.md\"", "extract -> project"),
                (
                    "on_file \"*.md\"",
                    "extract -> project if contains decision",
                ),
            ]
            .into_iter()
            .map(|(a, b)| (a.into(), b.into()))
            .collect(),
            forget_never: [
                "passwords",
                "keys",
                "PII",
                "secrets",
                "api_key",
                "token",
                "secret",
                "password",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            forget_after_days: [("session".into(), 7), ("user".into(), 90)]
                .into_iter()
                .collect(),
            confidence: [
                ("explicit".into(), 1.0),
                ("inferred".into(), 0.7),
                ("mentioned".into(), 0.5),
            ]
            .into_iter()
            .collect(),
            db_path: None,
            aliases: Vec::new(),
            builtin_aliases: true,
            citations_root: None,
            guard: crate::guard::Policy::default(),
            project_dir: None,
            warnings: Vec::new(),
        }
    }
}

/// Settings only you make, in `$XDG_CONFIG_HOME/memctrl/config.toml` or
/// `~/.config/memctrl/config.toml` (`%APPDATA%\memctrl\config.toml` on Windows). A repository's
/// `.memoryrc` arrives with every clone, so it can only tighten what this file allows:
///
/// ```toml
/// [guard]
/// mode = "flag"                      # the least protective mode a repository may use
/// trusted_sources = ["manual", "user", "cli"]
/// [trust]
/// repos = ["~/code/my-team-repo"]    # repositories whose .memoryrc applies in full
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserConfig {
    pub guard_mode: Option<crate::guard::Mode>,
    pub trusted_sources: Option<Vec<String>>,
    /// Repository roots whose `.memoryrc` (and committed database) are trusted, resolved.
    pub trusted_repos: Vec<PathBuf>,
    /// Where the file is (or would be).
    pub path: Option<PathBuf>,
}

impl UserConfig {
    pub fn default_path() -> Option<PathBuf> {
        let dir = if cfg!(windows) {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        };
        dir.map(|dir| dir.join("memctrl").join("config.toml"))
    }

    /// Your config, or defaults when there is none. An invalid file is an error.
    pub fn load() -> Result<Self> {
        match Self::default_path() {
            Some(path) => Self::load_from(&path),
            None => Ok(Self::default()),
        }
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let mut config = Self {
            path: Some(path.to_path_buf()),
            ..Self::default()
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(config),
            Err(error) => return Err(error.into()),
        };
        let raw: toml::Value =
            toml::from_str(&text).with_context(|| format!("invalid TOML in {}", path.display()))?;
        if let Some(guard) = raw.get("guard") {
            let guard = guard.as_table().context("guard must be a table")?;
            if let Some(mode) = guard.get("mode") {
                config.guard_mode = Some(parse_mode(mode)?);
            }
            if let Some(sources) = guard.get("trusted_sources") {
                config.trusted_sources = Some(parse_sources(sources)?);
            }
        }
        if let Some(repos) = raw.get("trust").and_then(|t| t.get("repos")) {
            let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
            for repo in repos
                .as_array()
                .context("trust.repos must be an array of paths")?
            {
                let repo = repo
                    .as_str()
                    .context("trust.repos entries must be strings")?;
                let repo = match (repo.strip_prefix("~/"), &home) {
                    (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
                    _ => PathBuf::from(repo),
                };
                if !repo.is_absolute() {
                    bail!(
                        "trust.repos entries must be absolute paths: {}",
                        repo.display()
                    );
                }
                config
                    .trusted_repos
                    .push(repo.canonicalize().unwrap_or(repo));
            }
        }
        Ok(config)
    }

    /// True when the repository that holds `path` is listed under `[trust] repos`.
    pub fn trusts(&self, path: &Path) -> bool {
        !self.trusted_repos.is_empty() && self.trusted_repos.contains(&repository_root(path))
    }

    fn describe(&self) -> String {
        self.path
            .as_ref()
            .map_or("your user config".into(), |p| p.display().to_string())
    }
}

/// The repository a file belongs to: the nearest ancestor of its folder containing `.git`, else
/// the folder itself. Resolved, so it compares with `[trust] repos`.
pub fn repository_root(path: &Path) -> PathBuf {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    dir.ancestors()
        .find(|d| d.join(".git").exists())
        .map_or(dir.clone(), Path::to_path_buf)
}

fn parse_mode(value: &toml::Value) -> Result<crate::guard::Mode> {
    Ok(
        match value.as_str().context("guard.mode must be a string")? {
            "off" => crate::guard::Mode::Off,
            "flag" => crate::guard::Mode::Flag,
            "strict" => crate::guard::Mode::Strict,
            _ => bail!("guard.mode must be \"off\", \"flag\" or \"strict\""),
        },
    )
}

fn parse_sources(value: &toml::Value) -> Result<Vec<String>> {
    value
        .as_array()
        .context("guard.trusted_sources must be an array of strings")?
        .iter()
        .map(|v| {
            let source = v
                .as_str()
                .context("guard.trusted_sources entries must be strings")?;
            validate_identifier(source, "trusted source")?;
            Ok(source.to_string())
        })
        .collect()
}

/// A consolidation trigger that moves memories out of `user`, which holds personal notes and is
/// never exported, into a layer that is.
fn moves_personal_notes(action: &str) -> bool {
    let pattern = Regex::new(r"(?i)^\s*(?:consolidate|summarize)\s+(\w+)\s*-+>\s*(\w+)\s*$")
        .expect("valid regex");
    pattern
        .captures(action)
        .is_some_and(|c| c[1].eq_ignore_ascii_case("user") && !c[2].eq_ignore_ascii_case("user"))
}

impl Rules {
    /// Loads a rules file under your user config (see `UserConfig`).
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::load_with(path, &UserConfig::load()?)
    }

    /// Loads a rules file. Unless `user` trusts its repository, settings that would loosen
    /// defenses are ignored and recorded in `warnings`.
    pub fn load_with(path: impl AsRef<Path>, user: &UserConfig) -> Result<Self> {
        let path = path.as_ref();
        let mut base = Self::default();
        if let Some(mode) = user.guard_mode {
            base.guard.mode = mode;
        }
        if let Some(sources) = &user.trusted_sources {
            base.guard.trusted_sources = sources.clone();
        }
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(base),
            Err(error) => return Err(error.into()),
        };
        let mut rules = Self::parse(path, &text, base.clone())?;
        if !user.trusts(path) {
            rules.only_tighten(&base, path, user);
        }
        Ok(rules)
    }

    /// A repository's rules may only tighten `base`: the guard stays at least as strict,
    /// trusted sources only narrow, `forget.never` only grows, paths stay inside the
    /// repository, and no trigger moves personal notes into a shared layer.
    fn only_tighten(&mut self, base: &Rules, path: &Path, user: &UserConfig) {
        let mut ignored = Vec::new();
        if self.guard.mode < base.guard.mode {
            ignored.push(format!("[guard] mode = {:?}", self.guard.mode));
            self.guard.mode = base.guard.mode;
        }
        let extra: Vec<String> = self
            .guard
            .trusted_sources
            .iter()
            .filter(|s| !base.guard.trusted_sources.contains(s))
            .cloned()
            .collect();
        if !extra.is_empty() {
            ignored.push(format!("[guard] trusted_sources {extra:?}"));
            self.guard
                .trusted_sources
                .retain(|s| base.guard.trusted_sources.contains(s));
        }
        let missing: Vec<String> = base
            .forget_never
            .iter()
            .filter(|w| !self.forget_never.contains(w))
            .cloned()
            .collect();
        if !missing.is_empty() {
            ignored.push(format!("removing {missing:?} from [forget] never"));
            self.forget_never.extend(missing);
        }
        let repo = repository_root(path);
        if let Some(root) = &self.citations_root {
            if root.canonicalize().is_ok_and(|r| !r.starts_with(&repo)) {
                ignored.push(format!(
                    "[citations] root = {:?} (outside the repository)",
                    root
                ));
                self.citations_root = None;
            }
        }
        if let Some(db) = &self.db_path {
            let db_path = Path::new(db);
            if db_path.is_absolute()
                || db_path
                    .components()
                    .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
            {
                ignored.push(format!(
                    "[memctrl] db_path = {db:?} (outside the repository)"
                ));
                self.db_path = None;
            }
        }
        let leaks: Vec<String> = self
            .triggers
            .iter()
            .filter(|(_, action)| moves_personal_notes(action))
            .map(|(key, _)| key.clone())
            .collect();
        for key in leaks {
            ignored.push(format!(
                "[triggers] {key} (it moves personal user-layer notes into a shared layer)"
            ));
            self.triggers.remove(&key);
        }
        for setting in ignored {
            self.warnings.push(format!(
                "ignored {setting} from {}: a repository's .memoryrc can only tighten memctrl's defenses. To allow it, set it in {}, or list this repository under [trust] repos there",
                path.display(),
                user.describe()
            ));
        }
    }

    fn parse(path: &Path, text: &str, base: Rules) -> Result<Self> {
        let raw: toml::Value = toml::from_str(text).context("invalid .memoryrc TOML")?;
        let mut rules = base;
        if let Some(layers) = raw.get("layers") {
            for (key, value) in layers.as_table().context("layers must be a table")? {
                validate_identifier(key, "layer")?;
                rules.layers.insert(
                    key.clone(),
                    value
                        .as_str()
                        .context("layer description must be a string")?
                        .into(),
                );
            }
        }
        if let Some(triggers) = raw.get("triggers") {
            for (key, value) in triggers.as_table().context("triggers must be a table")? {
                if let Some(action) = value.as_str() {
                    // Flat on_file = "*.md -> extract -> project" also works.
                    if key == "on_file" && !action.trim_start().starts_with("extract") {
                        if let Some((glob, action)) = action.split_once("->") {
                            rules.triggers.insert(
                                format!("on_file \"{}\"", glob.trim()),
                                action.trim().into(),
                            );
                        } else {
                            bail!("on_file must specify a pattern and action");
                        }
                    } else {
                        rules.triggers.insert(key.clone(), action.into());
                    }
                } else if let Some(table) = value.as_table() {
                    for (glob, action) in table {
                        rules.triggers.insert(
                            format!("{key} \"{glob}\""),
                            action
                                .as_str()
                                .context("trigger action must be a string")?
                                .into(),
                        );
                    }
                } else {
                    bail!("trigger action must be a string or table");
                }
            }
        }
        if let Some(forget) = raw.get("forget") {
            let forget = forget.as_table().context("forget must be a table")?;
            if let Some(never) = forget.get("never") {
                rules.forget_never = never
                    .as_array()
                    .context("forget.never must be an array")?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(String::from)
                            .context("forget.never entries must be strings")
                    })
                    .collect::<Result<_>>()?;
            }
            if let Some(days) = forget.get("after_days") {
                rules.forget_after_days.clear();
                for (layer, days) in days
                    .as_table()
                    .context("forget.after_days must be a table")?
                {
                    validate_identifier(layer, "layer")?;
                    let days = days
                        .as_integer()
                        .context("retention days must be integers")?;
                    if !(0..=365_000).contains(&days) {
                        bail!("retention days must be between 0 and 365000");
                    }
                    rules.forget_after_days.insert(layer.clone(), days);
                }
            }
        }
        if let Some(confidence) = raw.get("extract").and_then(|v| v.get("confidence")) {
            for (level, value) in confidence
                .as_table()
                .context("extract.confidence must be a table")?
            {
                let value = value
                    .as_float()
                    .or_else(|| value.as_integer().map(|i| i as f64))
                    .context("confidence must be numeric")?;
                crate::model::validate_confidence(value)?;
                rules.confidence.insert(level.clone(), value);
            }
        }
        if let Some(retrieval) = raw.get("retrieval") {
            let retrieval = retrieval.as_table().context("retrieval must be a table")?;
            if let Some(flag) = retrieval.get("builtin_aliases") {
                rules.builtin_aliases = flag
                    .as_bool()
                    .context("retrieval.builtin_aliases must be true or false")?;
            }
            if let Some(groups) = retrieval.get("aliases") {
                let groups = groups
                    .as_array()
                    .context("retrieval.aliases must be an array of arrays of strings")?;
                if groups.len() > 500 {
                    bail!("retrieval.aliases allows at most 500 groups");
                }
                for group in groups {
                    let words = group
                        .as_array()
                        .context("each retrieval.aliases group must be an array of strings")?;
                    let mut cleaned = Vec::new();
                    for word in words {
                        let word = word
                            .as_str()
                            .context("alias entries must be strings")?
                            .trim()
                            .to_lowercase();
                        if word.is_empty() || word.chars().count() > 64 {
                            bail!("alias entries must be 1 to 64 characters");
                        }
                        if has_secrets(&word) {
                            bail!("alias entries must not contain credentials");
                        }
                        if !cleaned.contains(&word) {
                            cleaned.push(word);
                        }
                    }
                    if cleaned.len() < 2 {
                        bail!("each retrieval.aliases group needs at least two distinct words");
                    }
                    rules.aliases.push(cleaned);
                }
            }
        }
        if let Some(guard) = raw.get("guard") {
            let guard = guard.as_table().context("guard must be a table")?;
            if let Some(mode) = guard.get("mode") {
                rules.guard.mode = parse_mode(mode)?;
            }
            if let Some(sources) = guard.get("trusted_sources") {
                rules.guard.trusted_sources = parse_sources(sources)?;
            }
        }
        if let Some(root) = raw.get("citations").and_then(|v| v.get("root")) {
            let root = root.as_str().context("citations.root must be a string")?;
            let base = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            rules.citations_root = Some(base.join(root));
        }
        if let Some(path) = raw.get("memctrl").and_then(|v| v.get("db_path")) {
            rules.db_path = Some(path.as_str().context("db_path must be a string")?.into());
        }
        Ok(rules)
    }

    /// Every active synonym group: built-ins (unless disabled) followed by custom groups.
    pub fn alias_groups(&self) -> Vec<Vec<String>> {
        let mut groups: Vec<Vec<String>> = if self.builtin_aliases {
            BUILTIN_ALIASES
                .iter()
                .map(|g| g.iter().map(|w| w.to_string()).collect())
                .collect()
        } else {
            Vec::new()
        };
        groups.extend(self.aliases.iter().cloned());
        groups
    }

    pub fn expiry(&self, layer: &str) -> Option<String> {
        self.forget_after_days
            .get(layer)
            .and_then(|days| Utc::now().checked_add_signed(Duration::days(*days)))
            .map(|dt| dt.to_rfc3339())
    }

    pub fn extract(&self, text: &str, layer: &str) -> Vec<NewMemory> {
        if validate_identifier(layer, "layer").is_err() {
            return Vec::new();
        }
        let patterns = [
            (
                r"(?i)we\s+(?:use|used|chose|decided|migrated|switched|implemented)\s+.+",
                "explicit",
                "tech_choice",
            ),
            (r"(?i)adr[-\s]?\d+\s*[:\-]?\s*.+", "explicit", "adr"),
            (r"(?i)decided\s+to\s+.+", "explicit", "decision"),
            (
                r"(?i)migrated?\s+(?:from\s+)?\w+\s+to\s+\w+.+",
                "explicit",
                "migration",
            ),
            (r"(?i)^\s*(?:import|from)\s+\w+.*", "inferred", "dependency"),
            (
                r"(?i)(?:built|written|developed)\s+(?:with|on|using)\s+\w+.*",
                "inferred",
                "framework",
            ),
            (
                r"(?i)(?:prefer|like|always|never)\s+.+",
                "explicit",
                "preference",
            ),
            (
                r"(?i)(?:consider|considering|evaluating|looking at|might|maybe|suggested|proposed|idea)\b.+",
                "mentioned",
                "mentioned",
            ),
        ];
        let patterns: Vec<_> = patterns
            .into_iter()
            .map(|(p, l, t)| (Regex::new(p).expect("static extraction regex"), l, t))
            .collect();
        let mut seen = BTreeSet::new();
        let mut results = Vec::new();
        for line in text.lines() {
            // Scan the complete line before truncation so long secrets cannot evade detection.
            if has_secrets(line)
                || self
                    .forget_never
                    .iter()
                    .any(|p| line.to_lowercase().contains(&p.to_lowercase()))
            {
                continue;
            }
            let line = sanitize_text(line);
            let line: String = line.trim().chars().take(500).collect();
            if line.chars().count() < 10 {
                continue;
            }
            for (pattern, level, tag) in &patterns {
                if let Some(matched) = pattern.find(&line) {
                    let content = matched
                        .as_str()
                        .trim_matches(|c: char| ". ;,\t".contains(c))
                        .to_string();
                    if content.chars().count() < 10 {
                        continue;
                    }
                    let key = content.chars().take(50).collect::<String>().to_lowercase();
                    if seen.insert(key) {
                        results.push(NewMemory {
                            layer: layer.into(),
                            content,
                            source: "heuristic_extract".into(),
                            confidence: self.confidence.get(*level).copied().unwrap_or(0.5),
                            expires_at: self.expiry(layer),
                            tags: vec![layer.into(), (*tag).into(), (*level).into()],
                            ..Default::default()
                        });
                    }
                    break;
                }
            }
        }
        results
    }

    pub fn fire_trigger(
        &self,
        event: &str,
        context: &Value,
        store: &mut MemoryStore,
    ) -> Result<Vec<String>> {
        validate_identifier(event, "event")?;
        if !context.is_object() {
            bail!("trigger context must be an object");
        }
        let consolidate = Regex::new(r"(?i)^\s*(consolidate|summarize)\s+(\w+)\s*-+>\s*(\w+)\s*$")?;
        let extract = Regex::new(r"(?i)^\s*extract\s*-+>\s*(\w+)\s*$")?;
        let mut actions: Vec<(String, String, String, String)> = Vec::new();
        for (pattern, action) in &self.triggers {
            let (name, glob) = pattern
                .split_once(' ')
                .map(|(a, b)| (a, Some(b.trim().trim_matches('"').trim_matches('\''))))
                .unwrap_or((pattern, None));
            if name.to_lowercase() != event.to_lowercase() {
                continue;
            }
            if let Some(glob) = glob {
                let path = context
                    .get("path")
                    .or_else(|| context.get("file"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let glob_regex = format!(
                    "^{}$",
                    regex::escape(glob).replace("\\*", ".*").replace("\\?", ".")
                );
                if !Regex::new(&glob_regex)?.is_match(path) {
                    continue;
                }
            }
            let (action, condition) = action
                .split_once(" if ")
                .map(|(a, b)| (a, Some(b)))
                .unwrap_or((action, None));
            if let Some(condition) = condition {
                let needle = condition
                    .trim()
                    .strip_prefix("contains ")
                    .context("unsupported trigger condition")?
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'');
                let text = context
                    .get("text")
                    .or_else(|| context.get("content"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !text.to_lowercase().contains(&needle.to_lowercase()) {
                    continue;
                }
            }
            let parsed = consolidate.captures(action);
            if let Some(parsed) = parsed {
                validate_identifier(&parsed[2], "layer")?;
                validate_identifier(&parsed[3], "layer")?;
                actions.push((
                    parsed[1].to_lowercase(),
                    parsed[2].into(),
                    parsed[3].into(),
                    action.to_string(),
                ));
            } else if let Some(parsed) = extract.captures(action) {
                validate_identifier(&parsed[1], "layer")?;
                actions.push((
                    "extract".into(),
                    String::new(),
                    parsed[1].into(),
                    action.to_string(),
                ));
            } else {
                bail!("unsupported trigger action");
            }
        }
        // A malformed later action cannot leave earlier actions partially applied.
        let store: &MemoryStore = store;
        let guard = AtomicWrite::new(&store.conn)?;
        let result = (|| -> Result<Vec<String>> {
            let mut ids = Vec::new();
            for (verb, from, to, action) in actions {
                if verb == "extract" {
                    let text = context
                        .get("text")
                        .or_else(|| context.get("content"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    for mut memory in self.extract(text, &to) {
                        crate::guard::apply(&mut memory, &self.guard);
                        ids.push(store.insert_memory(memory)?);
                    }
                    store.maintenance(
                        "trigger",
                        serde_json::json!({"event":event,"action":action}),
                    )?;
                } else {
                    ids.extend(store.consolidate_and_log(&from, &to, event, &action)?);
                }
            }
            ids.sort();
            ids.dedup();
            Ok(ids)
        })();
        let ids = result?;
        guard.commit()?;
        Ok(ids)
    }
}
