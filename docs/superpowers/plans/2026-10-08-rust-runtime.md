# Rust migration recovery plan

## Scope

Reconstruct the expired, unpublished Rust migration from original commit f4089a8 and retained review/design contracts. Native Rust replaces runtime, CLI, MCP, SQLite data/model layer, retrieval/tree/rules, decay/reflection, LLM client, Heli ingestion, telemetry/spans, checkpoint/agent APIs, installer, doctor, examples and benchmarks. Remove all Python source and packaging from the branch. Retain SQLite v4 and original data, with explicit third-party API changes.

## Work and ownership

| Work | Files | Validation |
|---|---|---|
| Core store/models/security | model,store,schema,sanitize | migration, defaults, redaction, transactional rollback, identifier/evidence contracts |
| Retrieval/maintenance | rules,tree,retriever,decay,reflection | captured query oracle, retention, triggers, cache concurrency/temporal isolation, candidate lineage |
| Native adapters | llm,heli | local HTTP mocks, readonly corpus, typed response validation, atomic import |
| Support | telemetry,span,agent,installer,doctor | original span fields, checkpoints, bounded scopes, diagnostic exposure, scoped install |
| Entry points | main,mcp | real CLI and SDK stdio, invalid inputs before mutations, hot reload |
| Integration | Cargo/CI/docs/examples | full tests, strict Clippy/format/release builds, source-only check, independent review |
| GitHub handoff | feat/rust-runtime -> main | user explicitly authorized a PR; no merge/release/package publication |

Shared interfaces were fixed before independent modules were assigned. Shared Cargo/lib declarations are controller-owned. No worker may write another worker's files or publish. Final integration verification and independent whole-branch review cover boundaries between those modules.

## Research/design choices

Rust ownership and RAII transactions for cleanup/rollback; typed semantics with legacy NULL defaults; recursive structured redaction and credential identifier rejection; consistent retrieval transaction for cache writer safety; explicit maintenance and source-preserving candidate reflection; maintained SDK stdio; native JSON agent/checkpoint integration. All optional provider traffic is explicit, all Heli source files read-only.

Retrieval locks the SQLite writer while scoring, recording access/provenance and populating cache. This is a correctness-first tradeoff and limits concurrent write throughput. It replaces the initial expired implementation's conditional snapshot cache publication; it should not be represented as byte-for-byte recovery or equivalent benchmark results.

## Completion evidence

See docs/RUST-VERIFICATION.md for fresh results. The original suite passed520 tests with3 skipped during recovery. Separate Rust coverage and captured SQL/query fixtures are used for migration validation; the former count is not Rust test coverage.
