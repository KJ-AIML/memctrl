---
title: "memctrl cite"
description: "Link an existing memory to the code it describes"
sidebar:
  label: "cite"
editUrl: false
---

Link an existing memory to the code it describes

```text
memctrl cite [OPTIONS] <MEMORY_ID> <PATH[:LINE[-LINE]]>...
```

| Argument | Description |
| --- | --- |
| `<MEMORY_ID>` | The memory id, or its first 8 characters |
| `<PATH[:LINE[-LINE]]>...` | Code the memory describes: a file, a line or a line range, relative to the project root |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.6.2` by `website/scripts/gen-cli.mjs`.</sub>
