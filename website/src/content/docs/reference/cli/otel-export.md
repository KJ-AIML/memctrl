---
title: "memctrl otel-export"
description: "Export persisted spans as JSON or OTLP JSON"
sidebar:
  label: "otel-export"
editUrl: false
---

Export persisted spans as JSON or OTLP JSON

```text
memctrl otel-export [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--output <OUTPUT>` | File to write | `memctrl_spans.json` |  |
| `--otlp` | Write OTLP JSON instead of memctrl spans |  |  |
| `--limit <LIMIT>` | At most this many spans | `1000` |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.7.0` by `website/scripts/gen-cli.mjs`.</sub>
