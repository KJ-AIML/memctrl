//! The checkpoint: where work on a branch left off, one JSON file per branch, written by hooks
//! and shown to the next session.
use crate::fsutil;
use crate::gitmeta::{self, Head};
use crate::sanitize::sanitize_text;
use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const VERSION: u32 = 1;
pub const MAX_PROMPTS: usize = 3;
pub const MIN_PROMPT_CHARS: usize = 12;
pub const PROMPT_CHARS: usize = 500;
pub const MESSAGE_CHARS: usize = 2_000;
pub const SUMMARY_CHARS: usize = 6_000;
pub const NOTE_CHARS: usize = 300;
pub const MAX_FILES: usize = 20;
pub const MAX_NOTES: usize = 10;

/// Characters of the sanitized branch name kept in a key, before its hash.
const KEY_NAME_CHARS: usize = 60;
const CLIP_MARK: &str = " … ";

/// One stored piece of text and when it was stored.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub at: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Checkpoint {
    pub version: u32,
    /// `None` for a detached HEAD or a folder outside git.
    pub branch: Option<String>,
    pub updated_at: Option<String>,
    pub session_id: Option<String>,
    pub prompts: Vec<Part>,
    pub last_message: Option<Part>,
    pub compaction: Option<Part>,
    pub files: Vec<String>,
    pub files_at: Option<String>,
    pub notes: Vec<Part>,
}

/// `text` cut to `max` characters: its first third and its end, with " … " between.
pub fn clip_middle(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let room = max.saturating_sub(CLIP_MARK.chars().count());
    let head = room / 3;
    let tail = room - head;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(count - tail).collect();
    format!("{start}{CLIP_MARK}{end}")
}

fn short_hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A file name for a branch (see the spec's "Key").
pub fn key(head: Option<&Head>, worktree: &Path) -> String {
    match head {
        Some(Head::Branch(name)) => {
            let mut safe: String = name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || "._-".contains(c) {
                        c
                    } else {
                        '-'
                    }
                })
                .take(KEY_NAME_CHARS)
                .collect();
            if safe.starts_with('.') {
                safe.replace_range(..1, "-");
            }
            format!("{safe}-{}", short_hash(name))
        }
        _ => format!("_detached-{}", short_hash(&worktree.to_string_lossy())),
    }
}

/// The checkpoint file for the branch checked out in `worktree`, and the branch's name.
pub fn file(store_dir: &Path, worktree: &Path) -> (PathBuf, Option<String>) {
    let root = gitmeta::repo_root(worktree);
    let head = root.as_deref().and_then(gitmeta::head);
    let root = root.unwrap_or_else(|| worktree.to_path_buf());
    let name = format!("{}.json", key(head.as_ref(), &root));
    let branch = match head {
        Some(Head::Branch(name)) => Some(name),
        _ => None,
    };
    (store_dir.join("checkpoints").join(name), branch)
}

/// Missing, unreadable, not JSON, or a newer version: an empty checkpoint.
pub fn load(path: &Path) -> Checkpoint {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Checkpoint>(&text).ok())
        .filter(|checkpoint| checkpoint.version <= VERSION)
        .unwrap_or_default()
}

pub fn save(path: &Path, checkpoint: &Checkpoint) -> Result<()> {
    let dir = path.parent().context("a checkpoint lives in a folder")?;
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    fsutil::ignore_all(dir);
    let mut bytes = serde_json::to_vec_pretty(checkpoint)?;
    bytes.push(b'\n');
    fsutil::write_atomic(path, &bytes, Some(dir))?;
    Ok(())
}

fn timestamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Trimmed, sanitized and clipped; `None` when nothing is left.
fn part(text: &str, max: usize, now: DateTime<Utc>) -> Option<Part> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let text = clip_middle(sanitize_text(text).trim(), max);
    (!text.is_empty()).then(|| Part {
        at: timestamp(now),
        text,
    })
}

