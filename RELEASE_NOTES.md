## MemCtrl v1.3.0 — Reliability Foundation & Evidence-Linked Memory Lifecycle

### Overview

Major reliability and knowledge lifecycle release. Upgrades MemCtrl from observable memory infrastructure into a reviewable, evidence-linked agent memory system with an explicit knowledge lifecycle, immutable provenance, and measurable history reuse.

### What's New

**P0 Reliability & Storage Foundation**
- **Centralized Persistence Boundary** — All memory insertions (manual, extractor, reflection, MCP, adapters) route through `_insert_memory_tx()` with mandatory secret sanitization (`sanitize_text()`) before hitting SQLite.
- **Expiry as Eligibility** — Memory expiration now controls retrieval eligibility (`expires_at < now` filtered by default across get, list, query, tree, and reflection inputs) rather than depending solely on physical deletion.
- **Scope-Isolated Query Cache** — `QueryCacheKey` parameterizes cache identity across `query`, `layer`, `retrieval_version`, and `model_identity`, preventing cross-layer result leakage in both in-memory and SQLite cache.
- **Immutable Creation Timestamps** — `created_at` is strictly immutable. Introduced `updated_at`, `last_accessed_at`, and `access_count` in schema v3; decoupled retrieval access telemetry from evidential reinforcement.
- **Durable Decay Scheduling** — Maintenance execution state is persisted in SQLite `maintenance_state` across process restarts. Aligned floor review detection with clamp semantics (`confidence <= floor` is review-eligible).
- **Diagnostic Decoupling** — Health checks (`memctrl doctor`) distinguish source attribution from verification and report `retrieval_exposure` instead of calling query trace appearances "provenance coverage".

**Knowledge Lifecycle & Lineage (Schema v4)**
- **Orthogonal Semantic Axes** — Distinguishes what was asserted vs what was verified:
  - `claim_type`: `observation`, `assertion`, `hypothesis`, `decision`, `derived_lesson`
  - `lifecycle_state`: `candidate`, `accepted`, `superseded`, `rejected`, `archived`
  - `verification_state`: `unverified`, `supported`, `disputed`, `refuted`
  - Temporal bounds: `observed_at`, `valid_from`, `valid_until`
- **Memory Relations** — Table `memory_relations` records directed lineage (`derived_from`, `supports`, `contradicts`, `supersedes`, `refutes`). Distinguishes temporal replacement (`supersede_memory()`) from empirical refutation (`refute_memory()`).
- **External Evidence References** — Table `memory_evidence` connects memories to authoritative external records (e.g. task IDs, commit SHAs) without mutating or copying authority.
- **Automated Database Migration** — Safe, idempotent migration path from `v2` through `v3` to `v4` with legacy semantics preserved.

**Reflection as Candidate Generation**
- **Preserved Session Evidence** — Session-end reflection preserves session source records in the session layer instead of bulk-promoting them into project truth.
- **Reviewable Candidates** — Reflection output is stored as reviewable candidate knowledge (`lifecycle_state='candidate'`, `verification_state='unverified'`) linked to source memories via `derived_from` relations.

**Review Workflow (CLI & MCP)**
- **CLI Commands** — Added `candidates`, `show <id>`, `accept <id>`, `reject <id>`, `history <id>`, `dispute <id>`, and `refute <id> --reason <reason>`. Guards against accepting refuted claims or creating self-referencing cycles.
- **MCP Tools** — Added `memctrl_candidates` and `memctrl_review` tools for agent-native memory curation.

**Lifecycle-Aware Retrieval**
- **Allowlist-Based Filtering** — Default retrieval strictly requires `lifecycle_state == 'accepted'`, `verification_state != 'refuted'`, and non-expired status.
- **Annotated History Mode** — `history=True` retrieval surfaces historical facts with explicit visual annotations (`[REFUTED]`, `[SUPERSEDED]`, `[CANDIDATE]`, `[ARCHIVED]`).

**Read-Only Heli History Adapter & Evaluation**
- **Read-Only Source Adapter** — `HeliSourceAdapter` inspects completed Heli-Harness workspace tasks without modifying any files under `.heli-harness`.
- **Lesson Distiller** — `HeliDistiller` extracts reusable candidate lessons, links external evidence, and captures counterexamples across tasks with similar symptoms.
- **CLI Review Command** — `memctrl review heli --workspace <path> --completed <n> [--dry-run|--persist]`.
- **Pilot Evaluation Benchmark** — An initial read-only Heli-history evaluation across 20 completed tasks returned useful leads for all 12 benchmark queries, compared with 11/12 for the grep baseline, while correctly identifying the benchmark's refuted hypothesis. This represents pilot validation evidence on a curated historical corpus, not a general uncurated recall benchmark.

---

## MemCtrl v1.2.1 — Credibility Hardening Release

### Overview

Production-readiness patch focused on benchmark honesty, retrieval precision, CLI correctness, and SQLite durability.

### What's New

**Benchmarks & Trust**
- **Honest Capability Benchmark** — Replaced misleading precision comparisons with a capability matrix that accurately shows MemCtrl's strengths (trace explainability, redaction, layer enforcement, lifetime management)
- **Retrieval Precision Fix** — Added synonym expansion, layer boost, confidence weighting, and relative threshold filtering. Benchmark precision improved from 27% to 100% on the demo harness.

**CLI Polish**
- **`memctrl serve`** no longer accepts fake `port`/`host` args (stdio transport)
- **`pyproject.toml` description** aligned with v1.2 "Observable Memory Infrastructure" positioning

