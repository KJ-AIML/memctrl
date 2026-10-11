# MemCtrl 2.1: Rust

Local, explainable memory for coding agents. The executable, library, CLI, MCP server, tests, examples and benchmarks are native Rust. No Python runtime or bridge is included.

MemCtrl stores facts in SQLite and ranks them with SQLite's built-in full-text search: BM25 over Porter-stemmed words, plus character trigrams for Thai, Chinese, Japanese, Korean and other scripts written without spaces. Every result records why it matched. Lifecycle and verification states keep candidates, expired facts and refuted claims out of ordinary retrieval.

It needs no model and downloads none. Optional LLM requests (only with `--llm`) go to OpenAI, xAI, Anthropic or any OpenAI-compatible server such as Ollama.

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

Windows: download `memctrl-x86_64-pc-windows-msvc.zip` from [Releases](https://github.com/KJ-AIML/memctrl/releases) and put `memctrl.exe` on your `PATH`. Upgrading from 1.x or 2.0? Read [MIGRATION.md](MIGRATION.md) first.

```bash
memctrl init
memctrl add "We chose SQLx over Diesel: compile-time checks caught 3 prod bugs" \
  --layer project --claim-type decision --cite src/db/mod.rs:12-40
memctrl query "sqlx diesel"
memctrl context                                  # the session brief an agent sees
memctrl install --tool claude --project --hook   # Claude Code loads the brief every session
```

For local development, `make ci` runs exactly the checks GitHub CI runs (fmt, strict Clippy, tests, release build, Rust-only source). `make hooks` makes git run it before every push.

The built CLI is `target/release/memctrl` (`memctrl.exe` on Windows). Run `memctrl --help` and command-specific `--help` for options. Responses are structured JSON; errors go to stderr with a nonzero exit status.

## Session brief

Agents skip optional lookups, so memory has to arrive without being asked for. `memctrl context` prints the highest-trust memories that fit a token budget (default 1500, from the `project` and `user` layers). Ranking weighs verification, claim type, confidence and how often retrieval returns a memory. Output is grouped into Decisions, Facts and conventions, Lessons and Open hypotheses. Disputed memories, and memories whose cited code changed, go under **Check before trusting**.

| Host | How it loads |
| --- | --- |
| Claude Code | `memctrl install --tool claude --hook` adds a SessionStart hook. With `--project` it goes in this repo's `.claude/settings.local.json`, which is personal and not committed; otherwise it goes in your user settings. It also runs after `/clear`, resume and compaction. |
| Codex, Cursor, others | `memctrl context --write AGENTS.md` refreshes a marked block in AGENTS.md. |
| Any MCP host | the `memctrl_context` tool |

The brief records which memories it showed (provenance method `context_brief`), but it does not count as access, so it can't keep promoting itself. Where no memctrl store exists, it prints nothing and creates nothing.

## Search

`memctrl query` finds memories by the words they contain: names, paths, commands, error text. Each fact comes with `why`:

```json
{"memory_id":"…","score":1.35,"coverage":0.5,"matched":["prod → production (alias)"],"freshness":"fresh"}
```

- Rare words outweigh common ones (BM25). Porter stemming matches `testing` to `tests` and `deploying` to `deploys`.
- A word with no hits retries as a substring, so `kube` finds `Kubernetes` and `PostgreSQL` is found inside `订单服务使用PostgreSQL`.
- Synonym groups ship built in (`db`/`database`, `prod`/`production`, `k8s`/`kubernetes`, `pg`/`postgres`, …). Add your own in `.memoryrc`:

  ```toml
  [retrieval]
  aliases = [["orm", "sqlx", "diesel"], ["ci", "github actions"]]
  builtin_aliases = true
  ```

- Generic phrases ("current project context") match nothing; that is what the brief is for.

## Citations

A memory can cite the code it describes. memctrl hashes the cited lines and checks them whenever the memory is used, with no git, network or model involved.

```bash
memctrl add "get_by_id hides soft-deleted users" --layer project --cite src/user.rs:3-5
memctrl cite <id> docs/adr/007-sqlx.md          # add a citation to an existing memory
memctrl stale                                   # memories whose cited code changed or vanished
memctrl refresh <id>                            # after re-checking, re-anchor (audited)
```

Statuses are `fresh`, `moved` (the same lines now start elsewhere), `changed`, `missing`, `unresolved` (outside the project root) and `unchecked` (not a regular file, or over 8 MB). Paths are relative to the project root. The root is `MEMCTRL_PROJECT_ROOT`, else `[citations] root` in `.memoryrc`, else the nearest directory containing `.git`.

Secrets files (`.env`, keys, `.npmrc`, files under `.ssh`, and so on) can't be cited: citation hashes travel in exports, and a short secret can be guessed back from its hash.

## Safe writes and review

Memory is replayed into every future session, so a stored instruction is a persistent prompt injection. A new memory becomes a **candidate** instead of active knowledge when it contains:
- hidden Unicode: any invisible character, bidi controls, tag characters. Joiners and selectors inside emoji or non-Latin text are allowed.
- instruction-like text: "ignore previous instructions", fake `<system>` tags, `curl … | sh`, "send the API keys to …". Matching ignores invisible characters and common lookalike letters.

`add` then reports `"status": "quarantined"` with the reasons. This is a heuristic filter: it never blocks, and determined phrasing can still get past it. Reviewing candidates is the safety net. Review with `memctrl candidates`, then `memctrl accept <id>` or `memctrl reject <id>`.

```toml
[guard]
mode = "flag"                               # "strict" also holds every source not listed below; "off" disables
trusted_sources = ["manual", "user", "cli"]
```

## Share through git

```bash
memctrl export            # writes .memctrl/memory.md; commit it and review changes in PRs
memctrl export --check    # in CI: fail when the file is out of date
memctrl import .memctrl/memory.md
```

The export is plain Markdown: readable entries plus metadata comments (identity, states, citations, a content hash). Import never raises trust:
- Local memories are never overwritten. Unchanged entries keep their identity.
- Edited entries come back as candidates, and so do new `- ` bullets a teammate writes in a PR, filed under the section heading they sit beneath.
- A memory you `forget` stays forgotten.
- Text you already hold in any state, refuted included, is not added again.
- Verification arrives as `unverified` unless you pass `--trust-verification`, since anyone who can edit the file can write metadata.

`.memctrl/.gitignore` keeps the database out of git.

## Measuring retrieval

```bash
memctrl eval                        # built-in developer-memory suite; never touches your database
memctrl eval --file my-suite.json   # your own memories and queries
memctrl eval --min-hit-rate 0.9     # CI gate
```

Built-in suite (30 memories, 48 queries), k=5:

| | lexical (2.1) | legacy keyword scorer (2.0) |
| --- | --- | --- |
| hit@k on answerable queries | 0.927 | 0.756 |
| MRR | 0.915 | 0.728 |
| unanswerable queries correctly empty | 0.857 | 0.429 |
| Thai, Chinese, Japanese, Korean | 10 / 10 | 2 / 10 |
| median latency | 0.13 ms | 2.8 ms |

The same author wrote the suite and the retriever, so treat these as regression floors, not an independent benchmark. Paraphrases with no shared words ("which ORM did we pick") are a known limit of lexical search: 3 of 6 are found. Aliases close the gaps that matter to your team.

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

Requests time out after 120 s. Transport errors and 5xx responses are retried up to `MEMCTRL_LLM_MAX_RETRIES` times (default 2) with exponential backoff. A 429 is retried once, and only when `Retry-After` is 30 s or less. Other 4xx responses fail immediately. If a provider rejects its structured-output field, the request is repeated once without it, and responses are always validated locally. The design follows xAI's open-source Grok Build CLI (`xai-org/grok-build`).

## Behavior

- `project` facts have no default expiry. `session` defaults to seven days and `user` to 90 days. Configure layers, triggers, retention, aliases, the guard and the citation root in `.memoryrc` TOML (`memctrl init` writes a commented default).
- Retrieval needs no network, model or embeddings. Cache identity includes query, layer, result limit, alias settings and temporal eligibility. Mutations invalidate caches across SQLite connections. Full-text indexes are maintained by triggers for every writer, and `memctrl maintenance` rebuilds them.
- Superseding, refuting, evidence links, citations and consolidation use transactions. Reflection keeps source facts and creates candidate lessons that need review.
- Credentials and PII in text and nested JSON are redacted before storage and external text output. Identifiers that carry credentials are rejected, which preserves lookup identity.
- Explicit decay and reinforcement update confidence. Local provenance and OpenTelemetry spans support inspection and OTLP JSON export.
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

The server uses the official Rust MCP SDK over stdio, and stdout carries only protocol messages. Configuration reloads for each tool request. It has nine tools:
- `memctrl_context`
- `memctrl_query`
- `memctrl_add` (with `claim_type`, `tags` and `cite`)
- `memctrl_stale`
- `memctrl_trigger`
- `memctrl_tree`
- `memctrl_audit`
- `memctrl_candidates`
- `memctrl_review`

See [MIGRATION.md](MIGRATION.md) before opening an existing database, [RUST-RESEARCH.md](RUST-RESEARCH.md) for research and design tradeoffs, [docs/PLAN-2.1.md](docs/PLAN-2.1.md) for the 2.1 design, and [docs/RUST-VERIFICATION.md](docs/RUST-VERIFICATION.md) for current checks and limitations. Historical documents in `docs/history` describe earlier releases.
