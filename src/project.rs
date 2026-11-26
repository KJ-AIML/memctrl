//! Finds the project a command runs in, so memctrl behaves the same from any subfolder: the
//! nearest `.memoryrc` above the working directory, else a store an earlier version left at the
//! git root or here. Only `memctrl init` (or a database path the user names) creates a store.
use std::path::{Path, PathBuf};

pub const RULES_FILE: &str = ".memoryrc";
pub const DEFAULT_DB: &str = ".memctrl/memories.db";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// The rules file. It may not exist; a missing file means default rules.
    pub rc: PathBuf,
    /// The project folder: where the rules file or store was found, else the working directory.
    pub dir: PathBuf,
    /// True when a rules file or store was found, i.e. memctrl is set up here.
    pub found: bool,
    /// The last folder searched, for error messages.
    pub searched_to: PathBuf,
    /// The folder whose `.memctrl/` holds the store: `dir`, or the main worktree for a linked
    /// worktree that has no store of its own.
    pub store_root: PathBuf,
}

impl Project {
    /// The store a project uses when nothing names another one.
    pub fn default_db(&self) -> PathBuf {
        self.store_root.join(DEFAULT_DB)
    }
}

/// Walks up from `cwd` to the nearest folder with a `.memoryrc`. The walk stops at the first
/// folder containing `.git` (the repository root), and never climbs into `home` from below it,
/// so one stray file can't capture every project. Without a rules file, a store already at the
/// git root or in `cwd` is used, which keeps projects set up by memctrl 2.1.0 working. A linked
/// worktree without a store of its own uses its main worktree's store (git ignores the store, so
/// a new worktree never has one), while the worktree stays the project its files are read from.
pub fn locate(cwd: &Path, home: Option<&Path>) -> Project {
    // `cwd` and its ancestors, never climbing into `home` from below it.
    let walk = |from: &'_ Path| {
        from.ancestors()
            .take_while(|dir| *dir == cwd || home.is_none_or(|home| *dir != home))
            .map(Path::to_path_buf)
            .collect::<Vec<_>>()
    };
    let mut searched_to = cwd.to_path_buf();
    let mut git_root = None;
    let mut rules_dir = None;
    for dir in walk(cwd) {
        searched_to = dir.clone();
        if dir.join(RULES_FILE).is_file() {
            rules_dir = Some(dir);
            break;
        }
        if dir.join(".git").exists() {
            git_root = Some(dir);
            break;
        }
    }
    let mut project = if let Some(dir) = rules_dir {
        // The rules file ended the walk early: keep walking up from it for the repository root.
        git_root = walk(&dir).into_iter().find(|d| d.join(".git").exists());
        Project {
            rc: dir.join(RULES_FILE),
            store_root: dir.clone(),
            dir,
            found: true,
            searched_to,
        }
    } else if let Some(dir) = git_root
        .iter()
        .map(PathBuf::as_path)
        .chain([cwd])
        .find(|dir| dir.join(DEFAULT_DB).is_file())
    {
        Project {
            rc: dir.join(RULES_FILE),
            dir: dir.to_path_buf(),
            store_root: dir.to_path_buf(),
            found: true,
            searched_to,
        }
    } else {
        Project {
            rc: cwd.join(RULES_FILE),
            dir: cwd.to_path_buf(),
            store_root: cwd.to_path_buf(),
            found: false,
            searched_to,
        }
    };
    if project.dir.join(DEFAULT_DB).is_file() {
        return project;
    }
    let Some(root) = git_root else {
        return project;
    };
    let Some(main) = crate::gitmeta::main_worktree(&root) else {
        return project;
    };
    let dir = if project.found {
        project.dir.clone()
    } else {
        root.clone()
    };
    // The same project folder in the main checkout, so a nested project never takes the root's.
    let Ok(relative) = dir.strip_prefix(&root) else {
        return project;
    };
    let main_dir = main.join(relative);
    if !main_dir.join(DEFAULT_DB).is_file() {
        return project;
    }
    project.dir = dir;
    let own_rc = project.dir.join(RULES_FILE);
    project.rc = if own_rc.is_file() {
        own_rc
    } else {
        main_dir.join(RULES_FILE)
    };
    project.found = true;
    project.store_root = main_dir;
    project
}