**Durability**
- **SQLite Retry Logic** — All 19 write paths now wrapped with exponential backoff (50ms / 200ms / 500ms) for concurrent CLI + MCP safety
- **Tree Batching** — LLM clustering capped at 20 memories per prompt to prevent O(N²) blow-up at scale

**Docs & Demos**
- **Migration Demo** — `examples/migration_demo.py` shows cross-tool memory portability (Claude Code → Cursor)
- **OTel Spec Submitted** — `gen_ai.memory.*` proposal published to OpenTelemetry GenAI SIG ([#200](https://github.com/open-telemetry/semantic-conventions-genai/issues/200))

### Benchmark Status

The demo harness now reports honest numbers:
- Context retention: 100%
- Retrieval precision: 100%
- Trace accuracy: 100%

These are demonstration-only metrics on a tiny keyword dataset, not validated vector-DB comparisons.

---

## MemCtrl v1.2.0 — Observable Memory Infrastructure for AI Agents

### Overview

MemCtrl is the first observable memory infrastructure for AI agents. While vector databases store chunks, MemCtrl stores context with provenance — every retrieval shows its reasoning path, every memory has a confidence score that decays over time, and every operation exports to OpenTelemetry.

### What's New in v1.2

**Observability**
- **Retrieval Provenance** — Full audit trail for every retrieval: source, confidence, match reason, trace path
- **OpenTelemetry Exporter** — First reference implementation for `gen_ai.memory.*` conventions
- **Memory Span** — Context manager for operation tracing across agent tasks

### What's New in v1.1

**Agent Runtime**
- **Confidence Decay** — Inferred facts decay over time if not reinforced; explicit facts persist forever
- **Query Result Cache** — Repeat queries return in <1ms (was 50-500ms)
- **Reflection Engine** — Auto-detect session end via git commit, time-based, or explicit `memctrl done`
- **Incremental Tree Rebuild** — Only rebuild affected branches, not the entire tree
- **Project-Local Database** — Each project gets its own `.memctrl/memories.db`
- **LangGraph Verification** — 13 tests covering `MemCtrlMemory`, `MemoryNode`, `MemCtrlSaver`

---

## MemCtrl v1.0.0 — Cognitive Memory Runtime for AI Agents

### Overview

MemCtrl is an operating system for long-lived agent memory — hierarchical, explainable, and self-managing. It replaces passive vector dumps with human-like memory layers that remember, forget, consolidate, and explain their reasoning.

### What's Included

**Core Architecture**
- **Hierarchical memory layers** — Project (forever), Session (7 days), User (90 days)
- **Tree-based retrieval** — LLM reasons over memory structure, not vector similarity
- **Reasoning traces** — Every answer shows its exact path: `root -> project -> auth -> jwt`
- **Automatic consolidation** — Session memories merge into project knowledge via trigger rules
- **Security-first** — Secrets, API keys, and PII are redacted before storage

**Integrations**
- **LangGraph** — `MemCtrlSaver` checkpoint saver + `MemoryNode` for agent workflows
- **MCP** — Stdio transport server for IDE integration
- **Claude Code / Cursor / Kimi Code / Codex** — SKILL.md registration via `memctrl install`

**CLI Commands**
- `memctrl init` — Create `.memoryrc`
- `memctrl add` — Store memory with layer, tags, confidence
- `memctrl query` — Retrieve with reasoning trace
- `memctrl heatmap` — Visualize memory distribution
- `memctrl timeline` — Chronological memory events
- `memctrl tree` — Hierarchical tree view
- `memctrl trigger-cmd` — Fire automation rules
- `memctrl audit` — Complete trigger log

**Demos & Benchmark Harness**
- `examples/coding_agent_demo.py` — Multi-session agent simulation
- `examples/killer_demo.py` — Bug prevention across sprints (the "holy sh*t" moment)
- `examples/langgraph_integration.py` — LangGraph usage patterns
- `benchmarks/retention_benchmark.py` — Local harness for retrieval experiments; not a validated vector-DB benchmark

**Visualizer**
- Interactive memory graph: [Live Demo](https://kj-aiml.github.io/memctrl/memory-viz.html)
- Landing page: [https://kj-aiml.github.io/memctrl/](https://kj-aiml.github.io/memctrl/)

### Installation

```bash
pip install memctrl
```

### Quick Start

```bash
memctrl init
memctrl add "we use FastAPI + PostgreSQL" --layer project
memctrl query "what is our stack?"
# Trace: root -> project -> tech_stack -> FastAPI + PostgreSQL
```

### Benchmark Status

The repository includes an experimental benchmark harness, but v1.2 does not make validated performance claims against vector databases. Publish benchmark numbers only after adding real vector baselines, a larger query set, variance reporting, and documented methodology.

### Links

- **Repository**: https://github.com/KJ-AIML/memctrl
- **Landing Page**: https://kj-aiml.github.io/memctrl/
- **Memory Visualizer**: https://kj-aiml.github.io/memctrl/memory-viz.html
- **Documentation**: See [README.md](https://github.com/KJ-AIML/memctrl#readme)
- **Technical Article**: [ARTICLE.md](https://github.com/KJ-AIML/memctrl/blob/master/ARTICLE.md)

### Thanks

To everyone who shaped MemCtrl's direction. This is just the beginning.

---

**Full Changelog**: https://github.com/KJ-AIML/memctrl/commits/main
