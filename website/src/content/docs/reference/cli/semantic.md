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
| [`setup`](#memctrl-semantic-setup) | Find an embedding model on this machine (Ollama, LM Studio) and turn search by meaning on: adds [semantic] to your user config and indexes this project's memories |
| [`status`](#memctrl-semantic-status) | Settings, whether the model answers, and how many memories have vectors |
| [`index`](#memctrl-semantic-index) | Embed every memory without a vector (searches embed a few on the spot) |
| [`tune`](#memctrl-semantic-tune) | Measure similarity thresholds for your model on an eval suite (built in, or --file) |
| [`clear`](#memctrl-semantic-clear) | Delete every stored vector; they're rebuilt as you search while [semantic] is on |

### `memctrl semantic setup`

Find an embedding model on this machine (Ollama, LM Studio) and turn search by meaning on: adds [semantic] to your user config and indexes this project's memories

```text
memctrl semantic setup [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--base-url <BASE_URL>` | An OpenAI-compatible server on this machine to use instead of looking for Ollama and LM Studio |  |  |
| `--dry-run` | Show what would be written, without writing or indexing |  |  |

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

<sub>Generated from `memctrl 2.8.0` by `website/scripts/gen-cli.mjs`.</sub>
