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

**Full Changelog**: https://github.com/KJ-AIML/memctrl/commits/v1.0.0
