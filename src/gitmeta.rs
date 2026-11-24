//! Read the checked-out branch and a linked worktree's main worktree straight from the git
//! folder, without starting git.

use std::fs;
use std::path::{Component, Path, PathBuf};

/// What `HEAD` points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// `ref: refs/heads/<name>`: the branch name.
    Branch(String),
    /// A commit id: 40 (SHA-1) or 64 (SHA-256) hex characters.
    Detached(String),
}

/// Nearest ancestor of `start` (itself included) that contains `.git`.
pub fn repo_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// `<repo_root>/.git` when it is a folder; for a `.git` file, the folder its `gitdir:` line names.
pub fn git_dir(repo_root: &Path) -> Option<PathBuf> {
    let dot_git = repo_root.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let pointer = fs::read_to_string(&dot_git).ok()?;
    let target = pointer
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))?
        .trim();
    if target.is_empty() {
        return None;
    }
    let dir = normalize(&repo_root.join(target));
    dir.is_dir().then_some(dir)
}

/// From `<git_dir>/HEAD`: `ref: refs/heads/<name>` is a branch, 40 or 64 hex characters is detached.
pub fn head(repo_root: &Path) -> Option<Head> {
    let text = fs::read_to_string(git_dir(repo_root)?.join("HEAD")).ok()?;
    let text = text.trim();
    if let Some(name) = text.strip_prefix("ref: refs/heads/") {
        let name = name.trim();
        return (!name.is_empty()).then(|| Head::Branch(name.to_string()));
    }
    let is_commit = matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit());
    is_commit.then(|| Head::Detached(text.to_string()))
}

/// The main worktree of a linked worktree. `None` for a main worktree, a submodule, a bare
/// repository, or pointers that lead nowhere.
pub fn main_worktree(repo_root: &Path) -> Option<PathBuf> {
    if !repo_root.join(".git").is_file() {
        return None;
    }
    let git_dir = git_dir(repo_root)?;
    let common = fs::read_to_string(git_dir.join("commondir")).ok()?;
    let common = common.trim();
    if common.is_empty() {
        return None;
    }
    let common = normalize(&git_dir.join(common));
    if common.file_name()? != ".git" || !common.is_dir() {
        return None;
    }
    common.parent().map(Path::to_path_buf)
}

/// Drop `.` and resolve `..` lexically, without touching the file system.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push(component);
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A main worktree at `main` and a linked one at `wt` on `branch`, laid out as git does.
    fn linked(main: &Path, wt: &Path, branch: &str, relative: bool) {
        let gitdir = main.join(".git/worktrees/wt");
        fs::create_dir_all(&gitdir).unwrap();
        fs::write(main.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(gitdir.join("HEAD"), format!("ref: refs/heads/{branch}\n")).unwrap();
        fs::write(gitdir.join("commondir"), "../..\n").unwrap();
        fs::create_dir_all(wt).unwrap();
        let pointer = if relative {
            "../main/.git/worktrees/wt".to_string()
        } else {
            gitdir.display().to_string()
        };
        fs::write(wt.join(".git"), format!("gitdir: {pointer}\n")).unwrap();
    }

    #[test]
    fn a_plain_repository_has_a_branch_and_no_main_worktree() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/feat/auth\n").unwrap();
        fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        assert_eq!(
            repo_root(&dir.path().join("src/deep")).as_deref(),
            Some(dir.path())
        );
        assert_eq!(head(dir.path()), Some(Head::Branch("feat/auth".into())));
        assert_eq!(main_worktree(dir.path()), None);
    }

    #[test]
    fn a_detached_head_is_its_commit() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        let sha = "0123456789abcdef0123456789abcdef01234567";
        fs::write(dir.path().join(".git/HEAD"), format!("{sha}\n")).unwrap();
        assert_eq!(head(dir.path()), Some(Head::Detached(sha.into())));
    }

    #[test]
    fn a_linked_worktree_names_its_branch_and_its_main_worktree() {
        for relative in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let (main, wt) = (dir.path().join("main"), dir.path().join("wt"));
            linked(&main, &wt, "feat/x", relative);
            assert_eq!(
                head(&wt),
                Some(Head::Branch("feat/x".into())),
                "relative={relative}"
            );
            let found = main_worktree(&wt).expect("main worktree");
            assert!(
                found.join(".git/HEAD").is_file(),
                "relative={relative}: {found:?}"
            );
            assert!(
                !found.components().any(|c| c.as_os_str() == ".."),
                "normalized: {found:?}"
            );
        }
    }

    #[test]
    fn pointers_that_lead_nowhere_give_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // A submodule: .git points into modules/, which has no commondir.
        let sub = dir.path().join("sub");
        let modules = dir.path().join("super/.git/modules/sub");
        fs::create_dir_all(&modules).unwrap();
        fs::write(modules.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join(".git"), format!("gitdir: {}\n", modules.display())).unwrap();
        assert_eq!(head(&sub), Some(Head::Branch("main".into())));
        assert_eq!(main_worktree(&sub), None);
        // The main worktree was deleted.
        let gone = dir.path().join("gone");
        fs::create_dir_all(&gone).unwrap();
        fs::write(gone.join(".git"), "gitdir: /nowhere/.git/worktrees/gone\n").unwrap();
        assert_eq!((head(&gone), main_worktree(&gone)), (None, None));
        // Not a pointer at all, and garbage in HEAD.
        let odd = dir.path().join("odd");
        fs::create_dir_all(odd.join(".git")).unwrap();
        fs::write(odd.join(".git/HEAD"), "what\n").unwrap();
        assert_eq!(head(&odd), None);
    }
}
