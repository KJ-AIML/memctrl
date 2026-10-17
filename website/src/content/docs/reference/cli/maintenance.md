---
title: "memctrl maintenance"
description: "Checkpoint SQLite WAL and report store statistics"
sidebar:
  label: "maintenance"
editUrl: false
---

Checkpoint SQLite WAL and report store statistics

```text
memctrl maintenance [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--vacuum` | Also rewrite the database to reclaim free space (takes the write lock briefly) |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.7.0` by `website/scripts/gen-cli.mjs`.</sub>
