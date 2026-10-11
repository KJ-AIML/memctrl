//! Install the bundled tool skill under an explicit project or home root.
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// `root` is the selected scope's base directory, never implicitly the process HOME.
pub fn install(tool: &str, project: bool, root: &Path) -> Result<PathBuf> {
    crate::sanitize::validate_identifier(tool, "tool")?;
    let relative = match tool {
        "claude_code" | "claude" => ".claude/agent/skills/memctrl/SKILL.md",
        "cursor" => ".cursor/skills/memctrl/SKILL.md",
        "codex" => ".codex/skills/memctrl/SKILL.md",
        "axga" => ".axga/agent/skills/memctrl/SKILL.md",
        "pi" => ".pi/agent/skills/memctrl/SKILL.md",
        "kimi" => ".kimi/skills/memctrl/SKILL.md",
        _ => bail!("unsupported tool; choose claude_code, cursor, codex, axga, pi, or kimi"),
    };
    // Both supported scopes use a root supplied by the caller.
    let _ = project;
    std::fs::create_dir_all(root)?;
    let scope = root.canonicalize()?;
    let destination = root.join(relative);
    // Existing configuration symlinks must remain inside the requested scope.
    // Check before directory creation to avoid creating directories through an escape.
    let mut checked = scope.clone();
    for component in Path::new(relative).components() {
        checked.push(component);
        if std::fs::symlink_metadata(&checked).is_ok()
            && !checked.canonicalize()?.starts_with(&scope)
        {
            bail!("installation path escapes the selected scope");
        }
    }
    std::fs::create_dir_all(destination.parent().unwrap())?;
    if !destination
        .parent()
        .unwrap()
        .canonicalize()?
        .starts_with(&scope)
    {
        bail!("installation path escapes the selected scope");
    }
    std::fs::write(&destination, include_str!("../assets/SKILL.md"))?;
    Ok(destination)
}
