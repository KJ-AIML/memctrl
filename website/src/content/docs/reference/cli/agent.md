---
title: "memctrl agent"
description: "JSON agent checkpoint and memory-node APIs"
sidebar:
  label: "agent"
editUrl: false
---

JSON agent checkpoint and memory-node APIs

```text
memctrl agent [OPTIONS] <COMMAND>
```

## Subcommands

| Subcommand | What it does |
| --- | --- |
| [`put`](#memctrl-agent-put) | Save arbitrary JSON state for a thread |
| [`get`](#memctrl-agent-get) | Load a thread checkpoint |
| [`list`](#memctrl-agent-list) | List recent checkpoints |
| [`node`](#memctrl-agent-node) | Read memories and extract new ones from JSON agent state |

### `memctrl agent put`

Save arbitrary JSON state for a thread

```text
memctrl agent put [OPTIONS] <THREAD_ID> <STATE>
```

| Argument | Description |
| --- | --- |
| `<THREAD_ID>` | The thread to save state for |
| `<STATE>` | A JSON object |

### `memctrl agent get`

Load a thread checkpoint

```text
memctrl agent get [OPTIONS] <THREAD_ID>
```

| Argument | Description |
| --- | --- |
| `<THREAD_ID>` | The thread to load |

### `memctrl agent list`

List recent checkpoints

```text
memctrl agent list [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--limit <LIMIT>` | At most this many checkpoints | `50` |  |

### `memctrl agent node`

Read memories and extract new ones from JSON agent state

```text
memctrl agent node [OPTIONS] <STATE>
```

| Argument | Description |
| --- | --- |
| `<STATE>` | A JSON object of agent state |

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--persist` | Save the extracted memories |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.6.2` by `website/scripts/gen-cli.mjs`.</sub>
