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
| LiteLLM routing | native presets for OpenAI, xAI (Responses API), Anthropic (Messages API) and OpenAI-compatible servers; see README "Optional LLM providers" |
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
- **Provenance names the real method.** `method` and each source's `match_reason` are `keyword_tree` (2.1: `lexical_bm25`) for local retrieval and `llm_selection` for `query --llm`; `tree_version` records the store revision used.
- **UUIDs are identities, not PII.** Memory ids whose last group is all digits are no longer redacted as phone numbers.
- **Numbers inside JSON content are redacted.** When memory content (or any text) is a JSON document, numeric values that match a PII pattern (e.g. `{"phone": 5551234567}`) are replaced with a redaction string. Numeric fields in command output and structured metadata (timestamps, counts) are left as numbers.
- **Secret values are redacted whole.** `password=p,ssw0rd;x` and `Bearer abc,def` redact the entire value, as the original runtime did; the native runtime previously stopped at `,`, `;` or `}`.
- **LLM configuration is explicit and argv-free.** `--llm-provider` accepts `openai`, `xai`, `anthropic` and `compatible`. Use `--llm-api-key-env NAME` or `MEMCTRL_LLM_API_KEY`; `--llm-api-key` still works but is hidden and prints a warning, because argv is visible to other processes. `OPENAI_BASE_URL` and `OPENAI_MODEL` apply only to OpenAI-shaped providers.
- **Writes take the lock up front.** Top-level mutations begin `IMMEDIATE`, so concurrent writers wait (up to the 30s busy timeout) instead of failing with `SQLITE_BUSY_SNAPSHOT`. `add --llm` and `clear --layer` are all-or-nothing.

Legacy databases from the original runtime are covered by migration tests built from real stores: `tests/fixtures/python-v{2,3,4}-full.sql` were generated by running each commit's own `memctrl/store.py` (v2 at `76ee03a`, v3 at `47adfe7`, v4 at `519e95d`).

## 2.0 → 2.1

Opening a 2.0 database with 2.1 adds two full-text indexes and a `memory_citations` table, and indexes existing memories once. Nothing is rewritten and `schema_version` stays 4. A 2.0 binary can still open the database and its writes keep the indexes current, because maintenance happens in SQLite triggers.

- **Retrieval ranking changed.** `memctrl query`, `memctrl_query` and the agent node rank with FTS5 BM25 plus trigrams instead of the 1.x keyword scorer. Generic queries that used to return every memory (`"current project context"`) now return nothing; use `memctrl context` for session-start context. `confidence` is now the mean of memory confidence × query coverage. The 1.x scorer remains available as `retriever::keyword_retrieve`, and `memctrl eval --retriever legacy` compares the two.
- **Provenance method renamed.** Local retrieval records `lexical_bm25` (2.0 recorded `keyword_tree`). Each provenance source also carries `why`.
- **Results carry explanations.** Query output gains `why` (one entry per fact, plus `freshness` for cited memories). Library users who construct `RetrievalResult` literally need `..Default::default()`.
- **Installer paths fixed.** Claude Code skills are written to `.claude/skills/memctrl/` and Codex skills to `.agents/skills/memctrl/`. 2.0 used `.claude/agent/skills/` and `.codex/skills/`, which those tools never read. Re-run `memctrl install`; it reports a leftover legacy file as `legacy_skill`, which you can delete.
- **Guarded writes.** Memories with hidden Unicode or instruction-like text are stored as candidates (`status: quarantined`, tags `guard:*`) instead of being accepted. Set `[guard] mode = "off"` to restore 2.0 behavior.
- **New commands and tools:** `context`, `cite`, `stale`, `refresh`, `export`, `import`, `eval`, `install --hook`, and the MCP tools `memctrl_context` and `memctrl_stale`. `memctrl_add` accepts `claim_type`, `tags` and `cite`.
- **Writers need FTS5.** The search indexes are kept in sync by triggers, so any program that writes to `memories` needs a SQLite build with FTS5. memctrl 2.x binaries bundle it, and so do most language runtimes. Tools without it, such as macOS's `/usr/bin/sqlite3`, can still read the database, but their writes fail with `no such module: fts5`; nothing is corrupted. To write with such a tool, drop the three `memory_fts_*` triggers first. memctrl notices the missing triggers (or index row counts that no longer match) on its next start, recreates the triggers and re-indexes.
- **`memctrl init`** also writes `.memctrl/.gitignore`, so the database stays out of git when `.memctrl/memory.md` is committed. `memctrl maintenance` rebuilds the search indexes.

## Recovery note

The initial local migration checkout expired before GitHub publication. This branch reconstructs the native runtime from the unchanged original repository and retained design/review findings. Its verification is fresh; previous local build numbers are not used as evidence for this checkout.