/// True when git tracks `db`. A committed database holds whatever anyone with push access wrote,
/// already accepted, and the write guard never saw it; memory is shared through memory.md
/// instead, where every entry is reviewed on import.
pub fn tracked_by_git(db: &Path) -> bool {
    let (Some(dir), Some(name)) = (db.parent(), db.file_name()) else {
        return false;
    };
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let Ok(dir_abs) = dir.canonicalize() else {
        return false;
    };
    let Some(root) = dir_abs.ancestors().find(|a| a.join(".git").exists()) else {
        return false;
    };
    // Starting git costs a few milliseconds on every command, so read the index first: when it
    // plainly doesn't list the database, nothing tracks it.
    if index_lists(root, &dir_abs.join(name)) == Some(false) {
        return false;
    }
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Whether `<root>/.git/index` contains `path`, read without git. `None` means "can't tell":
/// a worktree or submodule (`.git` is a file), a split index, or index version 4, which
/// prefix-compresses paths so they aren't stored whole. Versions 2 and 3 store every tracked
/// path in full, so a path that isn't in the bytes isn't tracked.
fn index_lists(root: &Path, path: &Path) -> Option<bool> {
    let git_dir = root.join(".git");
    if !git_dir.is_dir() {
        return None;
    }
    let rel = path.strip_prefix(root).ok()?;
    let mut needle = Vec::new();
    for (i, part) in rel.components().enumerate() {
        if i > 0 {
            needle.push(b'/');
        }
        needle.extend_from_slice(part.as_os_str().to_str()?.as_bytes());
    }
    // Git may store a non-ASCII name in another Unicode normalization than the filesystem.
    if !needle.is_ascii() {
        return None;
    }
    let index = match std::fs::read(git_dir.join("index")) {
        Ok(index) => index,
        // No index yet: nothing has ever been added.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(false),
        Err(_) => return None,
    };
    if index.len() < 12 || &index[..4] != b"DIRC" || !matches!(index[4..8], [0, 0, 0, 2 | 3]) {
        return None;
    }
    let shared = std::fs::read_dir(&git_dir)
        .ok()?
        .any(|e| e.is_ok_and(|e| e.file_name().to_string_lossy().starts_with("sharedindex.")));
    if shared {
        return None;
    }
    // Case-insensitive, so a case-insensitive filesystem can only cause a fallback to git.
    Some(
        index
            .windows(needle.len())
            .any(|w| w.eq_ignore_ascii_case(&needle)),
    )
}

/// `locate` from the process's working directory and home.
pub fn locate_here() -> std::io::Result<Project> {
    let cwd = std::env::current_dir()?;
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    // The working directory is already resolved; resolve home the same way (on Windows,
    // canonicalize adds a \\?\ prefix the working directory doesn't have).
    #[cfg(not(windows))]
    let home = home.map(|home| home.canonicalize().unwrap_or(home));
    Ok(locate(&cwd, home.as_deref()))
}

/// Stores that memctrl 2.1.0 created in subfolders (any command used to create one in the
/// working directory). Bounded: skips hidden and dependency folders and stops after a few
/// thousand folders, so it stays fast on large repositories.
pub fn stray_stores(project_dir: &Path, active: &Path) -> Vec<PathBuf> {
    const SKIP: [&str; 6] = [
        "node_modules",
        "target",
        "vendor",
        "dist",
        "build",
        "__pycache__",
    ];
    let active = active.canonicalize().ok();
    let mut found = Vec::new();
    let mut queue = vec![(project_dir.to_path_buf(), 0usize)];
    let mut visited = 0;
    while let Some((dir, depth)) = queue.pop() {
        visited += 1;
        if visited > 5_000 || found.len() >= 20 {
            break;
        }
        let store = dir.join(DEFAULT_DB);
        if store.is_file() && store.canonicalize().ok() != active {
            found.push(
                store
                    .strip_prefix(project_dir)
                    .map(Path::to_path_buf)
                    .unwrap_or(store),
            );
        }
        if depth >= 6 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || SKIP.contains(&name.as_ref()) {
                continue;
            }
            // file_type doesn't follow symlinks, so a link can't send the walk outside.
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                queue.push((entry.path(), depth + 1));
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    #[test]
    fn nearest_rules_file_wins_and_the_walk_stops_at_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        touch(&root.join(RULES_FILE)); // outside the repository: never adopted
        let found = locate(&repo.join("src/deep"), None);
        assert!(!found.found);
        assert_eq!(found.dir, repo.join("src/deep"));
        assert_eq!(found.searched_to, repo);

        touch(&repo.join(RULES_FILE));
        let found = locate(&repo.join("src/deep"), None);
        assert!(found.found);
        assert_eq!(found.dir, repo);
        assert_eq!(found.rc, repo.join(RULES_FILE));
        assert_eq!(found.default_db(), repo.join(DEFAULT_DB));

        // A nested project (e.g. one package of a monorepo) is its own project.
        touch(&repo.join("src/.memoryrc"));
        assert_eq!(locate(&repo.join("src/deep"), None).dir, repo.join("src"));
    }

    #[test]
    fn a_store_without_rules_is_found_at_the_git_root_and_stray_stores_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        touch(&repo.join("src").join(DEFAULT_DB)); // left by 2.1.0 in a subfolder
        touch(&repo.join(DEFAULT_DB));
        let found = locate(&repo.join("src/lib"), None);
        assert!(found.found);
        assert_eq!(found.dir, repo);
        assert_eq!(
            stray_stores(&repo, &repo.join(DEFAULT_DB)),
            [Path::new("src").join(DEFAULT_DB)]
        );
    }

    #[test]
    fn the_walk_never_climbs_into_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        touch(&home.join(RULES_FILE));
        std::fs::create_dir_all(home.join("notes/today")).unwrap();
        assert!(!locate(&home.join("notes/today"), Some(&home)).found);
        // Home itself is a project when you run memctrl there.
        assert!(locate(&home, Some(&home)).found);
    }

    #[test]
    fn a_linked_worktree_falls_back_to_the_main_store() {
        let dir = tempfile::tempdir().unwrap();
        let (main, wt) = (dir.path().join("main"), dir.path().join("wt"));
        let gitdir = main.join(".git/worktrees/wt");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();
        std::fs::create_dir_all(wt.join("src")).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        touch(&main.join(DEFAULT_DB));
        touch(&main.join(RULES_FILE));

        // No rules file in the worktree: the main worktree's rules and store, the worktree as project.
        let found = locate(&wt.join("src"), None);
        assert!(found.found);
        assert_eq!(
            (found.dir.as_path(), found.rc.as_path()),
            (wt.as_path(), main.join(RULES_FILE).as_path())
        );
        assert!(found.default_db().ends_with("main/.memctrl/memories.db"));

        // Its own rules file is kept; the store is still the main one.
        touch(&wt.join(RULES_FILE));
        let found = locate(&wt, None);
        assert_eq!(found.rc, wt.join(RULES_FILE));
        assert!(found.default_db().ends_with("main/.memctrl/memories.db"));

        // Its own store wins.
        touch(&wt.join(DEFAULT_DB));
        let found = locate(&wt, None);
        assert_eq!(found.store_root, wt);

        // A main worktree, and a folder outside git, are unchanged.
        assert_eq!(locate(&main, None).store_root, main);
        let plain = dir.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let nothing = locate(&plain, None);
        assert!(!nothing.found);
        assert_eq!(nothing.store_root, plain);
    }

    #[test]
    fn a_nested_project_in_a_linked_worktree_falls_back_to_its_own_main_store() {
        let dir = tempfile::tempdir().unwrap();
        let (main, wt) = (dir.path().join("main"), dir.path().join("wt"));
        let gitdir = main.join(".git/worktrees/wt");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();
        std::fs::create_dir_all(wt.join("sub/src")).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        touch(&wt.join("sub").join(RULES_FILE));

        // Only the root project's store: never the nested project's.
        touch(&main.join(DEFAULT_DB));
        let found = locate(&wt.join("sub/src"), None);
        assert_eq!(found.store_root, wt.join("sub"));

        // The nested project's own store in the main checkout.
        touch(&main.join("sub").join(DEFAULT_DB));
        let found = locate(&wt.join("sub/src"), None);
        assert_eq!(found.store_root, main.join("sub"));
        assert_eq!(found.dir, wt.join("sub"));
        assert_eq!(found.rc, wt.join("sub").join(RULES_FILE));
    }

    #[test]
    fn the_index_is_read_without_git_only_when_it_can_answer() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().canonicalize().unwrap();
        let db = repo.join(DEFAULT_DB);
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        assert_eq!(index_lists(&repo, &db), Some(false), "no index yet");

        let index = |version: u8, body: &[u8]| {
            let mut bytes = b"DIRC\0\0\0".to_vec();
            bytes.push(version);
            bytes.extend_from_slice(&[0, 0, 0, 1]);
            bytes.extend_from_slice(body);
            std::fs::write(repo.join(".git/index"), bytes).unwrap();
        };
        index(2, b"\0src/main.rs\0");
        assert_eq!(index_lists(&repo, &db), Some(false));
        index(2, b"\0.MEMCTRL/memories.db\0");
        assert_eq!(index_lists(&repo, &db), Some(true), "case never hides it");
        // Version 4 compresses paths, so a missing path proves nothing.
        index(4, b"\0src/main.rs\0");
        assert_eq!(index_lists(&repo, &db), None);
        index(2, b"\0src/main.rs\0");
        touch(&repo.join(".git/sharedindex.0123"));
        assert_eq!(index_lists(&repo, &db), None, "split index");

        // A worktree's .git is a file pointing elsewhere.
        let worktree = repo.join("wt");
        touch(&worktree.join(".git"));
        assert_eq!(index_lists(&worktree, &worktree.join(DEFAULT_DB)), None);
    }
}
