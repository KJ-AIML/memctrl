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
}

impl Project {
    /// The store a project uses when nothing names another one.
    pub fn default_db(&self) -> PathBuf {
        self.dir.join(DEFAULT_DB)
    }
}

/// Walks up from `cwd` to the nearest folder with a `.memoryrc`. The walk stops at the first
/// folder containing `.git` (the repository root), and never climbs into `home` from below it,
/// so one stray file can't capture every project. Without a rules file, a store already at the
/// git root or in `cwd` is used, which keeps projects set up by memctrl 2.1.0 working.
pub fn locate(cwd: &Path, home: Option<&Path>) -> Project {
    let mut searched_to = cwd.to_path_buf();
    let mut git_root = None;
    for dir in cwd.ancestors() {
        if dir != cwd && home.is_some_and(|home| dir == home) {
            break;
        }
        searched_to = dir.to_path_buf();
        if dir.join(RULES_FILE).is_file() {
            return Project {
                rc: dir.join(RULES_FILE),
                dir: dir.to_path_buf(),
                found: true,
                searched_to,
            };
        }
        if dir.join(".git").exists() {
            git_root = Some(dir);
            break;
        }
    }
    for dir in git_root.into_iter().chain([cwd]) {
        if dir.join(DEFAULT_DB).is_file() {
            return Project {
                rc: dir.join(RULES_FILE),
                dir: dir.to_path_buf(),
                found: true,
                searched_to,
            };
        }
    }
    Project {
        rc: cwd.join(RULES_FILE),
        dir: cwd.to_path_buf(),
        found: false,
        searched_to,
    }
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
}
