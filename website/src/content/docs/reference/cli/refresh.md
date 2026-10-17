---
title: "memctrl refresh"
description: "Re-anchor a memory's citations to the current code after re-checking the memory"
sidebar:
  label: "refresh"
editUrl: false
---

Re-anchor a memory's citations to the current code after re-checking the memory

```text
memctrl refresh [OPTIONS] <MEMORY_ID>
```

| Argument | Description |
| --- | --- |
| `<MEMORY_ID>` | The memory id, or its first 8 characters |

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--cite <PATH:START-END>` | Set the range for a path the memory cites, when its code can't be followed automatically (repeatable) |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.6.2` by `website/scripts/gen-cli.mjs`.</sub>
