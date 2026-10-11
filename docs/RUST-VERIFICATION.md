# Rust migration verification

## 2.1 verification (branch `feat/memory-2.1`, 2026-10-12)

`make ci` passes locally on macOS arm64: fmt, strict all-target Clippy, 128 passing tests (66 test functions on 2.0 main, 120 now), release build and the Rust-only check. New regression contracts:
- `tests/search.rs`: multilingual matching, BM25 ranking, aliases and the cache key, explanations in provenance, index maintenance by raw writers, backfill on open.
- `tests/citations.rs`: edit, move and delete freshness; atomic invalid citations; audited refresh; MCP.
- `tests/mirror.rs`: byte-identical export → import → export, edited and new entries become candidates once, local state wins, atomic failures, guard on every write path.
- `tests/eval.rs`: retrieval floors on the built-in suite.
- `cli.rs`: session brief and hook install.

Measured with the release binary: queries take about 10 ms on a 3,000-memory store (2.0.0: about 120 ms). A 2.0-created database is indexed automatically on first open. The built-in eval results are in README "Measuring retrieval".

Limits:
- The eval suite was written by the same author as the retriever; it is a regression gate, not an independent benchmark.
- Lexical retrieval does not answer paraphrases with no shared words.
- Prompt-injection detection is heuristic. It lowers trust for review and never blocks, so false positives cost a review click and false negatives remain possible.
- Citation freshness compares the working tree, not a specific commit.

## 2.0 migration verification

Fresh local verification:52 Rust tests passed, formatting and strict all-target Clippy passed. Optimized executable and four Cargo examples also built successfully. Independent final review and GitHub CI remain in progress. Original version1.3.0 baseline:520 tests passed,3 skipped during recovery. Native coverage is a separate Rust suite with captured SQL/query results and the preserved20-task Heli corpus.

## Review regressions carried forward

- Consistent cache snapshot across concurrent writers and time-only validity changes.
- Recursive structured password/API key redaction and full/truncated private key handling.
- Credential-bearing layer, checkpoint thread and span operation identifiers rejected before storage.
- Supplied optional MCP field types and context validated before mutations.
- Original dedicated telemetry fields preserved on read/export.
- Historical NULL/empty semantic values defaulted instead of breaking database reads.
- Accepting refuted facts rejected; preference/migration and ordinary agent-message extraction retained.

## Limits

Local execution is Linux only. The CI matrix runs Linux/macOS/Windows. No paid provider API calls, automatic package publication, release signing or merge are part of this change. Optional providers are tested with loopback mock HTTP only. This is not source-compatible with former Python integration classes. LLM-assisted reflection is explicitly unavailable; reflection is source-preserving local distillation. JSON output, explicit `clear --yes`, and required `install --tool` are native CLI shifts.
