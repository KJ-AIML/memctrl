---
title: "memctrl refute"
description: "Refute a memory; refuted claims cannot be accepted"
sidebar:
  label: "refute"
editUrl: false
---

Refute a memory; refuted claims cannot be accepted

```text
memctrl refute [OPTIONS] --reason <REASON> <MEMORY_ID>
```

| Argument | Description |
| --- | --- |
| `<MEMORY_ID>` | The memory id, or its first 8 characters |

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `-r, --reason <REASON>` | Why the memory is wrong |  |  |
| `--refuting-id <REFUTING_ID>` | A memory that shows it is wrong, linked as evidence |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.6.2` by `website/scripts/gen-cli.mjs`.</sub>
