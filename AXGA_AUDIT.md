# MemCtrl v1.2.0 — Independent Adversarial Production Audit
**Auditor:** Axga (adversarial QA)  
**Date:** 2026-05-26  
**Version Audited:** 1.2.0 + Phase A hardening  
**Method:** Static code analysis, operational scenario modeling, concurrency stress logic, adversarial claim verification  
**Scope:** Correctness, durability, observability, recoverability, security, operational maturity

> **This audit is brutal by design.** If something is weak, it is called weak. If a claim is unsupported, it is called unsupported. No optimism, no marketing, no aesthetics. I have read the existing PRODUCTION_AUDIT.md and PRODUCTION_AUDIT_v2.md — this is my independent assessment with additional findings they missed.

---

## 1. Production Readiness Score

### **Score: 5.0 / 10**

| Category | Score | Why |
|----------|-------|-----|
| Single-user local CLI | 6/10 | WAL + busy timeout helps; atomic reflection is solid; but keyword retrieval is broken |
| Multi-agent concurrent | 2/10 | No retry logic; no connection pooling; tree rebuild blocks all queries |
| Long-running (>1 week) | 3/10 | WAL growth unbounded; decay never runs; OTel span writes are unbatched |
| Crash recovery | 4/10 | Atomic reflection prevents half-state; but tree rebuild is NOT atomic |
| Observability at scale | 4/10 | OTel spans persist to SQLite but no async export; no real OTLP HTTP endpoint |
| Security | 4/10 | LLM prompts sanitized; but direct insert bypasses redaction; regex-only detection |
| Retrieval correctness | 3/10 | Keyword fallback is the ONLY path in CLI; no stemming; stop words not filtered |

**Verdict**: MemCtrl is a **promising prototype with good architectural bones, but it is NOT production infrastructure.** It is suitable for personal experiments, demos, and small single-developer projects. It is **not suitable** for teams, multi-agent deployments, or any system where retrieval accuracy or data durability matters.

---

## 2. Critical Risks (Must-Fix Before Production)

### CR-1: Tree Rebuild Is NOT Atomic — Crash Mid-Rebuild = Corrupted Tree

**Location:** `memctrl/store.py:clear_tree_nodes()` + `insert_tree_node()` called sequentially from `tree.py`

**Problem:** `build_tree()` calls:
1. `store.clear_tree_nodes()` — DELETE all tree_nodes
2. Multiple `store.insert_tree_node()` — INSERT new nodes

Each is a **separate transaction**. If the process crashes between step 1 and step 2, the tree is completely empty but memories still exist. All queries return zero results until the next successful rebuild.

**Evidence:**
```python
# tree.py (build_tree calls these sequentially, each in its own transaction)
store.clear_tree_nodes()      # Transaction 1: DELETE
store.insert_tree_node(...)   # Transaction 2: INSERT
store.insert_tree_node(...)   # Transaction 3: INSERT
# ... no wrapper transaction
```

**Fix Required:** Wrap `clear_tree_nodes()` + all `insert_tree_node()` calls in a single SQLite transaction.

**Severity:** 🔴 CRITICAL — Silent total retrieval failure after crash.

---

### CR-2: Confidence Decay Is Dead Code — The Entire Decay System Is Theater

**Location:** `memctrl/decay.py` — exists but **never called automatically**

**Problem:** `ConfidenceDecay.decay_memories()` must be called explicitly. Search the entire codebase for non-test callers:
- CLI: no `memctrl decay` command
- Store: no auto-decay on query/add
- Reflection: no decay before consolidation
- Background thread: none exists

After 30 days of use, inferred memories (confidence=0.7) are still 0.7. The "confidence decay" feature advertised in the README is **pure theater**.

**Evidence:**
```bash
$ grep -r "decay_memories" memctrl/ tests/
memctrl/decay.py:def decay_memories(self) -> List[str]:    # Definition only
memctrl/decay.py:    return decayed                    # Definition only
tests/test_decay.py:def test_decay_reduces_confidence():  # Test only
```

**Fix Required:** Add `memctrl decay` CLI command; auto-trigger decay in `query()` and `add()` after N hours since last decay; or spawn a background thread in long-running processes.

