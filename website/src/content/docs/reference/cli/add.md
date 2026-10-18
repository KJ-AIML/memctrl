---
title: "memctrl add"
description: "Store a memory (or explicitly request LLM extraction)"
sidebar:
  label: "add"
editUrl: false
---

Store a memory (or explicitly request LLM extraction)

```text
memctrl add [OPTIONS] <CONTENT>
```

| Argument | Description |
| --- | --- |
| `<CONTENT>` | The memory, one fact per memory |

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--layer <LAYER>` | `project` (no default expiry), `user` (90 days) or `session` (7 days) | `session` |  |
| `--source <SOURCE>` | Where the memory came from (default: manual). Run by a coding agent, it records the agent instead (such as claude-code), and never a source the guard trusts |  |  |
| `--confidence <CONFIDENCE>` | From 0 to 1; weighs the memory in the brief and in search | `1` |  |
| `--claim-type <CLAIM_TYPE>` | `assertion`, `observation`, `decision`, `hypothesis` or `derived_lesson` | `assertion` |  |
| `--lifecycle-state <LIFECYCLE_STATE>` | `accepted`, `candidate`, `superseded`, `rejected` or `archived` | `accepted` |  |
| `--verification-state <VERIFICATION_STATE>` | `unverified`, `supported`, `disputed` or `refuted` | `unverified` |  |
| `--tags <TAGS>` | A tag (repeatable). Tags starting with `guard:` or `import:` are reserved |  |  |
| `--expires-at <EXPIRES_AT>` | RFC 3339 time after which the memory stops being retrieved |  |  |
| `--cite <PATH[:LINE[-LINE]]>` | Code this memory describes (repeatable); memctrl flags the memory when it changes |  |  |
| `--llm` | Explicitly enable calls to an LLM provider |  |  |
| `--llm-provider <LLM_PROVIDER>` | openai, xai, anthropic or compatible (default: MEMCTRL_LLM_PROVIDER or openai) |  |  |
| `--llm-model <LLM_MODEL>` | Model name (default: MEMCTRL_LLM_MODEL; required for xai and anthropic) |  |  |
| `--llm-base-url <LLM_BASE_URL>` | Base URL, e.g. `http://localhost:11434/v1` for Ollama |  |  |
| `--llm-api-key-env <LLM_API_KEY_ENV>` | Name of the environment variable that holds the API key |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.7.0` by `website/scripts/gen-cli.mjs`.</sub>
