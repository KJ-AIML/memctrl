---
title: "memctrl eval"
description: "Measure retrieval quality on a suite (built in, or --file) without touching your database"
sidebar:
  label: "eval"
editUrl: false
---

Measure retrieval quality on a suite (built in, or --file) without touching your database

```text
memctrl eval [OPTIONS]
```

| Option | Description | Default | Environment |
| --- | --- | --- | --- |
| `--file <FILE>` | JSON suite: {name, memories:[{key,content,...}], cases:[{query,expect,kind}]} |  |  |
| `--retriever <RETRIEVER>` | Possible values: - lexical - legacy - both - semantic: Lexical fused with matches by meaning; needs [semantic] in your user config - recall:   What the prompt hook would add (semantic too when [semantic] is on) | `both` |  |
| `--top-k <TOP_K>` | How many results count as a hit | `5` |  |
| `--min-hit-rate <MIN_HIT_RATE>` | Fail unless lexical hit@k on answerable cases reaches this rate (0-1) |  |  |

Every command also takes the [global options](/memctrl/reference/cli/#global-options) `--db` and `--rc`.

<sub>Generated from `memctrl 2.6.2` by `website/scripts/gen-cli.mjs`.</sub>
