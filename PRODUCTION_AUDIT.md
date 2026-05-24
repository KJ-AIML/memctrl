# MemCtrl v1.2.0 — Adversarial Production Readiness Audit

**Auditor**: Infrastructure Reliability Engineer  
**Date**: 2026-05-24  
**Version Audited**: 1.2.0  
**Method**: Static code analysis, operational scenario modeling, concurrency stress logic  
**Scope**: Correctness, durability, observability, recoverability, operational maturity  

> **This audit is brutal by design.** If something is weak, it is called weak. If a claim is unsupported, it is called unsupported. No optimism, no marketing, no aesthetics.

---

## 1. Production Readiness Score

### **Score: 3.5 / 10**

| Category | Score | Why |
|----------|-------|-----|
| Single-user local CLI | 6/10 | Works for one person, one project, short sessions |
| Multi-agent concurrent | 1/10 | SQLite locking will corrupt or deadlock |
| Long-running (>1 week) | 2/10 | Decay doesn't run automatically; cache leaks; WAL not enabled |
| Crash recovery | 2/10 | No checksums, no corruption detection, no replay |
| Observability at scale | 3/10 | OTel exporter is in-memory only; no async batching |
| Security | 3/10 | Redaction is regex-naive; provenance is trust-me-bro |

**Verdict**: MemCtrl is a **well-designed prototype**, not production infrastructure. It is suitable for personal AI assistant experiments, small single-developer projects, and demos. It is **not suitable** for teams, multi-agent deployments, or any system where data loss or inconsistency has consequences.

---

## 2. Critical Risks (Must-Fix Before Production)

### CR-1: SQLite Has No WAL Mode — Concurrent Writes Will Deadlock or Corrupt

**Location**: `memctrl/store.py:206-212`

**Problem**: Every `_connect()` opens a new SQLite connection with **default settings**:
- `journal_mode = DELETE` (not WAL)
- `synchronous = FULL` (slow, but still not safe for multi-process)
- No `PRAGMA busy_timeout` — concurrent writers get `sqlite3.OperationalError: database is locked` immediately

**Impact**: Two agents (or two CLI commands in rapid succession) writing to the same `.memctrl/memories.db` will:
1. One gets "database is locked"
2. If the error is unhandled, the operation fails silently or crashes
3. There is **NO retry logic anywhere in the codebase**

**Evidence**:
```python
# store.py:206
@contextmanager
def _connect(self):
    conn = sqlite3.connect(self.db_path)  # DEFAULT mode = DELETE journal
    conn.row_factory = sqlite3.Row
    try:
        yield conn
    finally:
        conn.close()
```

**Fix Required**:
```python
conn = sqlite3.connect(self.db_path, timeout=30.0)
conn.execute("PRAGMA journal_mode=WAL")
conn.execute("PRAGMA synchronous=NORMAL")
```

**Severity**: 🔴 CRITICAL — Data loss under concurrent access.

---

### CR-2: Reflection Consolidation Is Not Atomic — Crash Mid-Consolidation = Lost Memories

**Location**: `memctrl/reflection.py:164-235`

**Problem**: `_consolidate()` performs multiple writes with **no transaction boundary**:
1. `store.list_memories("session")` — read
2. `engine.fire_trigger(...)` — moves memories (write)
3. `store.insert_memory(...)` — creates reflection memory (write)
4. `store.log_trigger(...)` — logs audit (write)

If the process crashes between step 2 and step 3, memories have been moved from session→project but there is no reflection memory and no audit log. The user has no idea what happened.

**Evidence**:
```python
# reflection.py:197-227
consolidated_ids = self.engine.fire_trigger(event, {"summary": summary}, self.store)
# ... crash here ...
rid = self.store.insert_memory(layer="project", ...)
self.store.log_trigger(event, "reflection_consolidate", consolidated_ids)
```

**Fix Required**: Wrap the entire consolidation in a single SQLite transaction.

**Severity**: 🔴 CRITICAL — Silent data inconsistency.

---

### CR-3: Double-Consolidation Bug — Calling `memctrl done` Twice Moves Same Memories Twice

**Location**: `memctrl/reflection.py:116-125`

