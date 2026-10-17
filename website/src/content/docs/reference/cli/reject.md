---
title: "memctrl reject"
description: "Reject a memory"
sidebar:
  label: "reject"
editUrl: false
---

Reject a memory

```text
memctrl reject [OPTIONS] <MEMORY_ID>
```

| Argument | Description |
| --- | --- |
| `<MEMORY_ID>` | The memory id, or its first 8 characters |

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--reason <REASON>` | Why, kept with the rejection |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.6.3` by `website/scripts/gen-cli.mjs`.</sub>