**Severity:** 🔴 CRITICAL — Advertised feature does not function.

---

### CR-3: Cache Is Completely Dead for CLI Users — The "<1ms Repeat Query" Claim Is False

**Location:** `memctrl/cache.py` + `memctrl/cli.py`

**Problem:** `QueryCache` is a pure Python in-memory dict. Every CLI invocation (`memctrl query ...`) is a **new OS process**, which means:
- Fresh Python interpreter
- Fresh module imports
- Fresh `QueryCache()` instance
- Cache is always empty

The README claims: "Repeat query latency: 50-500ms → <1ms"  
**Reality for CLI users:** 0% cache hit rate. Always cold.

**Evidence:**
```python
# cli.py — new builder AND new cache every invocation
builder = MemoryTreeBuilder()   # Fresh instance
_query_cache = QueryCache()     # Fresh instance (module-level, but process dies)
# Process exits → cache destroyed
```

**Fix Required:** Persist cache to SQLite or JSON file with TTL invalidation; OR remove the claim from README.

**Severity:** 🔴 CRITICAL — Marketing claim is demonstrably false for primary use case.

---

### CR-4: Keyword Retrieval Has No Stemming — "auth" Won't Match "authentication"

**Location:** `memctrl/retriever.py:_keyword_retrieve_with_sources()`

**Problem:** Simple substring matching:
```python
score = sum(1 for word in query_words if w in node_title)
```

- Query "auth" → does NOT match "authentication" (no stemming)
- Query "deploy" → does NOT match "deployment" (no stemming)
- Query "fix" → matches "fixture" (false positive)
- Query "I fixed the bug" scores same as "fixed" (stop words not filtered)

**Impact:** In production, users ask natural language questions and get zero results. They conclude MemCtrl "doesn't work" and abandon it.

**Fix Required:** Integrate `nltk.PorterStemmer` or `snowballstemmer` for both query words AND memory content. Filter stop words.

**Severity:** 🔴 CRITICAL — Core retrieval is unreliable for real usage.

---

### CR-5: No Retry Logic for SQLite Locking — WAL Is Not a Panacea

**Location:** `memctrl/store.py:_connect()`

**Problem:** WAL mode + busy_timeout=30s helps, but there is **NO retry loop**. If a concurrent writer holds the lock (e.g., reflection consolidation on 500 session memories), the second writer gets `OperationalError: database is locked` after 30s and **fails permanently**.

**Impact:** Two `memctrl add` commands in rapid succession → one crashes. In CI or multi-agent setups, this causes non-deterministic failures.

**Fix Required:** Add exponential backoff retry (3 attempts: 50ms / 200ms / 500ms) around every write operation.

**Severity:** 🔴 CRITICAL

---

### CR-6: LLM Features Are Dead in CLI — "LLM-Powered Retrieval" Is False for CLI Users

**Location:** `memctrl/cli.py`

**Problem:** No `llm_client` is ever passed to `MemoryTreeBuilder`, `MemoryRetriever`, or `ReflectionEngine` in CLI commands. The `--tool` flag only affects installer path resolution.

CLI users get:
- Keyword fallback retrieval 100% of the time
- Heuristic reflection summaries 100% of the time
- No LLM tree clustering ever

The README's "PageIndex-style LLM retrieval" claim is **only true for Python API users**.

**Fix Required:** Add `--llm-provider` / `--llm-model` CLI flags; integrate with LiteLLM; or remove the claim from README.

**Severity:** 🔴 CRITICAL — Core differentiation is unavailable to primary users.

---

## 3. High Risks

### HR-1: Tree Rebuild Is O(N²) LLM Prompt Size — Will Fail at Scale

**Location:** `memctrl/tree.py:_cluster_with_llm()`

**Problem:** `_cluster_with_llm()` sends **ALL memories of a layer** to the LLM in a single prompt. At 10K memories × 200 chars = 2MB prompt. At 100K memories = 20MB prompt.

**Impact:**
- API rejects oversized prompts
- Prohibitively expensive
- Blocking: query commands wait for LLM response
- No async/background rebuild

