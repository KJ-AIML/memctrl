---
title: "memctrl tree"
description: "Show the memory tree (layers, clusters, counts); --rebuild builds it again"
sidebar:
  label: "tree"
editUrl: false
---

Show the memory tree (layers, clusters, counts); --rebuild builds it again

```text
memctrl tree [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--rebuild` | Build the tree again from the current memories |  |  |
| `--layer <LAYER>` | Only this layer |  |  |
| `--limit <LIMIT>` | Memory ids listed per cluster | `20` |  |
| `--llm` | Explicitly enable calls to an LLM provider |  |  |
| `--llm-provider <LLM_PROVIDER>` | openai, xai, anthropic or compatible (default: MEMCTRL_LLM_PROVIDER or openai) |  |  |
| `--llm-model <LLM_MODEL>` | Model name (default: MEMCTRL_LLM_MODEL; required for xai and anthropic) |  |  |
| `--llm-base-url <LLM_BASE_URL>` | Base URL, e.g. `http://localhost:11434/v1` for Ollama |  |  |
| `--llm-api-key-env <LLM_API_KEY_ENV>` | Name of the environment variable that holds the API key |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.6.2` by `website/scripts/gen-cli.mjs`.</sub>
