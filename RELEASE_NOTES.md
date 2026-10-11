# MemCtrl 2.0.0 — native Rust runtime

MemCtrl 2.0 replaces the Python runtime with a single native Rust executable: the same CLI commands and MCP tools, local SQLite storage, and no Python or virtualenv to manage. Existing 1.x databases migrate in place.

## Highlights

- **One binary, fast start.** Startup is about 5 ms instead of about 110 ms (median `--version` on the author's machine; not a general throughput claim). Prebuilt binaries for macOS (Apple silicon and Intel), Linux (x86_64 and arm64, static musl) and Windows.
- **Same surface.** All 1.3 CLI commands and the 7 MCP tools are present, plus `supersede`, `reinforce`, `maintenance`, `stats` and `agent`. Output is JSON; errors go to stderr with a nonzero exit.
- **Safer by construction.**
  - Every write is sanitized at one boundary, and redaction now covers numbers inside JSON content and secret values containing punctuation.
  - Mutations are atomic and take the write lock up front, so concurrent writers wait instead of failing.
  - Refuted claims stay refuted.
- **Native AI providers.** Optional `--llm` requests go to OpenAI, xAI (Responses API), Anthropic (Messages API) or any OpenAI-compatible server such as Ollama. Each provider uses its own structured-output format, with bounded retries and keys read from environment variables. The design is modeled on xAI's Grok Build CLI.
- **Explainable retrieval with honest provenance.** Each answer records the method (`keyword_tree` or `llm_selection`) and the store revision it used. LLM selections are re-checked under the writer lock before they are returned.
- **Migration you can trust.** Databases written by the 1.x Python store (schemas v2, v3 and v4) migrate with memories, tags, tree, audit, provenance, spans, relations and evidence retained. This is covered by tests built from real Python-generated databases.

## Breaking changes

Read [MIGRATION.md](MIGRATION.md) before opening an existing database. The most visible changes:

- Python packaging and the Python API are gone. Use the CLI, MCP server or JSON agent node instead of LangGraph Python classes.
- History-mode retrieval with `[REFUTED]`-style annotations is removed. Use `list --include-history` instead.
- Refutation is terminal. To revise a refuted claim, add a new memory and `supersede` the old one.
- Decay runs only through `memctrl decay`. Schedule `memctrl decay --min-hours 24` to keep the old daily behavior.
- Naive timestamps from 1.x are read as UTC.

## Install

```bash
# macOS / Linux: prebuilt binary, verified against SHA256SUMS
curl -fsSL https://raw.githubusercontent.com/KJ-AIML/memctrl/main/install.sh | sh

# Any platform with Rust
cargo install --locked memctrl
```

Windows: download `memctrl-x86_64-pc-windows-msvc.zip` from the release and put `memctrl.exe` on your `PATH`.

## Upgrading from 1.x

1. Stop writers and back up the database. Use SQLite backup, or copy only after every writer has closed.
2. Install 2.0 with one of the commands above.
3. Run `memctrl --db <backup> stats`, `doctor` and a few queries against the backup first.
4. Point your MCP client at the `memctrl serve` executable.

Previous release notes live in [docs/history](docs/history).