**Evidence:**
```python
mem_lines = "\n".join(
    f"  [{i}] id={m['id']} | {sanitize_text(m['content'])[:200]}"
    for i, m in enumerate(memories)  # ALL memories, no pagination
)
```

**Severity:** 🟠 HIGH

---

### HR-2: Secret Redaction Is Bypassable — Direct Insert Stores Secrets in Plaintext

**Location:** `memctrl/store.py:insert_memory()` + `memctrl/cli.py:add()`

**Problem:** `sanitize_text()` only runs in the extractor and LLM prompts. But `memctrl add` and `store.insert_memory()` **bypass redaction entirely**:

```bash
$ memctrl add "API key: sk-live-abc123"
# Stored UNREDACTED in SQLite
```

The README claims "API keys, tokens, passwords are automatically detected and replaced with [REDACTED]" — this is **only true for the extractor pipeline**, not for the actual storage API.

**Severity:** 🟠 HIGH

---

### HR-3: WAL File Grows Unbounded — No Checkpointing

**Location:** `memctrl/store.py:_connect()`

**Problem:** WAL mode creates `.db-wal` file that grows until auto-checkpoint. With high write volume and no explicit `PRAGMA wal_checkpoint`, the WAL file grows to multiple GBs.

**Impact:** Long-running MCP server fills disk or experiences read degradation.

**Severity:** 🟠 HIGH

---

### HR-4: No Schema Migration System — Upgrades Break Existing DBs

**Location:** `memctrl/store.py:_init_db()`

**Problem:** `_init_db()` uses `CREATE TABLE IF NOT EXISTS`. If schema changes (e.g., adding `provenance` table in v1.2.0), existing v1.1.0 databases will NOT get new tables.

**Impact:** Users upgrading from v1.1.0 → v1.2.0 get `no such table: provenance` errors.

**Fix Required:** Add `schema_version` table + migration runner.

**Severity:** 🟠 HIGH

---

### HR-5: OTel Span Writes Are Synchronous and Unbatched

**Location:** `memctrl/otel_exporter.py:_persist_span()`

**Problem:** Every span triggers an individual INSERT + DELETE (pruning) in a fresh SQLite connection. At >50 ops/sec, this becomes a bottleneck and can cause "database is locked" errors.

**Severity:** 🟠 HIGH

---

## 4. Medium Risks

### MR-1: Benchmarks Are Statistically Meaningless

**Location:** `benchmarks/retention_benchmark.py`

**Problems:**
- n=5 hardcoded queries
- No confidence intervals
- "Vector RAG Baseline" numbers are **fabricated** (hardcoded, not measured)
- No actual vector DB was tested
- Measures keyword-inclusion, not semantic relevance

**Honest framing:** "In synthetic keyword-matching tests with 5 hand-picked queries against 10 hand-written facts, MemCtrl matched more keywords than a non-existent baseline."

**Severity:** 🟡 MEDIUM

---

### MR-2: `list_memories()` Loads ALL Content Into Memory

**Location:** `memctrl/store.py:list_memories()`

**Problem:** `SELECT * FROM memories` returns full content for every memory. At 10K memories × 1KB each = 10MB loaded into RAM per invocation. No pagination, no content truncation.

**Severity:** 🟡 MEDIUM

---

### MR-3: `_init_db()` Runs on Every Store Instantiation

**Location:** `memctrl/store.py:__init__()`

**Problem:** 4 CREATE TABLE + 10 CREATE INDEX statements execute on every CLI command. Pure overhead.

**Severity:** 🟡 MEDIUM

---

### MR-4: `datetime.now()` Is Not Timezone-Aware

**Location:** Throughout codebase

**Problem:** `datetime.now()` returns naive datetimes. Cross-timezone deployments will have incorrect expiry and decay calculations.

**Severity:** 🟡 MEDIUM

---

### MR-5: No Input Length Limits on Memory Content

**Location:** `memctrl/store.py:insert_memory()`

**Problem:** `insert_memory()` accepts arbitrary-length content. A 100MB memory will be stored, indexed, and potentially sent to LLM prompts.

**Severity:** 🟡 MEDIUM

---

## 5. Architecture Strengths (Preserve These)