impl Checkpoint {
    fn touch(&mut self, now: DateTime<Utc>) {
        self.version = VERSION;
        self.updated_at = Some(timestamp(now));
    }

    /// Keeps a request unless it is a slash command or shorter than `MIN_PROMPT_CHARS`.
    pub fn record_prompt(&mut self, text: &str, now: DateTime<Utc>) -> bool {
        let text = text.trim();
        let substantial = text.chars().filter(|c| !c.is_whitespace()).count() >= MIN_PROMPT_CHARS;
        if text.starts_with('/') || !substantial {
            return false;
        }
        let Some(part) = part(text, PROMPT_CHARS, now) else {
            return false;
        };
        self.prompts.push(part);
        let excess = self.prompts.len().saturating_sub(MAX_PROMPTS);
        self.prompts.drain(..excess);
        self.touch(now);
        true
    }

    pub fn record_message(&mut self, text: &str, now: DateTime<Utc>) -> bool {
        let Some(part) = part(text, MESSAGE_CHARS, now) else {
            return false;
        };
        self.last_message = Some(part);
        self.touch(now);
        true
    }

    pub fn record_compaction(&mut self, text: &str, now: DateTime<Utc>) -> bool {
        let Some(part) = part(text, SUMMARY_CHARS, now) else {
            return false;
        };
        self.compaction = Some(part);
        self.touch(now);
        true
    }

    pub fn add_note(&mut self, text: &str, now: DateTime<Utc>) -> bool {
        let Some(part) = part(text, NOTE_CHARS, now) else {
            return false;
        };
        self.notes.push(part);
        let excess = self.notes.len().saturating_sub(MAX_NOTES);
        self.notes.drain(..excess);
        self.touch(now);
        true
    }

    /// Replaces the uncommitted files with the first `MAX_FILES` of `files`.
    pub fn record_files(&mut self, files: Vec<String>, now: DateTime<Utc>) {
        self.files = files
            .iter()
            .map(|file| sanitize_text(file.trim()))
            .filter(|file| !file.is_empty())
            .take(MAX_FILES)
            .collect();
        self.files_at = Some(timestamp(now));
        self.touch(now);
    }

