# MemCtrl native Rust development

MemCtrl 2.x is a Rust crate, CLI and MCP server with local SQLite defaults. No Python source, bridges, packaging or dependencies may be introduced.

Run `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked --all-targets`, and `cargo build --locked --release --bin memctrl --examples` before handoff (`make ci` runs all of them plus the Rust-only source check). Releases follow DISTRIBUTION.md; never create tags, releases or crates.io publications without explicit maintainer approval. Add regression contracts for meaningful behavior changes. Update migration notes for intentional interface shifts.

Sanitize all text/structured metadata before storage, model prompts or output. Reject credential-bearing identifiers rather than changing their identity. Preserve validated Git revisions only in explicitly named revision fields. Stdout is exclusively MCP protocol traffic in `serve`; CLI errors are sanitized stderr with nonzero status.

Retain SQLite v4 data and migrate older columns before indexing. Reject future schemas. Use atomic transactions/savepoints for multi-write lineage and state mutations. Candidates, refuted, expired and temporally invalid facts must not appear in ordinary retrieval. Cache identity and population must use a consistent snapshot across writers and temporal changes.

Optional provider requests are opt-in. Tests use local fixtures and mock endpoints; do not make paid provider calls. Heli source files are read-only. Native agent checkpoints are distinct from facts. Reflection preserves source evidence and proposes reviewable lessons.

Retrieval is FTS5 (`search.rs`); keep the trigger-maintained indexes additive and rebuildable, and bump `RETRIEVAL_VERSION` when scoring changes. Run `memctrl eval` (or `cargo test --test eval`) for any retrieval change; floors in tests/eval.rs must not be lowered to make a change pass. Every write path that creates memories from text must pass through `guard::apply`. Citation checks are read-only; only `refresh` re-anchors, and it is audited. The session brief must never bump access counts or create a store.
