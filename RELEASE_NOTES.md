# MemCtrl 2.9.0: where work left off

memctrl kept what stays true about a project, and nothing about the work in hand. After `/clear`, a compaction or a new terminal, the agent got the project's memories and not what you had asked, where it had stopped or what had failed, so you avoided `/clear` and retyped the task after every compaction. A new worktree started with an empty store. This release keeps a small checkpoint per branch, saved by the hooks from what the host already hands them, and shows it to the next session on that branch. Nothing is typed.

## What's new

- **A checkpoint per branch.** It holds the last three requests, the agent's last message, the last compaction summary (Claude Code only), the files with uncommitted changes, and notes. It's one JSON file per branch in `.memctrl/checkpoints/`, which git ignores. Every part is redacted before it's saved.
- **Shown when you start over.** A session that starts fresh or after `/clear` gets "Where work on `<branch>` left off" next to the brief, framed as notes the current request outranks. Not on resume, compaction or a fork, which still hold the conversation. Only if the checkpoint is under 7 days old, and within 900 tokens. Each part is checked by the guard as the block is built; a part that reads like instructions is left out, and the block says so. Claude Code also shows you one line: ``memctrl: picked up where `feat/auth-sessions` left off (2 hours ago)``.
- **Failed approaches have two ways in.** Before Claude Code compacts, a hook asks it to keep the request, the decisions, every approach that failed and why, and the next step in its summary. And anyone, the agent included, can leave a note: `memctrl checkpoint note "Tried X: deadlocks with the audit trigger"`. `memctrl checkpoint show` prints what the next session would get, and `memctrl checkpoint clear` deletes it.
- **Session-layer notes reach the next session.** Up to 5 of them join the checkpoint's notes. Before, the brief and recall never read the `session` layer, so notes saved there never came back.
- **Worktrees share the store.** A worktree made with `git worktree add` uses the main worktree's store, so it starts with the project's memories. Checkpoints are kept per branch, and git allows a branch in only one worktree, so worktrees don't overwrite each other's. `memctrl init --force` gives one its own store.
- **New hooks.** `memctrl install --tool <host> --hook` adds `Stop`, `PreCompact`, `PostCompact` and a second `SessionStart` handler for Claude Code; `Stop` and a second `SessionStart` for Codex; `AfterAgent` and a second `SessionStart` for Gemini CLI. Codex and Gemini CLI hand over no compaction summary, so they get the last exchange and the files.
- **Your settings.** `[continuity] prompts = false` keeps your requests out of the checkpoint, and `enabled = false` turns checkpoints off, in your user config or a repository's `.memoryrc`, which can only turn them off. `MEMCTRL_CONTINUITY=off` does it for one command.

On the machine this was built on, a `Stop` hook took about 0.2 s and a `SessionStart` checkpoint hook about 0.1 s, release build, Windows 11.

## Fixes

- **The brief stays under Claude Code's cap on hook output.** Claude Code replaces hook output over 10,000 characters with a file path and a 2,000-character preview, and the brief's 3,000-token budget can reach 12,000 characters. For Claude Code and Codex, the brief now shrinks its budget until it fits 9,500 characters, and a trimmed brief records one provenance row, not one per try. A large store shows fewer memories at session start; the header names what didn't fit.
- **`memctrl add` without `--layer` saves to `project`.** It saved to `session`, which the brief and recall don't read and which expires in seven days, so the quickest way to save a note lost it. The skill no longer tells agents to save work in progress there.

## Not measured

No agent benchmark was run for this release, so there's no number for how often a checkpoint helps an agent finish a task after `/clear` or a compaction. The 9,500-character cap follows Claude Code's documentation, not a recorded session: what the model receives from a brief just under or over the cap wasn't checked. The hooks were run by hand against the release binary with the JSON each host documents, not inside a live Claude Code, Codex or Gemini CLI session, so whether Claude Code shows the `systemMessage` line wasn't seen either.

## Upgrading

See the 2.8.0 → 2.9.0 section of [MIGRATION.md](MIGRATION.md). In short:

- **Run `memctrl install --tool <host> --hook` again** to get the new hooks. Codex asks you to trust them with `/hooks`.
- **`memctrl add` defaults to `project`.** Scripts that want a seven-day note pass `--layer session`.
- **Your last three requests are kept on disk**, redacted, in `.memctrl/checkpoints/`, where 2.8 stored no prompt. Set `[continuity] prompts = false` to stop it.
- **No schema change.** 2.8.0 and 2.9.0 work on one store.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/KJ-AIML/memctrl/main/install.sh | sh
```

Windows: download `memctrl-x86_64-pc-windows-msvc.zip` from the release and put `memctrl.exe` on your `PATH`.

Previous release notes live in [docs/history](docs/history).