1. **Layered memory model** — project/session/user is conceptually sound
2. **Rule-governed transitions** via `.memoryrc` — declarative policy is powerful
3. **Atomic reflection consolidation** — Phase A fixed the scariest data integrity risk
4. **Project-local DB isolation** — correct default for CLI tools
5. **OpenTelemetry positioning** — first-mover advantage in agent memory observability
6. **WAL mode + busy_timeout** — shows responsiveness to operational feedback

---

## 6. What Would Fail First In Real Production

**Week 1:** User asks "how do we auth?" → zero results because "auth" doesn't match "authentication" via substring. User concludes MemCtrl is broken.

**Week 2:** Two developers run `memctrl add` simultaneously → one gets `database is locked`. They add a `sleep 1` workaround and lose trust.

**Week 3:** SQLite `.db-wal` file grows to 5GB on the MCP server. Disk alerts fire. Ops team deletes the WAL → **database corruption**.

**Month 2:** `memctrl query` takes 3 seconds because `list_memories()` loads 50MB of content. User runs `rm -rf .memctrl/` and starts over.

---

## 7. Exact Missing Test Cases

```
test_store.py
  - test_tree_rebuild_is_atomic
  - test_concurrent_insert_with_retry
  - test_wal_checkpoint_prevents_bloat
  - test_schema_migration_v1_1_to_v1_2

test_retriever.py
  - test_stemming_auth_matches_authentication
  - test_stop_words_filtered
  - test_false_positive_substring
  - test_query_latency_under_10k_memories

test_cli.py
  - test_cache_is_cold_across_processes
  - test_llm_client_never_wired_in_cli
  - test_decay_command_exists

test_security.py
  - test_direct_insert_bypasses_redaction
  - test_poisoned_memory_high_confidence
  - test_100mb_memory_does_not_crash

test_tree.py
  - test_build_tree_100k_memories_prompt_size
  - test_incremental_build_after_delete_invalidates

test_reflection.py
  - test_concurrent_done_does_not_duplicate
  - test_crash_during_tree_rebuild_recoverable
```

---

## 8. Fix Priority Matrix

| Priority | Issue | Effort | Impact |
|----------|-------|--------|--------|
| P0 | Tree rebuild atomicity | 2h | Prevents total retrieval failure |
| P0 | Keyword stemming | 4h | Core retrieval correctness |
| P0 | Auto-trigger decay | 4h | Makes advertised feature work |
| P0 | Retry logic for DB locks | 2h | Prevents concurrent write failures |
| P0 | Cache persistence OR remove claim | 4h | Fixes false marketing |
| P1 | LLM CLI integration | 8h | Enables core differentiation |
| P1 | WAL checkpointing | 2h | Prevents disk exhaustion |
| P1 | Schema migrations | 4h | Enables safe upgrades |
| P1 | Secret redaction in insert | 2h | Completes security guarantee |
| P2 | Batched OTel writes | 4h | Scales observability |
| P2 | Content length limits | 1h | Prevents abuse |
| P2 | Pagination for list_memories | 2h | Scales to large datasets |
| P2 | Timezone-aware datetimes | 2h | Cross-timezone correctness |

---

## Final Verdict

**MemCtrl v1.2.0 is a well-architected prototype with significant production gaps.**

The Phase A hardening (WAL, atomic reflection, LLM prompt sanitization) shows the team can respond to operational feedback. But the remaining issues are **not edge cases** — they are central to the product's claims:

- Retrieval doesn't work for natural language (no stemming)
- Cache doesn't work for CLI users (primary use case)
- Decay doesn't work at all (dead code)
- Tree rebuild can corrupt the entire index (not atomic)
- LLM features are inaccessible to CLI users

**Estimated effort to small-team production (score 7+): 2-3 weeks of focused engineering.**  
**Estimated effort to enterprise production (score 9+): 6-8 weeks.**

MemCtrl should be marketed as what it is: **a promising memory layer prototype for solo developers and experiments.** Not as "Observable Memory Infrastructure" or "The only memory layer with provenance." Those claims create expectations that the current codebase cannot meet.

The bones are good. The execution needs 2-3 more hardening passes.
