---
title: "memctrl import"
description: "Import a memctrl export; never overwrites local memories. New entries arrive as candidates unless you pass --accept after reading the file's changes"
sidebar:
  label: "import"
editUrl: false
---

Import a memctrl export; never overwrites local memories. New entries arrive as candidates unless you pass --accept after reading the file's changes

```text
memctrl import [OPTIONS] <FILE>
```

| Argument | Description |
| --- | --- |
| `<FILE>` | A memctrl export, usually `.memctrl/memory.md` |

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--layer <LAYERS>` | Layer to import (repeatable; default: project). Hand-written entries go to the first |  |  |
| `--accept` | You have read the file's changes: accept new entries instead of holding them as candidates |  |  |
| `--dry-run` | Report what would change without changing anything |  |  |
| `--trust-verification` | Keep `supported` verification from the file (by default imported claims arrive unverified) |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.6.3` by `website/scripts/gen-cli.mjs`.</sub>
