---
title: "memctrl checkpoint"
description: "Where work on this branch left off: saved by hooks as you work, shown when a session starts"
sidebar:
  label: "checkpoint"
editUrl: false
---

Where work on this branch left off: saved by hooks as you work, shown when a session starts

```text
memctrl checkpoint [OPTIONS] [COMMAND]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--format <FORMAT>` | Possible values: - markdown: Markdown for people and AGENTS.md - json:     Machine-readable entries plus the Markdown - hook:     Claude Code and Codex hook output (additionalContext), reading the hook's JSON from stdin - gemini:   Gemini CLI hook output: the same, as SessionStart or BeforeAgent, and `{}` when there's nothing to add, since Gemini reads only JSON | `markdown` |  |
| `--announce` | With --format hook: also tell the person a checkpoint was picked up |  |  |

## Subcommands

| Subcommand | What it does |
| --- | --- |
| [`show`](#memctrl-checkpoint-show) | What the next session on this branch would get, however old (--format json: the stored file) |
| [`note`](#memctrl-checkpoint-note) | Leave a note for the next session on this branch |
| [`clear`](#memctrl-checkpoint-clear) | Delete this branch's checkpoint |

### `memctrl checkpoint show`

What the next session on this branch would get, however old (--format json: the stored file)

```text
memctrl checkpoint show [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--format <FORMAT>` | Possible values: - markdown: Markdown for people and AGENTS.md - json:     Machine-readable entries plus the Markdown - hook:     Claude Code and Codex hook output (additionalContext), reading the hook's JSON from stdin - gemini:   Gemini CLI hook output: the same, as SessionStart or BeforeAgent, and `{}` when there's nothing to add, since Gemini reads only JSON | `markdown` |  |

### `memctrl checkpoint note`

Leave a note for the next session on this branch

```text
memctrl checkpoint note [OPTIONS] <TEXT>
```

| Argument | Description |
| --- | --- |
| `<TEXT>` | — |

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--format <FORMAT>` | Possible values: - markdown: Markdown for people and AGENTS.md - json:     Machine-readable entries plus the Markdown - hook:     Claude Code and Codex hook output (additionalContext), reading the hook's JSON from stdin - gemini:   Gemini CLI hook output: the same, as SessionStart or BeforeAgent, and `{}` when there's nothing to add, since Gemini reads only JSON | `markdown` |  |

### `memctrl checkpoint clear`

Delete this branch's checkpoint

```text
memctrl checkpoint clear [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--format <FORMAT>` | Possible values: - markdown: Markdown for people and AGENTS.md - json:     Machine-readable entries plus the Markdown - hook:     Claude Code and Codex hook output (additionalContext), reading the hook's JSON from stdin - gemini:   Gemini CLI hook output: the same, as SessionStart or BeforeAgent, and `{}` when there's nothing to add, since Gemini reads only JSON | `markdown` |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.9.0` by `website/scripts/gen-cli.mjs`.</sub>
