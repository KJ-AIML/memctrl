---
title: "memctrl trigger"
description: "Fire a configured trigger"
sidebar:
  label: "trigger"
editUrl: false
---

Fire a configured trigger

```text
memctrl trigger [OPTIONS] <EVENT>
```

| Argument | Description |
| --- | --- |
| `<EVENT>` | The trigger name from `[triggers]` in `.memoryrc`, such as `on_commit` |

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--context <CONTEXT>` | A JSON object passed to the trigger |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.6.2` by `website/scripts/gen-cli.mjs`.</sub>
