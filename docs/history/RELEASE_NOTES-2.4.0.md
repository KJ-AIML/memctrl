# MemCtrl 2.4.0: search by meaning, when you want it

Word search can't find a question worded unlike its memory: "which JWT library" never finds "we verify tokens with jose". 2.4 can also match by meaning, with an embedding model you run (Ollama, LM Studio, llama.cpp or any OpenAI-compatible server). It's off until you turn it on. With it off, every command returns what 2.3 returned.

```toml
# ~/.config/memctrl/config.toml
[semantic]
model = "embeddinggemma"     # ollama pull embeddinggemma; served at http://localhost:11434/v1 by default
```

## Highlights

- **Questions find memories worded differently.** `query`, the MCP server and the prompt hook fuse word matches with matches by meaning. With EmbeddingGemma, the answer is in the top 5 for 95% of real-dev questions (92% with words only) and all of the Thai held-out questions (92%). A similarity threshold keeps vague questions answered with nothing. That held on the Thai and English vague questions, and two suites lost one question each. [Full results](docs/benchmarks/semantic-search.md).
- **The prompt hook stays quiet.** It adds a memory on meaning alone only above a stricter recall threshold, which none of 15 unrelated everyday prompts reached. It now adds the answer to 85% of real-dev questions (82%).
- **Measured, not guessed.** Thresholds ship for EmbeddingGemma and multilingual-e5-small. `memctrl semantic tune` measures them for any other model on the built-in suite, and semantic search stays off until you set them. `memctrl eval --retriever semantic` and `--retriever recall` measure it on your own suite.
- **Only you turn it on.** A repository's `.memoryrc` can't enable it or pick the server. A server off this machine needs `allow_remote = true` and HTTPS, because the prompt hook sends every request to it. When the model is down, searches fall back to word matches and say so. Deleting or editing a memory drops its vector, even when an older memctrl does it.
- **New commands:** `memctrl semantic status`, `index`, `tune` and `clear`.

## What it doesn't change

In the agent benchmark, the prompt hook added exactly the same memories to all 16 tasks with or without meaning, because those requests share words with the memories they need. Agents got 14 of 16 tasks right in both semantic setups, as with 2.3.0. The env-var preference agents miss isn't reachable this way either: "add an env var" and "document every new env var in the README" are linked by inference, at a similarity of 0.29. Search by meaning helps when people ask in their own words, especially in Thai, more than with requests that already name what they need.

## Upgrade notes

See the 2.3 → 2.4 section of [MIGRATION.md](MIGRATION.md). Nothing changes unless you add `[semantic]`. The database gains a table only when the first vector is stored, so 2.0 through 2.4 keep sharing one store.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/KJ-AIML/memctrl/main/install.sh | sh
```

Windows: download `memctrl-x86_64-pc-windows-msvc.zip` from the release and put `memctrl.exe` on your `PATH`.

Previous release notes live in [docs/history](docs/history).
