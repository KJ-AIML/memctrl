---
title: "memctrl decay"
description: "Run confidence decay or inspect low-confidence records"
sidebar:
  label: "decay"
editUrl: false
---

Run confidence decay or inspect low-confidence records

```text
memctrl decay [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--dry-run` | Only list low-confidence memories; change nothing |  |  |
| `--threshold <THRESHOLD>` | List memories whose confidence is below this | `0.3` |  |
| `--min-hours <MIN_HOURS>` | Skip decay if it ran less than this many hours ago; scheduled runs apply the elapsed time | `0` |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.8.0` by `website/scripts/gen-cli.mjs`.</sub>