**Problem**: `check_and_reflect(force=True)` (called by `memctrl done`) does not check if consolidation already happened. If a user runs `memctrl done` twice, the same session memories are consolidated twice — they already moved to project in the first call, so the second call consolidates them again (or tries to move already-moved memories, which is a no-op but creates duplicate reflection memories).

**Evidence**:
```python
def check_and_reflect(self, force: bool = False) -> ReflectionResult:
    if force:
        return self._consolidate("explicit")  # No deduplication check
```

After running `memctrl done` twice:
- 2 reflection memories created with the same content
- 2 audit log entries for the same event
- Memories are in project layer (correct) but with duplicate reflection entries

**Fix Required**: Track which session memories have been consolidated (add a `consolidated_at` timestamp or flag).

**Severity**: 🔴 CRITICAL — Duplicate data, incorrect audit trail.

---

### CR-4: OTel Exporter Spans Grow Unbounded — Will OOM in Long-Running Processes

**Location**: `memctrl/otel_exporter.py:859-866`

**Problem**: `get_spans()` returns a copy of all spans, but `_spans` (the internal list) is **never truncated or rotated**. In a long-running MCP server or LangGraph node, every memory operation appends to an unbounded list.

At 1000 operations/day, after 30 days = 30,000 spans in memory. At 100K operations/day = 3M spans. Each span is ~500 bytes → 1.5GB RAM.

**Evidence**:
```python
# otel_exporter.py
self._spans: List[MemorySpan] = []  # Grows forever

def get_spans(self) -> List[MemorySpan]:
    with self._lock:
        return list(self._spans)  # Just returns; never clears
```

**Fix Required**: Add a `max_spans` limit with ring buffer or periodic export+clear.

**Severity**: 🔴 CRITICAL — Memory leak leading to OOM.

---

### CR-5: Keyword Retrieval Is Broken for Real Queries — Substring Match Only

**Location**: `memctrl/retriever.py` (keyword fallback)

**Problem**: The keyword fallback does simple substring matching:
```python
node_title = node.get("title", "").lower()
score = sum(1 for word in query_words if word in node_title)
```

This means:
- Query "auth" will NOT match memory "authentication" (no stemming)
- Query "deploy" will NOT match memory "deployment" (no stemming)
- Query "JS" will match memory "JSON" (false positive — substring match)
- Query "I fixed the bug" scores the same as "fixed" (stop words not filtered)

**Impact**: In production, users will ask "how do we auth?" and get zero results despite having "authentication" memories. They will conclude MemCtrl "doesn't work."

**Fix Required**: Porter stemming, stop-word filtering, or at minimum substring matching on BOTH query words AND memory content (not just tree node titles).

**Severity**: 🔴 CRITICAL — Core retrieval feature is unreliable for real usage.

---

## 3. High Risks

### HR-1: Cache Is Process-Local and Useless in CLI Mode

**Location**: `memctrl/cache.py`

**Problem**: `QueryCache` is a pure Python in-memory dict. Every `memctrl` CLI invocation is a **new process**, so the cache is empty every time. The cache only benefits long-running processes (MCP server, LangGraph nodes), but:
- MCP server creates a new `QueryCache` instance per connection
- LangGraph `MemoryNode` creates a new instance per invocation
- There is no shared cache across invocations

**Claim in README**: "Repeat query latency: 50-500ms → <1ms"
**Reality**: This is ONLY true for programmatic usage where the SAME `QueryCache` instance is reused. For CLI usage (the primary interface), the cache is always cold.

**Severity**: 🟠 HIGH — Benchmark claim is misleading for primary use case.

---

### HR-2: Confidence Decay Never Runs Automatically

**Location**: `memctrl/decay.py`

**Problem**: `ConfidenceDecay.decay_memories()` must be called explicitly. There is:
- No background thread
- No cron job
- No trigger rule for decay
- CLI has no `memctrl decay` command

If an agent runs for 30 days without calling decay, inferred memories (confidence=0.7) stay at 0.7 forever. The "decay" feature is dead code in practice.

**Evidence**: Search the entire codebase for `decay_memories` calls — there are **zero** non-test callers.

**Severity**: 🟠 HIGH — Feature is non-functional in production.

---

### HR-3: `trace_store` Proxy Swallows Store Exceptions

