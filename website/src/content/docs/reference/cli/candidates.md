---
title: "memctrl candidates"
description: "List candidate memories awaiting review"
sidebar:
  label: "candidates"
editUrl: false
---

List candidate memories awaiting review

```text
memctrl candidates [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--layer <LAYER>` | Only this layer |  |  |
| `--limit <LIMIT>` | At most this many candidates, newest first | `50` |  |
| `--include-history` | Include expired and inactive lifecycle records |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.7.0` by `website/scripts/gen-cli.mjs`.</sub>
