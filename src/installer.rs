//! Install the bundled tool skill (and, for Claude Code, Codex and Gemini CLI, the session-start
//! and prompt hooks) under an explicit project or home root.
use anyhow::{bail, ensure, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Where each tool discovers skills, relative to the project or home root.
/// Claude Code: `.claude/skills/`. Codex and Gemini CLI: `.agents/skills/`, which Gemini prefers
/// over `.gemini/skills/`. Cursor: `.cursor/skills/`.
pub fn skill_path(tool: &str) -> Result<&'static str> {
    crate::sanitize::validate_identifier(tool, "tool")?;
    Ok(match tool {
        "claude_code" | "claude" => ".claude/skills/memctrl/SKILL.md",
        "cursor" => ".cursor/skills/memctrl/SKILL.md",
        "codex" | "gemini" => ".agents/skills/memctrl/SKILL.md",
        "axga" => ".axga/agent/skills/memctrl/SKILL.md",
        "pi" => ".pi/agent/skills/memctrl/SKILL.md",
        "kimi" => ".kimi/skills/memctrl/SKILL.md",
        _ => {
            bail!("unsupported tool; choose claude_code, codex, gemini, cursor, axga, pi, or kimi")
        }
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

/// How hooks start memctrl: plain `memctrl` when that resolves to this same executable (so
/// upgrades and package-manager paths keep working), otherwise the quoted absolute path.
fn hook_program(executable: &Path) -> String {
    let same = on_path()
        .and_then(|found| found.canonicalize().ok())
        .zip(executable.canonicalize().ok())
        .is_some_and(|(found, current)| found == current);
    if same {
        "memctrl".to_string()
    } else {
        shell_quote(&executable.to_string_lossy())
    }
}

/// The session-start hook command, which injects the brief.
pub fn hook_command(executable: &Path) -> String {
    format!("{} {BRIEF_ARGS}", hook_program(executable))
}

/// The prompt hook command, which adds the memories that match each prompt.
pub fn recall_hook_command(executable: &Path) -> String {
    format!("{} {RECALL_ARGS}", hook_program(executable))
}

const BRIEF_ARGS: &str = "context --format hook";
const RECALL_ARGS: &str = "recall --format hook";

/// What follows the subcommand in memctrl's hook for `args`: `--format hook` plus any flags you
/// added by hand, such as `--budget 1500`. `None` when `hook` isn't that hook.
fn hook_flags(hook: &Value, args: &str) -> Option<String> {
    let command = hook.get("command")?.as_str()?;
    let (subcommand, format) = args.split_once(' ')?;
    let pattern = Regex::new(&format!(r#"memctrl(?:\.exe)?['"]?\s+{subcommand}(\s.*)?$"#)).ok()?;
    let flags = pattern.captures(command)?.get(1)?.as_str();
    flags.contains(format).then(|| flags.to_string())
}

/// Adds (or updates) a Claude Code SessionStart hook that injects `memctrl context`. The hook has
/// no matcher, so it also runs after `/clear`, resume and compaction, when context was lost.
/// Idempotent: an existing memctrl hook gets the new program and keeps its flags, everything else
/// is preserved in its original key order, and a file that isn't valid JSON is never rewritten.
pub fn install_hook(root: &Path, settings: &str, command: &str) -> Result<(PathBuf, bool)> {
    let handler = Host::ClaudeCode.handler("SessionStart", command);
    install_event_hook(root, settings, "SessionStart", BRIEF_ARGS, command, handler)
}

/// Adds (or updates) the UserPromptSubmit hook that runs `memctrl recall` on each prompt, the
/// same way `install_hook` handles the session-start hook.
pub fn install_recall_hook(root: &Path, settings: &str, command: &str) -> Result<(PathBuf, bool)> {
    let handler = Host::ClaudeCode.handler("UserPromptSubmit", command);
    install_event_hook(
        root,
        settings,
        "UserPromptSubmit",
        RECALL_ARGS,
        command,
        handler,
    )
}

/// A coding agent whose hooks can add memctrl's brief at session start and recall on each prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    ClaudeCode,
    Codex,
    Gemini,
}

impl Host {
    pub fn for_tool(tool: &str) -> Option<Self> {
        match tool {
            "claude" | "claude_code" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            "gemini" => Some(Self::Gemini),
            _ => None,
        }
    }

    /// The file its hooks go in, relative to the project or home folder.
    pub fn settings_file(self, project: bool) -> &'static str {
        match self {
            Self::ClaudeCode => settings_file(project),
            Self::Codex => ".codex/hooks.json",
            Self::Gemini => ".gemini/settings.json",
        }
    }

    /// The session-start and prompt events, with the memctrl arguments each runs.
    fn events(self) -> [(&'static str, &'static str); 2] {
        match self {
            Self::ClaudeCode | Self::Codex => [
                ("SessionStart", BRIEF_ARGS),
                ("UserPromptSubmit", RECALL_ARGS),
            ],
            Self::Gemini => [
                ("SessionStart", "context --format gemini"),
                ("BeforeAgent", "recall --format gemini"),
            ],
        }
    }

    /// The handler written for a new hook. Codex cuts hook output past about 2,500 tokens unless
    /// told otherwise, and the brief can reach 3,000; Gemini times out in milliseconds.
    fn handler(self, event: &str, command: &str) -> Value {
        let brief = event == "SessionStart";
        match self {
            Self::ClaudeCode => json!({"type": "command", "command": command}),
            Self::Codex if brief => {
                json!({"type": "command", "command": command, "additionalContextLimit": 4000})
            }
            Self::Codex => json!({"type": "command", "command": command}),
            Self::Gemini => json!({
                "type": "command",
                "command": command,
                "name": if brief { "memctrl-brief" } else { "memctrl-recall" },
                "timeout": 10000,
            }),
        }
    }

    /// What to do after installing before the hooks run, if anything.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Self::ClaudeCode => None,
            Self::Codex => Some("Codex runs new hooks only after you trust them: start Codex, run /hooks and trust the two memctrl hooks. Project hooks also need the project trusted"),
            Self::Gemini => Some("with folder trust on, Gemini CLI runs these hooks only in folders you've trusted, and it warns about new project hooks the first time"),
        }
    }

    /// Adds (or updates) both hooks, the way `install_hook` does for Claude Code.
    pub fn install_hooks(
        self,
        root: &Path,
        project: bool,
        executable: &Path,
    ) -> Result<(PathBuf, bool)> {
        let program = hook_program(executable);
        let file = self.settings_file(project);
        let mut written = None;
        let mut changed = false;
        for (event, args) in self.events() {
            let command = format!("{program} {args}");
            let (path, this) = install_event_hook(
                root,
                file,
                event,
                args,
                &command,
                self.handler(event, &command),
            )?;
            written = Some(path);
            changed |= this;
        }
        Ok((written.expect("every host has hooks"), changed))
    }
}

fn install_event_hook(
    root: &Path,
    settings: &str,
    event: &str,
    args: &str,
    command: &str,
    handler: Value,
) -> Result<(PathBuf, bool)> {
    let destination = scoped_destination(root, settings)?;
    let mut settings: Value = match std::fs::read_to_string(&destination) {
        Ok(text) if !text.trim().is_empty() => serde_json::from_str(&text).with_context(|| {
            format!(
                "{} is not valid JSON; add this {event} hook manually: {command}",
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
    let groups = hooks
        .entry(event)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .with_context(|| format!("settings.json \"hooks.{event}\" must be an array"))?;
    let program = command.strip_suffix(args).unwrap_or(command).trim_end();
    let subcommand = args.split(' ').next().unwrap_or(args);
    let mut found = false;
    let mut changed = false;
    for group in groups.iter_mut() {
        if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
            for hook in list.iter_mut() {
                let Some(flags) = hook_flags(hook, args) else {
                    continue;
                };
                found = true;
                let updated = format!("{program} {subcommand}{flags}");
                if hook.get("command").and_then(Value::as_str) != Some(updated.as_str()) {
                    hook["command"] = json!(updated);
                    changed = true;
                }
            }
        }
    }
    if !found {
        groups.push(json!({ "hooks": [handler] }));
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
    fn reinstalling_keeps_flags_added_by_hand() {
        let dir = tempfile::tempdir().unwrap();
        let file = ".claude/settings.local.json";
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(
            dir.path().join(file),
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"memctrl context --format hook --budget 1500"}]}],"UserPromptSubmit":[{"hooks":[{"type":"command","command":"'/old/memctrl' recall --limit 3 --format hook"}]}]}}"#,
        )
        .unwrap();
        let (_, changed) = install_hook(dir.path(), file, "memctrl context --format hook").unwrap();
        assert!(
            !changed,
            "the same program with extra flags is already up to date"
        );
        let (_, changed) =
            install_hook(dir.path(), file, "'/opt/memctrl' context --format hook").unwrap();
        assert!(changed);
        let (_, changed) =
            install_recall_hook(dir.path(), file, "memctrl recall --format hook").unwrap();
        assert!(changed);
        let value: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(file)).unwrap()).unwrap();
        assert_eq!(
            value["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "'/opt/memctrl' context --format hook --budget 1500"
        );
        assert_eq!(
            value["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
            "memctrl recall --limit 3 --format hook",
            "flags before --format hook count too, and no second hook is added"
        );
        assert_eq!(
            value["hooks"]["UserPromptSubmit"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn codex_and_gemini_get_both_hooks_in_their_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let exe = Path::new("/opt/memctrl");
        std::fs::create_dir_all(dir.path().join(".gemini")).unwrap();
        std::fs::write(
            dir.path().join(".gemini/settings.json"),
            r#"{"theme":"GitHub","hooks":{"BeforeTool":[{"matcher":"write_file","hooks":[{"type":"command","command":"./lint.sh"}]}]}}"#,
        )
        .unwrap();

        let (path, changed) = Host::Codex.install_hooks(dir.path(), true, exe).unwrap();
        assert!(changed);
        assert!(path.ends_with(".codex/hooks.json"));
        let codex: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            codex["hooks"]["SessionStart"][0]["hooks"][0],
            json!({"type": "command", "command": "/opt/memctrl context --format hook", "additionalContextLimit": 4000})
        );
        assert_eq!(
            codex["hooks"]["UserPromptSubmit"][0]["hooks"][0],
            json!({"type": "command", "command": "/opt/memctrl recall --format hook"})
        );
        assert!(!Host::Codex.install_hooks(dir.path(), true, exe).unwrap().1);

        let (path, changed) = Host::Gemini.install_hooks(dir.path(), true, exe).unwrap();
        assert!(changed);
        let gemini: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(gemini["theme"], "GitHub");
        assert_eq!(gemini["hooks"]["BeforeTool"][0]["matcher"], "write_file");
        assert_eq!(
            gemini["hooks"]["SessionStart"][0]["hooks"][0],
            json!({"type": "command", "command": "/opt/memctrl context --format gemini", "name": "memctrl-brief", "timeout": 10000})
        );
        assert_eq!(
            gemini["hooks"]["BeforeAgent"][0]["hooks"][0]["command"],
            "/opt/memctrl recall --format gemini"
        );
        // A later install keeps the handler's other fields and any flags added by hand.
        let edited = std::fs::read_to_string(&path).unwrap().replace(
            "context --format gemini",
            "context --format gemini --budget 1500",
        );
        std::fs::write(&path, edited).unwrap();
        let moved = Path::new("/usr/local/bin/memctrl");
        assert!(
            Host::Gemini
                .install_hooks(dir.path(), true, moved)
                .unwrap()
                .1
        );
        let gemini: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            gemini["hooks"]["SessionStart"][0]["hooks"][0],
            json!({"type": "command", "command": "/usr/local/bin/memctrl context --format gemini --budget 1500", "name": "memctrl-brief", "timeout": 10000})
        );
        assert_eq!(gemini["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        assert!(Host::for_tool("cursor").is_none());
    }

    #[test]
    fn the_prompt_hook_installs_next_to_the_brief_hook() {
        let dir = tempfile::tempdir().unwrap();
        let file = ".claude/settings.local.json";
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        install_hook(dir.path(), file, "memctrl context --format hook").unwrap();
        let (_, changed) =
            install_recall_hook(dir.path(), file, "memctrl recall --format hook").unwrap();
        assert!(changed);
        let (_, changed) =
            install_recall_hook(dir.path(), file, "memctrl recall --format hook").unwrap();
        assert!(!changed, "second install must be a no-op");
        let value: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(file)).unwrap()).unwrap();
        assert_eq!(value["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        assert_eq!(
            value["hooks"]["UserPromptSubmit"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            value["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
            "memctrl recall --format hook"
        );
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
