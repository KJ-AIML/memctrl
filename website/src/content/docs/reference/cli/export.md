---
title: "memctrl export"
description: "Write shared memory to a Markdown file for review in pull requests"
sidebar:
  label: "export"
editUrl: false
---

Write shared memory to a Markdown file for review in pull requests

```text
memctrl export [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--out <OUT>` | Output file (default: .memctrl/memory.md in the project folder) |  |  |
| `--layer <LAYERS>` | Layer to export (repeatable; default: project) |  |  |
| `--check` | Fail if the file is not up to date instead of writing it |  |  |
| `--force` | Write from your store alone, dropping entries it hasn't imported and retraction records it doesn't know |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.6.3` by `website/scripts/gen-cli.mjs`.</sub>