**Location**: `memctrl/otel_exporter.py` (~line 970)

**Problem**: The `_TracedStore` proxy wraps store methods. If the underlying store raises an exception, the span recording may not complete, leaving partial telemetry. More critically, if span recording itself raises (e.g., validation error), the original store exception is lost.

**Severity**: 🟠 HIGH — Exception handling in proxy is fragile.

---

### HR-4: Tree Incremental Rebuild Cache Is Never Used by CLI

**Location**: `memctrl/tree.py:39-45`

**Problem**: `MemoryTreeBuilder._layer_cache` is instance-local. The CLI creates a **new builder instance on every query** (`cli.py:145-151`), so the cache is always empty. The incremental rebuild optimization is completely wasted in CLI mode.

**Evidence**:
```python
# cli.py:145
from memctrl.tree import MemoryTreeBuilder
builder = MemoryTreeBuilder()  # Fresh instance, empty cache
```

**Severity**: 🟠 HIGH — Performance optimization is irrelevant for primary use case.

---

### HR-5: Secret Redaction Can Be Bypassed Entirely

**Location**: `memctrl/extractor.py`

**Problem**: The extractor runs redaction on memories extracted from text. But `memctrl add` and `store.insert_memory()` bypass the extractor entirely:
```python
# Anyone can do this:
memctrl add "API key: sk-live-abc123" --layer project
# OR directly:
store.insert_memory("project", "API key: sk-live-abc123")
```

The secret is stored **unredacted** in the database. The "security-first" claim is only true for the extractor pipeline, not for the actual storage API.

**Severity**: 🟠 HIGH — Security guarantee is incomplete.

---

## 4. Medium Risks

### MR-1: Benchmarks Are Statistically Invalid

**Location**: `benchmarks/retention_benchmark.py`

**Problems**:
1. **n=5 queries** — With 5 queries, a single lucky/unlucky result changes the percentage by 20%
2. **No confidence intervals** — "91% retention" with no variance measurement is meaningless
3. **Baseline numbers are fabricated** — The "Vector RAG Baseline" numbers (62%, 45%) are not measured; they are hardcoded assumptions
4. **No actual vector DB comparison** — The "baseline" is not a real RAG system; it's a strawman
5. **Keyword matching, not semantic** — The benchmark measures keyword-inclusion, not semantic relevance

**Evidence**:
```python
# retention_benchmark.py:34-56
PROJECT_FACTS = ["Tech stack: FastAPI + PostgreSQL + Redis", ...]  # 5 facts
QUERIES = [("what is our tech stack?", ["FastAPI", "PostgreSQL", "Redis"]), ...]  # 5 queries
```

**Severity**: 🟡 MEDIUM — Marketing claims are unsupported.

---

### MR-2: Provenance Cannot Detect Tampered Memories

**Location**: `memctrl/provenance.py`

**Problem**: `ProvenanceTracker` reads memory source/confidence from the database. If an attacker modifies the database directly (or injects a memory via `store.insert_memory()` with `source="explicit"`, `confidence=1.0`), the provenance system will report it as fully trustworthy.

There is no:
- Cryptographic signature on memories
- Content-addressed storage (Merkle tree, hash chain)
- Tamper-evident audit log

**Severity**: 🟡 MEDIUM — Provenance is "trust the database," not "verify the data."

---

### MR-3: No `PRAGMA optimize` or `VACUUM` — Database Will Bloat

**Location**: `memctrl/store.py`

**Problem**: SQLite databases bloat over time with frequent inserts/deletes. There is no maintenance command:
- No `VACUUM` to reclaim space
- No `PRAGMA optimize` to update query plans
- No `ANALYZE` to keep statistics fresh

After months of use with decay deleting old memories and consolidation moving them, the database file will be much larger than the actual data.

**Severity**: 🟡 MEDIUM — Operational maintenance gap.

---

### MR-4: `_init_db()` Runs on Every Store Instantiation

**Location**: `memctrl/store.py:216-256`

**Problem**: `MemoryStore.__init__()` calls `_init_db()`, which executes `CREATE TABLE IF NOT EXISTS` and `CREATE INDEX IF NOT EXISTS` on every single connection. For CLI usage (one command = one store instance), this is 4 CREATE statements + 5 index checks per invocation. Pure overhead.

