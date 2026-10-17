---
title: "memctrl spans"
description: "Inspect persisted operation spans"
sidebar:
  label: "spans"
editUrl: false
---

Inspect persisted operation spans

```text
memctrl spans [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--name <NAME>` | Only spans whose operation name contains this |  |  |
| `--demo` | Record one demo span first |  |  |
| `--limit <LIMIT>` | At most this many spans | `50` |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.7.0` by `website/scripts/gen-cli.mjs`.</sub>
