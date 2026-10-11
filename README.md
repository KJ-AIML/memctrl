# MemCtrl 2.0 — Rust

Local, explainable memory for coding agents. The executable, library, CLI, MCP server, tests, examples and benchmarks are native Rust. No Python runtime or bridge is included.

MemCtrl stores facts in SQLite, groups them into a hierarchy, retrieves them with deterministic keyword scoring, and records their sources and reasoning trace. Lifecycle and verification state keep candidates, expired facts and refuted claims out of ordinary retrieval. Optional LLM requests (only with `--llm`) go to OpenAI, xAI, Anthropic or any OpenAI-compatible server such as Ollama.

## Install

```bash
# macOS / Linux: prebuilt binary, verified against SHA256SUMS
curl -fsSL https://raw.githubusercontent.com/KJ-AIML/memctrl/main/install.sh | sh

# Any platform with a Rust toolchain
cargo install --locked memctrl

# From source
git clone https://github.com/KJ-AIML/memctrl.git && cd memctrl
cargo install --path . --locked
```

Windows: download `memctrl-x86_64-pc-windows-msvc.zip` from [Releases](https://github.com/KJ-AIML/memctrl/releases) and put `memctrl.exe` on your `PATH`. Upgrading from 1.x? Read [MIGRATION.md](MIGRATION.md) first.

```bash
memctrl init
memctrl add "We use Rust and PostgreSQL" --layer project
memctrl query "database"
memctrl tree
memctrl doctor
```

For local development, `make ci` runs exactly the checks GitHub CI runs (fmt, strict Clippy, tests, release build, Rust-only source). `make hooks` makes git run it before every push.

The built CLI is `target/release/memctrl` (`memctrl.exe` on Windows). Run `memctrl --help` and command-specific `--help` for options. Responses are structured JSON; errors go to stderr with a nonzero exit status.

## Optional LLM providers

Everything works offline. Add `--llm` to a command to let a model extract memories, pick tree nodes or write summaries.

| Provider | `--llm-provider` | API | Key variable | Model |
| --- | --- | --- | --- | --- |
| OpenAI (default) | `openai` | Chat Completions, strict JSON schema | `OPENAI_API_KEY` | default `gpt-4o-mini` |
| xAI | `xai` (or `grok`) | Responses, strict JSON schema | `XAI_API_KEY` | set with `--llm-model` |
| Anthropic | `anthropic` | Messages, JSON schema output | `ANTHROPIC_API_KEY` | set with `--llm-model` |
| Local / compatible | `compatible` (or `ollama`) | Chat Completions, JSON mode | optional on loopback | set with `--llm-model` |

```bash
export XAI_API_KEY=...            # or keep it under any name and pass --llm-api-key-env NAME
memctrl query "database" --llm --llm-provider xai --llm-model <model>
memctrl add "$(cat notes.md)" --llm --llm-provider ollama --llm-model llama3 --llm-base-url http://localhost:11434/v1
```

Settings resolve in this order: flags (`--llm-provider`, `--llm-model`, `--llm-base-url`, `--llm-api-key-env`), then `MEMCTRL_LLM_PROVIDER` / `MEMCTRL_LLM_MODEL` / `MEMCTRL_LLM_BASE_URL` / `MEMCTRL_LLM_API_KEY`, then the provider's own key variable, then the preset. Keys never go in argv or config files.

Requests time out after 120 s. Transport errors and 5xx responses are retried up to `MEMCTRL_LLM_MAX_RETRIES` times (default 2), with exponential backoff. A 429 is retried once, and only when `Retry-After` is 30 s or less. Other 4xx responses fail immediately. If a provider rejects its structured-output field, the request is repeated once without it; responses are always validated locally. The design follows xAI's open-source Grok Build CLI (`xai-org/grok-build`).

## Behavior

- `project` facts have no default expiry; `session` defaults to seven days and `user` to 90 days. Configure layers, triggers, retention and confidence in `.memoryrc` TOML.
- Deterministic retrieval needs no network or embeddings. Cache identity includes query, layer, result limit and temporal eligibility. Mutations invalidate caches across SQLite connections.
- Superseding, refuting, evidence links and consolidation use transactions. Reflection retains source facts and creates candidate lessons that require review.
- Text and nested JSON credentials/PII are redacted before storage and external text output. Credential-bearing identifiers are rejected to preserve lookup identity.
- Explicit decay and reinforcement update confidence; local provenance and OpenTelemetry spans support inspection and OTLP JSON export.
- Heli history is read without modifying its source files. Imported lessons remain candidates with evidence links.
- Native JSON checkpoint and agent-state APIs replace Python-specific LangGraph inheritance.

## MCP

Point your MCP client at the native executable using absolute paths:

```json
{
  "mcpServers": {
    "memctrl": {
      "command": "/absolute/path/to/memctrl",
      "args": ["--db", "/absolute/path/to/memories.db", "--rc", "/absolute/path/to/.memoryrc", "serve"]
    }
  }
}
```

The server uses the official Rust MCP SDK over stdio. Stdout contains only protocol messages. Its seven tools are `memctrl_query`, `memctrl_add`, `memctrl_trigger`, `memctrl_tree`, `memctrl_audit`, `memctrl_candidates` and `memctrl_review`. Configuration reloads for each tool request.

See [MIGRATION.md](MIGRATION.md) before opening an existing database, [RUST-RESEARCH.md](RUST-RESEARCH.md) for research/design tradeoffs, and [docs/RUST-VERIFICATION.md](docs/RUST-VERIFICATION.md) for current checks and limitations. Historical documents in `docs/history` describe earlier releases.
