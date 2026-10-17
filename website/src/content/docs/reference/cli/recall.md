---
title: "memctrl recall"
description: "The memories that match a prompt, minus what this session already saw (for a prompt hook)"
sidebar:
  label: "recall"
editUrl: false
---

The memories that match a prompt, minus what this session already saw (for a prompt hook)

```text
memctrl recall [OPTIONS] [PROMPT]
```

| Argument | Description |
| --- | --- |
| `[PROMPT]` | Prompt text (with --format hook: read from the hook's JSON on stdin) |

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--limit <LIMIT>` | At most this many memories added to the request | `5` |  |
| `--layer <LAYERS>` | Layer to include (repeatable; default: project and user) |  |  |
| `--format <FORMAT>` | Possible values: - markdown: Markdown for people and AGENTS.md - json:     Machine-readable entries plus the Markdown - hook:     Claude Code hook output (additionalContext), reading the hook's JSON from stdin | `markdown` |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.6.3` by `website/scripts/gen-cli.mjs`.</sub>
