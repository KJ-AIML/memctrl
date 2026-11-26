---
title: "memctrl install"
description: "Install SKILL.md for a coding tool (and optionally its hooks: the brief, recall and checkpoints)"
sidebar:
  label: "install"
editUrl: false
---

Install SKILL.md for a coding tool (and optionally its hooks: the brief, recall and checkpoints)

```text
memctrl install [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--tool <TOOL>` | Required: `claude`, `codex`, `cursor`, `axga`, `pi` or `kimi` |  |  |
| `--project` | Install into this project instead of your home folder |  |  |
| `--hook` | Claude Code, Codex and Gemini CLI: add hooks that give the agent the brief at session start and matching memories with each prompt, and save a checkpoint per branch (with your last three requests) in .memctrl/checkpoints/ |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.9.0` by `website/scripts/gen-cli.mjs`.</sub>
