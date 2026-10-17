# MemCtrl 2.7.0: fixing what an honest review found

An honest review of memctrl listed its weak points. This release fixes the ones code can fix. The rest still stand: memctrl is a new project with one maintainer, and its benchmarks are its own. Only real users can change that, and only time.

## What's new

- **`remember:` saves a note as yours.** Start a prompt with `remember:` (or `จำไว้:` / `จำไว้ว่า`) in Claude Code, Codex or Gemini CLI, and the prompt hook saves the rest to your user layer as source `user`. Every later brief shows it under "From the user", which the agent is told to follow. The guard and redaction still apply, the note doesn't expire, and the agent is told it's saved so it doesn't save it again. `[capture] remember = false` in your user config turns this off.
- **What an agent saves is the agent's.** Claude Code, Codex, Gemini CLI and Cursor's agent each set a variable for the commands they run (`CLAUDECODE`, `CODEX_CI`, `GEMINI_CLI`, `CURSOR_AGENT`). memctrl now records an agent's `memctrl add` under the agent's name, such as `claude-code`, and an agent can't record a source the guard trusts, even with `--source user`. MCP writes can't either. So "From the user" holds what you saved, not what an agent saved for you. This is attribution, not proof: an agent that hides its variable can still write as you.
- **Codex and Gemini CLI get the hooks.** `memctrl install --tool codex --hook` and `--tool gemini --hook` add the session brief and per-prompt recall, as in Claude Code. In Codex 0.160, the request it sent the model carried both the brief and the recalled memory. The Gemini CLI hooks follow its documented format but haven't run in Gemini CLI itself yet. Cursor's prompt hook can't add context, so Cursor still gets the brief through AGENTS.md.
- **Search by meaning in one command.** `memctrl semantic setup` finds an embedding model in Ollama or LM Studio, checks that it answers, and measures thresholds when memctrl has none for it. Then it adds `[semantic]` to your user config, never replacing one, and indexes your memories. Against LM Studio with nomic-embed-text it took 1.5 seconds and measured the same thresholds as a manual `semantic tune`.
- **memctrl points out what to cite.** Only cited memories notice when their code changes. When a memory you add names project files but cites none, `add` prints the `memctrl cite` command for them, and `doctor` lists the same for memories you already have.
- **Every download is tested.** The release workflow now runs the whole test suite against each binary it's about to ship. Before, it ran three commands, and never ran the Intel macOS binary. That covers Linux x86_64 and arm64, macOS arm64, Windows, and Intel macOS under Rosetta.

## Fixes

- `semantic tune` could suggest a `recall_threshold` above 1, which the config then refused. It now stops at 1.
- `doctor` listed memories with source `user` as risky sources. It no longer does, and it recognizes the agents' sources too.

## Unchanged

The retrieval suites give the same results as 2.6.3: real project 91.7%, Thai 95.1%, Thai held out 91.7%, and vague questions answered with nothing at the same rates. The brief, recall's matching and the database format are the same.

## Upgrade notes

See the 2.6.3 → 2.7.0 section of [MIGRATION.md](MIGRATION.md). In short:

- If your agent used to save your preferences with `memctrl add --layer user`, those notes now count as the agent's and show as facts. Start a prompt with `remember:` to save one as yours.
- Run `memctrl install` again with the `--tool` you use, so the skill tells your agent about `remember:` and attribution.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/KJ-AIML/memctrl/main/install.sh | sh
```

Windows: download `memctrl-x86_64-pc-windows-msvc.zip` from the release and put `memctrl.exe` on your `PATH`.

Previous release notes live in [docs/history](docs/history).
