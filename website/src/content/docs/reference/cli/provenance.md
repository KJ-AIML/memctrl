---
title: "memctrl provenance"
description: "Show retrieval provenance"
sidebar:
  label: "provenance"
editUrl: false
---

Show retrieval provenance

```text
memctrl provenance [OPTIONS] [QUERY]
```

| Argument | Description |
| --- | --- |
| `[QUERY]` | Only records for exactly this query |

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--full` | Accepted for compatibility; records are always complete |  |  |
| `--limit <LIMIT>` | At most this many records | `50` |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.8.0` by `website/scripts/gen-cli.mjs`.</sub>