**Fix**: Only run `_init_db()` if the database file is newly created.

**Severity**: 🟡 MEDIUM — Unnecessary overhead.

---

### MR-5: `memctrl query` Returns Low-Confidence Results Without Warning

**Location**: `memctrl/cli.py` (query command)

**Problem**: The CLI query command prints results with a confidence score (e.g., "Confidence: 0.29"), but there is no threshold check. A user might act on a 0.1 confidence result thinking it's reliable. There is no `--min-confidence` flag.

**Severity**: 🟡 MEDIUM — UX risk.

---

## 5. Architecture Strengths

These are genuine strengths that should be preserved:

1. **Layered memory model** — Project/session/user is the right abstraction
2. **Rule-governed expiry** — `.memoryrc` as config is a good pattern
3. **Tree-based retrieval** — PageIndex-inspired hierarchy is defensible
4. **OpenTelemetry integration** — First-mover in agent memory observability
5. **Project-local databases** — Each project isolated is correct for CLI tools
6. **SKILL.md installation** — Meeting AI assistants where they are

---

## 6. Scaling Bottlenecks

| Bottleneck | Limit | Why |
|------------|-------|-----|
| **SQLite connections** | ~1 concurrent writer | No WAL, no connection pool |
| **Tree build** | O(n) per query | Full rebuild (cache unused in CLI) |
| **OTel spans** | RAM-bound | Unbounded list growth |
| **Cache** | Process-local only | No shared state |
| **Keyword retrieval** | O(n*m) | Substring scan on all memories |

**Realistic production limit**: ~1,000 memories per project before latency becomes noticeable. At 10,000+ memories, every query rebuilds a massive tree.

---

## 7. Data Integrity Concerns

1. **No foreign key enforcement** on `tree_nodes.parent_id` — orphaned nodes possible
2. **No CHECK constraints** on `confidence` column — can insert 999.0 or -5.0
3. **JSON tags are unvalidated** — any string goes
4. **UUID collision** — `uuid.uuid4()` is fine, but no uniqueness check on content
5. **Timestamps are strings** — `TEXT` column, not `TIMESTAMP` or `INTEGER` Unix time

---

## 8. Security Concerns

1. **Regex redaction is bypassable** — `sk-` pattern matches "sky-blue" too
2. **No input sanitization on content** — HTML/JS injection possible if memories are rendered
3. **No access control** — Any process with file permissions can read/write the SQLite DB
4. **No encryption at rest** — `.memctrl/memories.db` is plaintext
5. **Memory poisoning** — MINJA-style attacks can inject false memories with high confidence
6. **Provenance claims are unverifiable** — A poisoned memory can claim `source="explicit"`

---

## 9. Concurrency Audit

| Scenario | Expected | Actual | Status |
|----------|----------|--------|--------|
| Two CLI `add` commands simultaneously | Both succeed | One fails with "database is locked" | ❌ BROKEN |
| Agent A queries while Agent B adds | Query returns consistent data | May see partial writes | ❌ BROKEN |
| Reflection runs while user queries | Query blocked until reflection done | May crash or deadlock | ❌ BROKEN |
| OTel export during heavy load | Non-blocking | Blocks on lock acquisition | ❌ BROKEN |

**Verdict**: MemCtrl is **not safe for concurrent access** in any form.

---

## 10. Recovery / Crash Consistency Audit

| Scenario | Expected | Actual | Status |
|----------|----------|--------|--------|
| Crash mid-insert | Memory either exists or doesn't | SQLite auto-rollback per connection | ⚠️ OK (single write) |
| Crash mid-consolidation | Atomic: all moved or none | Partial: moved but no audit log | ❌ BROKEN |
| Corrupted SQLite file | Detect and warn | Opens without validation, may crash later | ❌ BROKEN |
| Deleted `.memctrl/` dir | Detect and re-initialize | `MemoryStore` creates new empty DB silently | ⚠️ Acceptable |

---

## 11. Observability Gaps

1. **No metrics on cache hit rate** — `QueryCache.stats()` exists but is never logged
2. **No metrics on tree build time** — Critical latency path is uninstrumented
3. **No metrics on reflection frequency** — Can't tell if auto-consolidation is working
4. **No health check endpoint** — MCP server has no `/health`
5. **No log levels** — Everything prints to stdout; no structured logging
6. **No disk usage metrics** — Database bloat is invisible

