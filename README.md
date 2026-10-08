# MemCtrl 2.0 — Rust

Local, explainable memory for coding agents. The executable, library, CLI, MCP server, tests, examples and benchmarks are native Rust. No Python runtime or bridge is included.

MemCtrl stores facts in SQLite, groups them into a hierarchy, retrieves them with deterministic keyword scoring, and records their sources and reasoning trace. Lifecycle and verification state keep candidates, expired facts and refuted claims out of ordinary retrieval. Optional LLM requests use an OpenAI-compatible endpoint.

## Build and run

Install a stable Rust toolchain, then:

```bash
git clone https://github.com/KJ-AIML/memctrl.git
cd memctrl
git checkout feat/rust-runtime
cargo install --path . --locked
memctrl init
memctrl add "We use Rust and PostgreSQL" --layer project
memctrl query "database"
memctrl tree
memctrl doctor
```

Source installation is supported. This migration does not publish a crates.io package or GitHub release.

For local development:

```bash
cargo test --locked --all-targets
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release --bin memctrl --examples
```

The built CLI is `target/release/memctrl` (`memctrl.exe` on Windows). Run `memctrl --help` and command-specific `--help` for options. Responses are structured JSON; errors go to stderr with a nonzero exit status.

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
