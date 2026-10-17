---
title: "memctrl list"
description: "List memories"
sidebar:
  label: "list"
editUrl: false
---

List memories

```text
memctrl list [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--layer <LAYER>` | Only this layer |  |  |
| `--limit <LIMIT>` | At most this many memories, newest first | `50` |  |
| `--include-history` | Include expired and inactive lifecycle records |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.6.2` by `website/scripts/gen-cli.mjs`.</sub>
