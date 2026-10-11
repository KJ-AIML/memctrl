//! Install the bundled tool skill (and, for Claude Code, a session-start hook) under an explicit
//! project or home root.
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Where each tool discovers skills, relative to the project or home root.
/// Claude Code: `.claude/skills/`. Codex: `.agents/skills/` (current Codex docs).
/// Cursor: `.cursor/skills/`.
pub fn skill_path(tool: &str) -> Result<&'static str> {
    crate::sanitize::validate_identifier(tool, "tool")?;
    Ok(match tool {
        "claude_code" | "claude" => ".claude/skills/memctrl/SKILL.md",
        "cursor" => ".cursor/skills/memctrl/SKILL.md",
        "codex" => ".agents/skills/memctrl/SKILL.md",
        "axga" => ".axga/agent/skills/memctrl/SKILL.md",
        "pi" => ".pi/agent/skills/memctrl/SKILL.md",
        "kimi" => ".kimi/skills/memctrl/SKILL.md",
        _ => bail!("unsupported tool; choose claude_code, cursor, codex, axga, pi, or kimi"),
    })
}

/// Skill locations written by memctrl 2.0 that the tool never read; reported so they can be removed.
pub fn legacy_skill_path(tool: &str) -> Option<&'static str> {
    match tool {
        "claude_code" | "claude" => Some(".claude/agent/skills/memctrl/SKILL.md"),
        "codex" => Some(".codex/skills/memctrl/SKILL.md"),
        _ => None,
    }
}

/// Resolves `relative` under `root`, creating parent directories, and refuses any existing
/// symlink along the way that points outside the root. The check runs before directories are
/// created so an escape can't be used to create them elsewhere.
fn scoped_destination(root: &Path, relative: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let scope = root.canonicalize()?;
    let destination = root.join(relative);
    let mut checked = scope.clone();
    for component in Path::new(relative).components() {
        checked.push(component);
        if std::fs::symlink_metadata(&checked).is_ok()
            && !checked.canonicalize()?.starts_with(&scope)
        {
            bail!("installation path escapes the selected scope");
        }
    }
    let parent = destination
        .parent()
        .context("installation path has no parent")?;
    std::fs::create_dir_all(parent)?;
    if !parent.canonicalize()?.starts_with(&scope) {
        bail!("installation path escapes the selected scope");
    }
    Ok(destination)
}

/// `root` is the selected scope's base directory, never implicitly the process HOME.
pub fn install(tool: &str, project: bool, root: &Path) -> Result<PathBuf> {
    let relative = skill_path(tool)?;
    // Both supported scopes use a root supplied by the caller.
    let _ = project;
    let destination = scoped_destination(root, relative)?;
    std::fs::write(&destination, include_str!("../assets/SKILL.md"))?;
    Ok(destination)
}