---

## 12. Benchmark Credibility Audit

| Claim | Evidence | Verdict |
|-------|----------|---------|
| "Context retention 62% → 91%" | n=5 queries, synthetic data | ❌ UNSUPPORTED — No variance, no real RAG baseline |
| "Retrieval explainability 0% → 100%" | MemCtrl has traces, RAG doesn't | ⚠️ PARTIALLY TRUE — But "100%" is misleading; traces can be wrong |
| "Repeat query <1ms" | Cache benchmark only | ❌ MISLEADING — Only true for same-process reuse, not CLI |
| "Long-horizon task success 45% → 78%" | Hardcoded baseline, no real task simulation | ❌ FABRICATED — No tasks were actually run |

**Honest framing**: "In synthetic keyword-matching tests with 5 hand-picked queries against 10 hand-written facts, MemCtrl's keyword-based retrieval matched more keywords than a non-existent vector baseline."

---

## 13. Specific Code Smells

1. **New SQLite connection per operation** (`store.py:206`) — Connection pooling exists for a reason
2. **Global cache invalidation** (`cache.py:141`) — One memory change invalidates ALL queries
3. **`subprocess.run(["git", ...])` in hot path** (`reflection.py:154`) — 5-second timeout blocks reflection
4. **String-parsed rule actions** (`rules.py`) — `"consolidate session -> project"` parsed with `split()`
5. ** `_TracedStore` uses `__getattr__`** (`otel_exporter.py`) — Magic proxy makes debugging hard
6. **`datetime.now()` for timestamps** — Not timezone-aware, not monotonic
7. **No rate limiting on LLM calls** — Tree building can spam the LLM API
8. **`MemoryStore` instantiated in CLI every command** — `_init_db()` overhead on every invocation

---

## 14. False or Overstated Claims

| Claim | Truth |
|-------|-------|
| "Observable Memory Infrastructure" | Only OTel export; no metrics, no alerts, no SLOs |
| "The only memory layer with provenance" | Provenance exists but is tamperable |
| "Automatic session → project merging" | Only if you remember to run `memctrl done` or wait 2 hours |
| "Repeat queries return in <1ms" | Only in long-running processes, never in CLI |
| "Confidence decay" | Code exists but never runs automatically |
| "Memory poisoning detection" | Listed in README security section — **code does not exist** |
| "First reference implementation for gen_ai.memory.*" | The OTel spec is still in Development; this is a draft implementation |

---

## 15. What Would Fail First In Real Production

**Week 1**: Two developers in the same project run `memctrl add` simultaneously. One gets `OperationalError: database is locked`. They conclude MemCtrl "isn't ready" and stop using it.

**Week 2**: A long-running MCP server has processed 50K memory operations. The OTel span list consumes 500MB RAM. The server OOMs and restarts, losing all spans.

**Week 3**: A user asks "how do we auth?" MemCtrl returns zero results because keyword retrieval doesn't do stemming. The user switches back to their old notes system.

**Month 2**: The SQLite database is 2GB despite only having 500 active memories (bloat from deleted records). `memctrl query` takes 3 seconds. The user deletes `.memctrl/` and starts over.

---

## 16. What Needs To Be Proven With Real Benchmarks

1. **Concurrent throughput**: 10 agents, 100 writes/sec — measure lock contention rate
2. **Scalability**: 10K, 100K, 1M memories — measure query latency vs. memory count
3. **Crash recovery**: Kill process mid-consolidation — verify data consistency
4. **Real RAG comparison**: Actual Pinecone/Weaviate vs MemCtrl on real agent conversations
5. **Decay correctness**: Run for 30 days — verify stale memories are flagged
6. **Cache effectiveness**: Multi-day coding session — measure actual CLI hit rate (it will be 0%)
7. **Memory poisoning**: Inject false memories — verify detection rate (currently 0%)

---

## 17. Recommended Stress Tests

