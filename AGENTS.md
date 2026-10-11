# MemCtrl native Rust development

MemCtrl2.0 is a Rust crate, CLI and MCP server with local SQLite defaults. No Python source, bridges, packaging or dependencies may be introduced.

Run `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked --all-targets`, and `cargo build --locked --release --bin memctrl --examples` before handoff (`make ci` runs all of them plus the Rust-only source check). Releases follow DISTRIBUTION.md; never create tags, releases or crates.io publications without explicit maintainer approval. Add regression contracts for meaningful behavior changes. Update migration notes for intentional interface shifts.

Sanitize all text/structured metadata before storage, model prompts or output. Reject credential-bearing identifiers rather than changing their identity. Preserve validated Git revisions only in explicitly named revision fields. Stdout is exclusively MCP protocol traffic in `serve`; CLI errors are sanitized stderr with nonzero status.

Retain SQLite v4 data and migrate older columns before indexing. Reject future schemas. Use atomic transactions/savepoints for multi-write lineage and state mutations. Candidates, refuted, expired and temporally invalid facts must not appear in ordinary retrieval. Cache identity and population must use a consistent snapshot across writers and temporal changes.

Optional provider requests are opt-in. Tests use local fixtures and mock endpoints; do not make paid provider calls. Heli source files are read-only. Native agent checkpoints are distinct from facts. Reflection preserves source evidence and proposes reviewable lessons.
