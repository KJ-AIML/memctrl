---
title: "memctrl query"
description: "Retrieve relevant active memories with provenance"
sidebar:
  label: "query"
editUrl: false
---

Retrieve relevant active memories with provenance

```text
memctrl query [OPTIONS] <QUERY_TEXT>
```

| Argument | Description |
| --- | --- |
| `<QUERY_TEXT>` | What to look for: names, paths, commands and error text work best |

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--layer <LAYER>` | Only this layer |  |  |
| `--limit <LIMIT>` | At most this many results | `10` |  |
| `--llm` | Explicitly enable calls to an LLM provider |  |  |
| `--llm-provider <LLM_PROVIDER>` | openai, xai, anthropic or compatible (default: MEMCTRL_LLM_PROVIDER or openai) |  |  |
| `--llm-model <LLM_MODEL>` | Model name (default: MEMCTRL_LLM_MODEL; required for xai and anthropic) |  |  |
| `--llm-base-url <LLM_BASE_URL>` | Base URL, e.g. `http://localhost:11434/v1` for Ollama |  |  |
| `--llm-api-key-env <LLM_API_KEY_ENV>` | Name of the environment variable that holds the API key |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.8.0` by `website/scripts/gen-cli.mjs`.</sub>
