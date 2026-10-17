---
title: "memctrl install"
description: "Install SKILL.md for a coding tool (and optionally a Claude Code session-start hook)"
sidebar:
  label: "install"
editUrl: false
---

Install SKILL.md for a coding tool (and optionally a Claude Code session-start hook)

```text
memctrl install [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--tool <TOOL>` | Required: `claude`, `codex`, `cursor`, `axga`, `pi` or `kimi` |  |  |
| `--project` | Install into this project instead of your home folder |  |  |
| `--hook` | Claude Code only: add a SessionStart hook that injects `memctrl context` |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.6.2` by `website/scripts/gen-cli.mjs`.</sub>