/// Quotes an executable path for the shell that runs Claude Code hook commands.
fn shell_quote(path: &str) -> String {
    if !path.is_empty()
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+:@%,".contains(c))
    {
        return path.into();
    }
    if cfg!(windows) {
        format!("\"{}\"", path.replace('"', "\\\""))
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

/// Claude Code settings file for a scope. Project installs use `settings.local.json`, which is
/// personal and not committed, so one machine's executable path never lands in the team's
/// shared `.claude/settings.json`.
pub fn settings_file(project: bool) -> &'static str {
    if project {
        ".claude/settings.local.json"
    } else {
        ".claude/settings.json"
    }
}

/// The first `memctrl` on PATH, if any.
fn on_path() -> Option<std::path::PathBuf> {
    let name = if cfg!(windows) {
        "memctrl.exe"
    } else {
        "memctrl"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The hook command: plain `memctrl` when that resolves to this same executable (so upgrades
/// and package-manager paths keep working), otherwise the quoted absolute path.
pub fn hook_command(executable: &Path) -> String {
    let same = on_path()
        .and_then(|found| found.canonicalize().ok())
        .zip(executable.canonicalize().ok())
        .is_some_and(|(found, current)| found == current);
    let program = if same {
        "memctrl".to_string()
    } else {
        shell_quote(&executable.to_string_lossy())
    };
    format!("{program} context --format hook")
}

fn is_memctrl_brief_hook(hook: &Value) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| c.contains("memctrl") && c.contains("context --format hook"))
}

/// Adds (or updates) a Claude Code SessionStart hook that injects `memctrl context`. The hook has
/// no matcher, so it also runs after `/clear`, resume and compaction, when context was lost.
/// Idempotent: an existing memctrl hook is updated in place, everything else is preserved in its
/// original key order, and a file that isn't valid JSON is never rewritten.
pub fn install_hook(root: &Path, settings: &str, command: &str) -> Result<(PathBuf, bool)> {
    let destination = scoped_destination(root, settings)?;
    let mut settings: Value = match std::fs::read_to_string(&destination) {
        Ok(text) if !text.trim().is_empty() => serde_json::from_str(&text).with_context(|| {
            format!(
                "{} is not valid JSON; add this SessionStart hook manually: {command}",
                destination.display()
            )
        })?,
        Ok(_) => json!({}),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(error) => return Err(error.into()),
    };
    let object = settings
        .as_object_mut()
        .context("settings.json must contain a JSON object")?;
    let hooks = object.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks
        .as_object_mut()
        .context("settings.json \"hooks\" must be an object")?;
    let session = hooks.entry("SessionStart").or_insert_with(|| json!([]));
    let groups = session
        .as_array_mut()
        .context("settings.json \"hooks.SessionStart\" must be an array")?;
    let mut found = false;
    let mut changed = false;
    for group in groups.iter_mut() {
        if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
            for hook in list.iter_mut().filter(|h| is_memctrl_brief_hook(h)) {
                found = true;
                if hook.get("command").and_then(Value::as_str) != Some(command) {
                    hook["command"] = json!(command);
                    changed = true;
                }
            }
        }
    }
    if !found {
        groups.push(json!({"hooks":[{"type":"command","command":command}]}));
        changed = true;
    }
    if changed {
        let mut text = serde_json::to_string_pretty(&settings)?;
        text.push('\n');
        crate::fsutil::write_atomic(&destination, text.as_bytes(), Some(root))?;
    }
    ensure!(destination.exists(), "settings.json was not written");
    Ok((destination, changed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_merge_is_idempotent_and_preserves_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join(".claude/settings.json");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(
            &settings,
            r#"{"permissions":{"allow":["Bash(ls)"]},"hooks":{"SessionStart":[{"matcher":"startup","hooks":[{"type":"command","command":"echo hi"}]}]},"model":"opus"}"#,
        )
        .unwrap();
        let (_, changed) = install_hook(
            dir.path(),
            ".claude/settings.json",
            "/bin/memctrl context --format hook",
        )
        .unwrap();
        assert!(changed);
        let (_, changed) = install_hook(
            dir.path(),
            ".claude/settings.json",
            "/bin/memctrl context --format hook",
        )
        .unwrap();
        assert!(!changed, "second install must be a no-op");
        let (_, changed) = install_hook(
            dir.path(),
            ".claude/settings.json",
            "/opt/memctrl context --format hook",
        )
        .unwrap();
        assert!(changed, "a moved binary updates the existing hook");
        let text = std::fs::read_to_string(&settings).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        let groups = value["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(groups.len(), 2, "{text}");
        assert_eq!(groups[0]["hooks"][0]["command"], "echo hi");
        assert_eq!(
            groups[1]["hooks"][0]["command"],
            "/opt/memctrl context --format hook"
        );
        assert_eq!(value["permissions"]["allow"][0], "Bash(ls)");
        // Original key order survives the rewrite.
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["permissions", "hooks", "model"]);
    }

    #[test]
    fn hook_install_refuses_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join(".claude/settings.json");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(&settings, "{ // comments are not JSON\n}").unwrap();
        let error = install_hook(
            dir.path(),
            ".claude/settings.json",
            "memctrl context --format hook",
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("add this SessionStart hook manually"));
        assert_eq!(
            std::fs::read_to_string(&settings).unwrap(),
            "{ // comments are not JSON\n}"
        );
    }

    #[test]
    fn executable_paths_are_quoted_for_the_shell() {
        assert_eq!(
            shell_quote("/usr/local/bin/memctrl"),
            "/usr/local/bin/memctrl"
        );
        if !cfg!(windows) {
            assert_eq!(shell_quote("/Users/a b/memctrl"), "'/Users/a b/memctrl'");
            assert_eq!(shell_quote("/x/it's"), "'/x/it'\\''s'");
        }
    }
}
