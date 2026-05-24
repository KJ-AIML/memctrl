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

**Demos & Benchmarks**
- `examples/coding_agent_demo.py` — Multi-session agent simulation
- `examples/killer_demo.py` — Bug prevention across sprints (the "holy sh*t" moment)
- `examples/langgraph_integration.py` — LangGraph usage patterns
- `benchmarks/retention_benchmark.py` — Measurable retention vs naive baseline

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

### Benchmarks

| Metric | Vector RAG | MemCtrl |
|---|---|---|
| Context Retention (10-turn) | 62% | **91%** |
| Retrieval Explainability | 0% | **100%** |
| Memory Management Overhead | Manual | **Zero ops** |
| Long-Horizon Task Success | 45% | **78%** |

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
