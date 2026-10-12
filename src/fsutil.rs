//! Filesystem helpers for commands that write files people read and commit.
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// The nearest existing ancestor of `path`, canonicalized (so symlinks along it are resolved).
fn existing_ancestor(path: &Path) -> Result<PathBuf> {
    let mut current = path;
    loop {
        if current.exists() {
            return Ok(current.canonicalize()?);
        }
        match current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => current = parent,
            _ => return Ok(std::env::current_dir()?.canonicalize()?),
        }
    }
}

/// Writes through a sibling temp file and a rename, so readers never see a half-written file and
/// a failed write leaves the original intact. The original file's permissions are kept.
///
/// A symlinked target (for example AGENTS.md pointing at CLAUDE.md) is resolved so the link
/// survives. With `confine`, the resolved target, including any symlinked directories on the
/// way, must stay inside that directory. This stops a cloned repository from aiming a committed
/// file at something like ~/.bashrc.
pub fn write_atomic(path: &Path, contents: &[u8], confine: Option<&Path>) -> Result<PathBuf> {
    let target = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::canonicalize(path)
            .with_context(|| format!("cannot resolve symlink {}", path.display()))?,
        _ => path.to_path_buf(),
    };
    if let Some(root) = confine {
        let root = root
            .canonicalize()
            .with_context(|| format!("cannot resolve {}", root.display()))?;
        let resolved = existing_ancestor(&target)?;
        if !resolved.starts_with(&root) {
            bail!(
                "refusing to write {}: it resolves outside {}",
                path.display(),
                root.display()
            );
        }
    }
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
    if let Ok(meta) = std::fs::metadata(&target) {
        std::fs::set_permissions(&temp, meta.permissions())?;
    }
    if let Err(error) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        return Err(error).with_context(|| format!("cannot replace {}", target.display()));
    }
    Ok(target)
}

/// Replaces the one block from `begin` through `end` with `block` (which carries its own
/// markers), or appends `block` after a blank line when neither marker is present. Anything else
/// (an orphan marker, several blocks, reversed markers) is refused rather than guessed at,
/// because guessing can delete the user's text.
pub fn upsert_block(existing: &str, begin: &str, end: &str, block: &str) -> Result<String> {
    let begins = existing.matches(begin).count();
    let ends = existing.matches(end).count();
    if begins == 0 && ends == 0 {
        if existing.trim().is_empty() {
            return Ok(format!("{block}\n"));
        }
        let separator = if existing.ends_with("\n\n") {
            ""
        } else if existing.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        return Ok(format!("{existing}{separator}{block}\n"));
    }
    let start = existing.find(begin);
    let stop = existing.find(end);
    match (begins, ends, start, stop) {
        (1, 1, Some(start), Some(stop)) if start < stop => {
            let stop = stop + end.len();
            Ok(format!("{}{}{}", &existing[..start], block, &existing[stop..]))
        }
        _ => bail!(
            "found {begins} begin and {ends} end memctrl markers; fix or remove them so the block can be replaced safely"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_replaces_only_a_well_formed_block() {
        let block = "<!-- b -->\nnew\n<!-- e -->";
        let up = |text: &str| upsert_block(text, "<!-- b -->", "<!-- e -->", block);
        assert_eq!(up("").unwrap(), format!("{block}\n"));
        assert_eq!(up("# Title\n").unwrap(), format!("# Title\n\n{block}\n"));
        assert_eq!(
            up("# Title\n\n<!-- b -->\nold\n<!-- e -->\n\nTail\n").unwrap(),
            format!("# Title\n\n{block}\n\nTail\n")
        );
        for broken in [
            "<!-- e -->\n<!-- b -->\n",
            "<!-- b -->\nUser section A\n",
            "<!-- b -->\n<!-- e -->\n<!-- b -->\n<!-- e -->\n",
            "text\n<!-- e -->\n",
        ] {
            assert!(up(broken).is_err(), "{broken:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_keeps_links_and_modes_and_refuses_escapes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        let link = dir.path().join("AGENTS.md");
        std::fs::write(&target, "old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        write_atomic(&link, b"new", Some(dir.path())).unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "permissions survive the rename");

        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join(".bashrc");
        std::fs::write(&victim, "export PATH").unwrap();
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".memctrl")).unwrap();
        std::os::unix::fs::symlink(&victim, repo.path().join(".memctrl/memory.md")).unwrap();
        assert!(write_atomic(
            &repo.path().join(".memctrl/memory.md"),
            b"x",
            Some(repo.path())
        )
        .is_err());
        std::os::unix::fs::symlink(outside.path(), repo.path().join("docs")).unwrap();
        assert!(write_atomic(&repo.path().join("docs/new.md"), b"x", Some(repo.path())).is_err());
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "export PATH");
        assert!(!outside.path().join("new.md").exists());
    }
}