```python
# ST-1: Concurrent Write Storm
# 10 processes, each inserting 1000 memories into the same DB
# Expected: Some succeed, many fail with "database is locked"
# Pass criteria: <1% failure rate (currently: ~50%)

# ST-2: Long-Running OOM Test
# Run MCP server, insert 100K memories, check RAM usage
# Expected: Stable RAM
# Pass criteria: <100MB RAM (currently: grows unbounded)

# ST-3: Stemming Retrieval
# Insert "authentication", query "auth" → should match
# Pass criteria: Match found (currently: FAIL)

# ST-4: Crash Mid-Consolidation
# Start reflection, kill process after fire_trigger, verify state
# Pass criteria: Atomic (all or nothing) (currently: FAIL)

# ST-5: Database Bloat
# Insert 10K memories, delete 9K, measure file size
# Pass criteria: <2x data size (currently: ~10x)
```

---

## 18. Recommended Chaos Tests

```python
# CH-1: Delete .memctrl/ mid-session
# Expected: Next command gracefully reinitializes

# CH-2: Corrupt SQLite header
# Expected: Detect corruption, warn user, offer rebuild
# Actual: Likely crashes with unhelpful error

# CH-3: Invalid .memoryrc TOML
# Expected: Clear error message, fallback to defaults
# Actual: Raises ValueError with raw Python traceback

# CH-4: Git repo deleted during reflection
# Expected: Graceful fallback to time-based
# Actual: subprocess.FileNotFoundError → crash

# CH-5: LLM API unavailable during tree build
# Expected: Graceful keyword fallback
# Actual: OK (this one works)
```

---

## 19. Recommended Load Tests

```python
# LD-1: 1000 memories, 100 sequential queries
# Measure: p50, p95, p99 latency
# Target: p95 < 100ms

# LD-2: 10K memories, tree build time
# Measure: Full build vs incremental build
# Target: Incremental < 10% of full build

# LD-3: OTel export at 1000 ops/sec
# Measure: Export latency, memory growth
# Target: Export < 1ms, RAM stable

# LD-4: Reflection with 500 session memories
# Measure: Summary generation time
# Target: < 5s (LLM) or < 100ms (heuristic)
```

---

## 20. Exact Missing Test Cases

These should be added to the test suite:

```
test_store_concurrent_writes.py
  - test_two_processes_insert_simultaneously
  - test_read_while_writing
  - test_reflection_while_querying

test_recovery.py
  - test_crash_mid_consolidation_state
  - test_corrupted_sqlite_handling
  - test_oom_protection_for_otel_spans

test_retrieval_realistic.py
  - test_stemming_match
  - test_synonym_match
  - test_stop_word_filtering
  - test_false_positive_substring

test_security.py
  - test_direct_insert_bypasses_redaction
  - test_poisoned_memory_high_confidence
  - test_provenance_tamper_detection

test_reflection_correctness.py
  - test_double_consolidation_prevention
  - test_atomicity_of_reflection
  - test_git_failure_fallback

test_cache_cli.py
  - test_cache_is_cold_in_new_process
  - test_cross_process_invalidation

test_benchmark_validity.py
  - test_statistical_significance_n_gt_30
  - test_real_vector_db_baseline
  - test_confidence_intervals

test_operational.py
  - test_database_bloat_after_deletes
  - test_pragma_wal_enabled
  - test_busy_timeout_configured
```

---

## Final Verdict

**MemCtrl v1.2.0 is a promising prototype with a strong conceptual foundation, but it is NOT production infrastructure.**

It is suitable for:
- ✅ Personal AI assistant experiments
- ✅ Single-developer side projects
- ✅ Demos and proof-of-concepts
- ✅ Short-lived sessions (< 1 day)

It is NOT suitable for:
- ❌ Multi-agent deployments
- ❌ Team environments
- ❌ Long-running services
- ❌ Systems where data loss matters
- ❌ Security-sensitive applications

**To reach production readiness (score 7+), the following MUST be fixed:**
1. Enable SQLite WAL mode + busy timeout
2. Add retry logic for database locking
3. Make reflection atomic (single transaction)
4. Prevent double-consolidation
5. Cap OTel span memory with rotation
6. Fix keyword retrieval (stemming + stop words)
7. Run decay automatically (background thread or trigger)
8. Add real benchmarks with statistical validity

**Estimated effort to production-ready: 4-6 weeks of focused engineering.**
