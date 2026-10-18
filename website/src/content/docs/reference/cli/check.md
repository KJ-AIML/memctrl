---
title: "memctrl check"
description: "Validate memory.md without a database (for CI): format, ids, hashes, layout, and with --base that no retraction was dropped"
sidebar:
  label: "check"
editUrl: false
---

Validate memory.md without a database (for CI): format, ids, hashes, layout, and with --base that no retraction was dropped

```text
memctrl check [OPTIONS] [FILE]
```

| Argument | Description |
| --- | --- |
| `[FILE]` | Export to check (default: .memctrl/memory.md in the project folder) |

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--base <GIT_REF>` | Compare with this commit's version of the file (e.g. origin/main) |  |  |
| `--strict` | Treat warnings (edited or hand-written entries, layout) as errors |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.7.0` by `website/scripts/gen-cli.mjs`.</sub>
