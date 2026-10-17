---
title: "memctrl semantic"
description: "Optional semantic search, set up under [semantic] in your user config"
sidebar:
  label: "semantic"
editUrl: false
---

Optional semantic search, set up under [semantic] in your user config

```text
memctrl semantic [OPTIONS] <COMMAND>
```

## Subcommands

| Subcommand | What it does |
| --- | --- |
| [`status`](#memctrl-semantic-status) | Settings, whether the model answers, and how many memories have vectors |
| [`index`](#memctrl-semantic-index) | Embed every memory without a vector (searches embed a few on the spot) |
| [`tune`](#memctrl-semantic-tune) | Measure similarity thresholds for your model on an eval suite (built in, or --file) |
| [`clear`](#memctrl-semantic-clear) | Delete every stored vector; they're rebuilt as you search while [semantic] is on |

### `memctrl semantic status`

Settings, whether the model answers, and how many memories have vectors

```text
memctrl semantic status [OPTIONS]
```

### `memctrl semantic index`

Embed every memory without a vector (searches embed a few on the spot)

```text
memctrl semantic index [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--layer <LAYER>` | Only embed memories in this layer |  |  |

### `memctrl semantic tune`

Measure similarity thresholds for your model on an eval suite (built in, or --file)

```text
memctrl semantic tune [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--file <FILE>` | An eval suite to tune on instead of the built-in one |  |  |
| `--top-k <TOP_K>` | How many results count as a hit | `5` |  |

### `memctrl semantic clear`

Delete every stored vector; they're rebuilt as you search while [semantic] is on

```text
memctrl semantic clear [OPTIONS]
```

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.6.2` by `website/scripts/gen-cli.mjs`.</sub>
