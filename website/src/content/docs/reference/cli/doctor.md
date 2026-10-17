---
title: "memctrl doctor"
description: "Report store health (always JSON)"
sidebar:
  label: "doctor"
editUrl: false
---

Report store health (always JSON)

```text
memctrl doctor [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--low-confidence-threshold <LOW_CONFIDENCE_THRESHOLD>` | Report memories whose confidence is below this | `0.5` |  |
| `--json` | Accepted for compatibility; the report is always JSON |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.7.0` by `website/scripts/gen-cli.mjs`.</sub>
