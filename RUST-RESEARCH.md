# Rust migration research

| Finding in original implementation | Native decision | Benefit and limit |
|---|---|---|
| Interpreter/import startup | compiled executable | measurable startup opportunity, not proof of faster queries |
| Repeated connections | connection owned by MemoryStore | less setup; SQLite still governs write concurrency |
| Multi-write mutations | RAII transactions/savepoints | rollback on errors and scoped resource cleanup |
| Runtime semantic strings | typed enums with legacy defaults | validation of new writes while retaining readable old databases |
| Partial redaction/structured metadata | once-compiled patterns plus recursive sensitive-field handling | preserves JSON syntax and removes ordinary nested credentials |
| Retrieval/cache writer races | consistent transaction across retrieval/access/provenance/cache | snapshot consistency; longer writer lock during scoring |
| Cache time-only changes | eligible-ID identity | new valid/expired facts alter the cache key without a SQL mutation |
| Reflection can lose evidence | source-preserving candidate lessons | explicit review and lineage |
| MCP framing complexity | official Rust SDK | maintained negotiation/stdio framing; application still validates supplied fields |
| Python checkpoint identity issue | native thread-keyed upsert table | atomic state updates separate from retrievable facts |

Rust's ownership system checks resource ownership at compile time. SQLite WAL permits readers alongside a writer but still has a single writer. Retrieval records access/provenance, so a consistent immediate transaction intentionally serializes it with writers. Database size, scanning, lock contention and optional provider latency can dominate throughput; native compilation alone does not remove these costs.

## Primary sources

- [Rust ownership](https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html)
- [rusqlite transactions](https://docs.rs/rusqlite/latest/rusqlite/struct.Transaction.html)
- [SQLite WAL concurrency](https://sqlite.org/wal.html)
- [Official Rust MCP SDK](https://rust.sdk.modelcontextprotocol.io/)

## Measurements

Measurements are recorded by the native `startup_benchmark` and `retention_benchmark` Cargo examples. Process startup uses the same `--version` action, launch count and working directory for each executable. It does not establish general retrieval/LLM speedups. Local Linux verification is not proof of macOS/Windows behavior; CI runs those platforms.

The original unchanged test suite passed 520 tests with 3 skipped during recovery. The Rust suite is separately maintained and includes captured SQL/query fixtures. The baseline count is not a claim that every original test was translated.

## Fresh local measurements

Linux x86_64, Rust1.99.0, optimized native executable,20 launches each:

| `--version` startup | Original | Rust |
|---|---:|---:|
| Median |113.01ms|5.52ms|
| Range |106.06–136.54ms|4.84–8.30ms|
| Peak RSS |unmeasured|unmeasured|

The measured startup ratio is20.5× for this action on this host, not a general workload speedup. Values vary with host and load. The native workload inserted1,000 records in22.31ms and completed30 queries in297.00ms (101.01 queries/s). No equivalent Python throughput run was made. Twenty Heli tasks yielded24 persisted review proposals; none entered ordinary retrieval before acceptance. JSON measurements are in docs/benchmarks.
