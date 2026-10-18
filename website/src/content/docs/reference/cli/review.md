---
title: "memctrl review"
description: "Review external task history"
sidebar:
  label: "review"
editUrl: false
---

Review external task history

```text
memctrl review [OPTIONS] <COMMAND>
```

## Subcommands

| Subcommand | What it does |
| --- | --- |
| [`heli`](#memctrl-review-heli) | Distill candidate lessons from Heli task history, read-only by default |

### `memctrl review heli`

Distill candidate lessons from Heli task history, read-only by default

```text
memctrl review heli [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `-w, --workspace <WORKSPACE>` | The Heli workspace to read | `.` |  |
| `-c, --completed <COMPLETED>` | How many recently completed tasks to read | `20` |  |
| `--persist` | Save the lessons as candidates (default: only report them) |  |  |
| `--dry-run` | Report only (the default) |  |  |
| `--repo <REPO>` | Only tasks for this repository |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.8.0` by `website/scripts/gen-cli.mjs`.</sub>
