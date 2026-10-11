# Rust-only migration

The baseline is version 1.3.0 at `f4089a853a77b4ab631b613dc787b67d0de4488a`. Python source, packaging, examples, benchmarks and PyPI publication are removed from this branch. Git history retains earlier releases.

## Database migration

1. Stop old MemCtrl writers and back up the database using SQLite backup, or copy only after all writers close and WAL is checkpointed. Copying just a live `.db` can omit pending WAL changes.
2. Build the native executable with `cargo build --release --locked`.
3. Test `--db /path/to/backup.db stats`, `doctor`, `list --include-history` and representative queries against a backup first.
4. Update CLI/MCP clients to use the native executable and explicit database/configuration paths.

Schema v4 memory, evidence, relations, audit, tree, provenance and span rows are retained. Older schemas receive additive migrations; future schemas are rejected. NULL/empty legacy semantic values use the historical assertion/accepted/unverified defaults. Native revision/cache and checkpoint tables are additive.

Original naive timestamps are interpreted as UTC. The original runtime used the host's local clock. If an existing database was created outside UTC, convert/interpret its naive timestamps before relying on expiry, decay and validity windows. RFC3339 offsets are supported for native inputs.

## API changes

| Former interface | Native replacement |
|---|---|
| Python memory store/dataclasses | `store::MemoryStore`, serde models and typed enums |
| Python CLI/Rich tables | compiled CLI with JSON output |
| LangGraph MemoryNode inheritance | `agent::memory_node`, JSON CLI or MCP |
| LangGraph checkpoint saver class | dedicated JSON checkpoint table and thread-keyed APIs |
| LiteLLM routing | optional OpenAI-compatible HTTP client |
| Watchdog reload | TOML reload per invocation or MCP request |
| Global monkey-patched tracing | explicit `telemetry::traced` and `span::SpanTracker` |
| Python benchmark scripts | native Cargo examples and JSON fixtures |
| PyPI package | Cargo source installation |

This is not a source-compatible Python bridge. Existing Python integrations must use the executable/MCP/JSON interface or be rewritten in Rust. The CLI uses JSON output; `clear` requires `--yes`, and `install` requires `--tool`. `done/reflect --llm` explicitly errors because reflection is local source-preserving distillation.

Previous checkpoint payloads stored as ordinary memory remain in the database; they are not automatically converted into the native checkpoint table.

Reflection deliberately preserves source facts and creates candidate lessons instead of immediately promoting generated summaries. Maintenance is explicit: decay runs only through `memctrl decay` (schedule `memctrl decay --min-hours 24` to keep the old daily behavior); it no longer runs on query cache misses or MCP startup. LLM-assisted tree summaries retain deterministic membership; this does not promise identical clustering or responses from previous third-party model routers.

## Removed and changed behavior

- **History-mode retrieval is removed.** The original `retrieve(..., history=True)` annotated results with `[REFUTED]`, `[SUPERSEDED]`, `[CANDIDATE]`, `[REJECTED]` and `[ARCHIVED]`. Use `list --include-history`, which returns raw rows with their lifecycle and verification states.
- **Removed Python-only helpers:** `expire_old_memories` (expiry is now an eligibility filter, not a deletion), `should_forget`, `get_flagged_memories` (`decay` reports memories below 0.3 instead of per-layer floor flags) and query-cache statistics.
- **Refutation is terminal.** `dispute`, `accept` or any other verification change on a refuted memory now errors. The original runtime allowed refute → dispute → accept, which put a disproven claim back into retrieval. To revise a refuted claim, add a new memory and `supersede` the old one.
- **Decay never raises confidence.** A memory already below its layer floor (session 0.3, user 0.5) is left unchanged. The original runtime lifted it to the floor.
- **Provenance names the real method.** `method` and each source's `match_reason` are `keyword_tree` for local retrieval and `llm_selection` for `query --llm`; `tree_version` records the store revision used.
- **UUIDs are identities, not PII.** Memory ids whose last group is all digits are no longer redacted as phone numbers.

## Recovery note

The initial local migration checkout expired before GitHub publication. This branch reconstructs the native runtime from the unchanged original repository and retained design/review findings. Its verification is fresh; previous local build numbers are not used as evidence for this checkout.
