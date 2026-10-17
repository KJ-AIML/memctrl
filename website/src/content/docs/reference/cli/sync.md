---
title: "memctrl sync"
description: "After pulling: import memory.md (new entries as candidates, retractions applied), then export your store merged with it"
sidebar:
  label: "sync"
editUrl: false
---

After pulling: import memory.md (new entries as candidates, retractions applied), then export your store merged with it

```text
memctrl sync [OPTIONS] [FILE]
```

| Argument | Description |
| --- | --- |
| `[FILE]` | Export to sync (default: .memctrl/memory.md in the project folder) |

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--layer <LAYERS>` | Layer to sync (repeatable; default: project) |  |  |
| `--accept` | You have read the file's changes: accept new entries (asks in a terminal) |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.6.2` by `website/scripts/gen-cli.mjs`.</sub>
