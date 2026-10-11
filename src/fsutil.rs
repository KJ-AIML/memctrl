//! Filesystem helpers for commands that write files people read and commit.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Writes through a sibling temp file and a rename, so readers never see a half-written file and
/// a failed write leaves the original intact. A symlinked target (for example AGENTS.md pointing
/// at CLAUDE.md) is resolved first, so the link survives and its target is updated.
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<PathBuf> {
    let target = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::canonicalize(path)
            .with_context(|| format!("cannot resolve symlink {}", path.display()))?,
        _ => path.to_path_buf(),
    };
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = target
        .file_name()
        .context("path has no file name")?
        .to_string_lossy();
    let temp = parent.join(format!(".{name}.memctrl-{}.tmp", std::process::id()));
    std::fs::write(&temp, contents)?;
    if let Err(error) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        return Err(error).with_context(|| format!("cannot replace {}", target.display()));
    }
    Ok(target)
}

/// Replaces everything from `begin` through `end` with `block` (which carries its own markers),
/// or appends `block` after a blank line when the markers are absent or out of order.
pub fn upsert_block(existing: &str, begin: &str, end: &str, block: &str) -> String {
    if let Some(start) = existing.find(begin) {
        if let Some(offset) = existing[start..].find(end) {
            let stop = start + offset + end.len();
            return format!("{}{}{}", &existing[..start], block, &existing[stop..]);
        }
    }
    if existing.trim().is_empty() {
        return format!("{block}\n");
    }
    let separator = if existing.ends_with("\n\n") {
        ""
    } else if existing.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{existing}{separator}{block}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_replaces_only_the_marked_block() {
        let block = "<!-- b -->\nnew\n<!-- e -->";
        assert_eq!(
            upsert_block("", "<!-- b -->", "<!-- e -->", block),
            format!("{block}\n")
        );
        assert_eq!(
            upsert_block("# Title\n", "<!-- b -->", "<!-- e -->", block),
            format!("# Title\n\n{block}\n")
        );
        let existing = "# Title\n\n<!-- b -->\nold\n<!-- e -->\n\nTail\n";
        assert_eq!(
            upsert_block(existing, "<!-- b -->", "<!-- e -->", block),
            format!("# Title\n\n{block}\n\nTail\n")
        );
        // An end marker before the begin marker is not a block: append instead of corrupting.
        let reversed = "<!-- e -->\n<!-- b -->\n";
        assert!(upsert_block(reversed, "<!-- b -->", "<!-- e -->", block)
            .ends_with(&format!("{block}\n")));
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_updates_a_symlink_target_and_keeps_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        let link = dir.path().join("AGENTS.md");
        std::fs::write(&target, "old").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        write_atomic(&link, b"new").unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    }
}
