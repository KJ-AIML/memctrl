---
title: "memctrl context"
description: "Print a budgeted brief of the highest-trust memories for a session start"
sidebar:
  label: "context"
editUrl: false
---

Print a budgeted brief of the highest-trust memories for a session start

```text
memctrl context [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--budget <BUDGET>` | Approximate token budget | `3000` |  |
| `--layer <LAYERS>` | Layer to include (repeatable; default: project and user) |  |  |
| `--format <FORMAT>` | Possible values: - markdown: Markdown for people and AGENTS.md - json:     Machine-readable entries plus the Markdown - hook:     Claude Code and Codex hook output (additionalContext), reading the hook's JSON from stdin - gemini:   Gemini CLI hook output: the same, as SessionStart or BeforeAgent, and `{}` when there's nothing to add, since Gemini reads only JSON | `markdown` |  |
| `--write <WRITE>` | Write the brief between memctrl markers in this file (e.g. AGENTS.md) instead of printing it |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.8.0` by `website/scripts/gen-cli.mjs`.</sub>