    /// No prompts, message, compaction, files or notes.
    pub fn is_empty(&self) -> bool {
        self.prompts.is_empty()
            && self.last_message.is_none()
            && self.compaction.is_none()
            && self.files.is_empty()
            && self.notes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitmeta::Head;
    use chrono::TimeZone;

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 24, hour, 0, 0).unwrap()
    }
    fn branch(name: &str) -> String {
        key(Some(&Head::Branch(name.into())), Path::new("/w"))
    }

    #[test]
    fn a_key_is_one_safe_file_name() {
        assert!(branch("feat/auth").starts_with("feat-auth-"));
        assert_eq!(branch("feat/auth").len(), "feat-auth-".len() + 8);
        assert_ne!(branch("a/b"), branch("a-b"), "the hash tells them apart");
        for name in [
            "feat/../../x",
            ".hidden",
            "ฟีเจอร์/ล็อกอิน",
            &"x".repeat(200),
            "a\\b:c",
        ] {
            let key = branch(name);
            assert!(key.len() <= 69, "{key}");
            assert!(!key.starts_with('.'), "{key}");
            assert!(
                key.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
                "{key}"
            );
            // ".." inside one file name is harmless; the file test below proves the path stays
            // inside checkpoints/.
            assert!(key != "." && key != "..", "{key}");
        }
        let detached = key(Some(&Head::Detached("0123".into())), Path::new("/w/one"));
        assert!(detached.starts_with("_detached-"));
        assert_ne!(detached, key(None, Path::new("/w/two")));
    }

    #[test]
    fn clipping_keeps_the_start_and_the_end() {
        assert_eq!(clip_middle("short", 10), "short");
        let clipped = clip_middle(&format!("{}{}", "a".repeat(100), "z".repeat(100)), 30);
        assert_eq!(clipped.chars().count(), 30);
        assert_eq!(clipped, format!("{} … {}", "a".repeat(9), "z".repeat(18)));
        assert_eq!(
            clip_middle(&"ก".repeat(50), 12).chars().count(),
            12,
            "counts characters, not bytes"
        );
    }

    #[test]
    fn requests_keep_the_last_three_worth_keeping() {
        let mut c = Checkpoint::default();
        for skipped in ["ok", "/clear", "ต่อเลย", "   ", "/compact keep the plan"] {
            assert!(!c.record_prompt(skipped, at(1)), "{skipped:?}");
        }
        assert!(c.is_empty());
        for (hour, text) in [
            (1, "first request here"),
            (2, "second request here"),
            (3, "third request here"),
            (4, "fourth request here"),
        ] {
            assert!(c.record_prompt(text, at(hour)));
        }
        let kept: Vec<&str> = c.prompts.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(
            kept,
            [
                "second request here",
                "third request here",
                "fourth request here"
            ]
        );
        assert_eq!(c.updated_at.as_deref(), Some("2026-11-24T04:00:00Z"));
        assert_eq!(c.version, VERSION);
        assert!(c.record_prompt(&"long ".repeat(200), at(5)));
        assert_eq!(c.prompts[2].text.chars().count(), PROMPT_CHARS);
    }

    #[test]
    fn parts_are_redacted_and_bounded() {
        let mut c = Checkpoint::default();
        assert!(c.record_message(
            "Set DB_PASSWORD=hunter2hunter2 in .env, then run the migration.",
            at(1)
        ));
        assert!(!serde_json::to_string(&c)
            .unwrap()
            .contains("hunter2hunter2"));
        assert!(
            !c.record_message("   ", at(2)),
            "an empty reply keeps the last one"
        );
        assert!(c
            .last_message
            .as_ref()
            .unwrap()
            .text
            .contains("run the migration"));
        // Words, not one 9,000-letter run: a run of 40+ letters is redacted as a token.
        assert!(c.record_compaction(&"so ".repeat(3_000), at(3)));
        assert_eq!(
            c.compaction.as_ref().unwrap().text.chars().count(),
            SUMMARY_CHARS
        );
        for n in 0..12 {
            assert!(c.add_note(&format!("note number {n}"), at(4)));
        }
        assert_eq!(c.notes.len(), MAX_NOTES);
        assert_eq!(c.notes[0].text, "note number 2");
        c.record_files((0..30).map(|n| format!("src/f{n}.rs")).collect(), at(5));
        assert_eq!(c.files.len(), MAX_FILES);
        assert_eq!(c.files_at.as_deref(), Some("2026-11-24T05:00:00Z"));
    }

    #[test]
    fn a_damaged_file_reads_as_empty_and_saving_ignores_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join(".memctrl");
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git/HEAD"),
            "ref: refs/heads/feat/../../x\n",
        )
        .unwrap();
        let (path, name) = file(&store, dir.path());
        assert_eq!(name.as_deref(), Some("feat/../../x"));
        assert_eq!(
            path.parent(),
            Some(store.join("checkpoints").as_path()),
            "{path:?}"
        );
        assert_eq!(load(&path), Checkpoint::default(), "missing");

        let mut c = Checkpoint {
            branch: name,
            ..Default::default()
        };
        c.record_message("Where the agent stopped.", at(1));
        save(&path, &c).unwrap();
        assert_eq!(load(&path), c);
        assert_eq!(
            std::fs::read_to_string(store.join("checkpoints/.gitignore"))
                .unwrap()
                .lines()
                .last(),
            Some("*")
        );

        for damaged in [
            "",
            "{",
            "[]",
            r#"{"version": 99, "prompts": [{"at":"x","text":"from the future"}]}"#,
        ] {
            std::fs::write(&path, damaged).unwrap();
            assert_eq!(load(&path), Checkpoint::default(), "{damaged:?}");
        }
    }
}
