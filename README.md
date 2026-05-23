# 🌲 MemCtrl

> **Rule-governed memory layer for AI coding assistants.**
>
> Replace passive vector stores with hierarchical, reasoning-based memory trees — fully explainable, auditable, and yours.

Type `/memctrl` in your AI assistant and every architectural decision, tech stack choice, and personal preference is remembered with a clear reasoning trace.

Works in **Claude Code**, **Codex**, **Cursor**, **Kimi Code**, **Gemini CLI**, **Aider**, **GitHub Copilot Chat**, **VS Code Copilot Chat**, and any tool that reads `SKILL.md` or `AGENTS.md`.

---

## 🚀 One-Command Quick Start

```bash
pip install memctrl
memctrl init          # creates .memoryrc in your project
memctrl install       # registers SKILL.md with your AI assistant
```

Then open your AI assistant and type:

```
/memctrl add "we use FastAPI + PostgreSQL + Redis cache"
```

Later, ask:

```
/memctrl query "what is our stack?"
# → root → project → tech_stack → FastAPI + PostgreSQL + Redis cache
```

Every answer shows its reasoning path. No black-box similarity scores. No forgotten context.

---

## ✨ What Makes MemCtrl Different

| Feature | Vectors | MemCtrl |
|---|---|---|
| **Retrieval logic** | Cosine similarity (black box) | 🌲 Tree traversal with reasoning trace |
| **Explainability** | "Score: 0.87" | `root → project → backend → fastapi` |
| **Forget policy** | Manual cleanup | 📜 Rule-driven expiry + `never-forget` lists |
| **Audit trail** | None | 📋 Complete trigger log: what, when, why |
| **Privacy** | Cloud embeddings | 🔒 Local SQLite. Your data never leaves your machine. |
| **LLM cost** | Per-query embedding API | 💰 Zero API calls for retrieval. Tree fits in context. |

---

## 🧠 Memory Layers

MemCtrl organizes memory into **layers** with different lifetimes and purposes:

| Layer | Purpose | Default Expiry |
|---|---|---|
| 🏗️ `project` | Architecture decisions, tech stack, ADRs, "why we chose X" | **Never** |
| 📝 `session` | Current task, WIP, what was done this session | **7 days** |
| 👤 `user` | Personal preferences, working style, coding patterns | **90 days** |

Rules in `.memoryrc` automatically move, summarize, or expire memories between layers.

---

## 🛠️ Install by Platform

```bash
# Universal install
pip install memctrl

# With LLM extras (LiteLLM + OpenAI adapters)
pip install "memctrl[llm]"

# Development install
pip install "memctrl[dev]"

# Everything
pip install "memctrl[llm,dev]"
```

Register the skill with your AI assistant:

| Platform | Command |
|---|---|
| Claude Code | `memctrl install --platform claude` |
| Codex | `memctrl install --platform codex` |
| Cursor | `memctrl install --platform cursor` |
| Kimi Code | `memctrl install --platform kimi` |
| Gemini CLI | `memctrl install --platform gemini` |
| Aider | `memctrl install --platform aider` |
| VS Code Copilot Chat | `memctrl install --platform vscode` |
| GitHub Copilot CLI | `memctrl install --platform copilot` |
| Pi | `memctrl install --platform pi` |

Project-scoped install (commits into your repo):

```bash
memctrl install --project
```

---

## 📖 Command Reference

### Core Memory Commands

| Command | Description |
|---|---|
| `memctrl init` | Create `.memoryrc` in current directory |
| `memctrl add <text>` | Add a memory (default layer: `session`) |
| `memctrl add <text> --layer project` | Add a permanent project memory |
| `memctrl query <question>` | Retrieve memories with reasoning trace |
| `memctrl list` | List all memories (optionally `--layer project`) |
| `memctrl tree` | Display the memory tree (Rich-formatted) |
| `memctrl forget <id>` | Remove a specific memory |
| `memctrl clear` | Clear all memories or a specific layer |

### Automation & Audit

| Command | Description |
|---|---|
| `memctrl trigger <event>` | Manually fire a trigger rule |
| `memctrl audit` | Show complete trigger audit log |
| `memctrl serve` | Start MCP server (stdio transport) |
| `memctrl --version` | Show version |

### AI Assistant Slash Commands (inside your IDE)

```
/memctrl add "we migrated from Flask to FastAPI on 2025-03-15"
/memctrl query "why did we choose FastAPI?"
/memctrl tree
/memctrl trigger on_commit
```

---

## 🔒 Security & Privacy

MemCtrl is designed with a **security-first** mindset:

- **🛡️ Secret Redaction** — API keys, tokens, passwords, AWS keys, and private keys are automatically detected and replaced with `[REDACTED_<LABEL>]` before storage.
- **🔏 PII Redaction** — Emails, SSNs, and phone numbers are sanitized.
- **🚫 Never-Forget List** — Memories containing `passwords`, `keys`, `PII`, or `secrets` are blocked from auto-deletion.
- **📍 Local-Only Default** — All data lives in `~/.memctrl/memories.db`. No cloud. No telemetry. No analytics.

---

## ⚙️ Configuration (`.memoryrc`)

Created automatically by `memctrl init`:

```toml
[layers]
project = "architecture decisions, tech stack, ADRs, why we chose X"
session = "current task, WIP, what was done this session"
user = "preferences, working style, patterns, personal rules"

[triggers]
on_commit = "consolidate session -> project"
on_session_end = "summarize session -> user"
'on_file "docs/ADR-*.md"' = "extract -> project"
'on_file "*.md"' = "extract -> project if contains decision"

[forget]
never = ["passwords", "keys", "PII", "secrets"]
after_days = { session = 7, user = 90 }

[extract]
confidence = { explicit = 1.0, inferred = 0.7, mentioned = 0.5 }
```

Hot-reload enabled: edit `.memoryrc` and changes apply immediately.

---

## 🧩 MCP Server

MemCtrl exposes an MCP server for deep IDE integration:

```bash
memctrl serve
```

**Available tools:**
- `memctrl_query` — Ask the memory tree
- `memctrl_add` — Add a memory programmatically
- `memctrl_trigger` — Fire automation rules
- `memctrl_tree` — Get structured tree JSON
- `memctrl_audit` — Read the trigger log

Register with Kimi Code:

```bash
kimi mcp add --transport stdio memctrl -- memctrl serve
```

---

## 🏗️ How It Works

```
┌─────────────┐     ┌──────────────┐     ┌─────────────────┐
│   Source    │────▶│  Extractor   │────▶│  Security Scan  │
│  (chat/CLI) │     │ (LLM + heur.)│     │ (secrets / PII) │
└─────────────┘     └──────────────┘     └─────────────────┘
                                                  │
                                                  ▼
┌─────────────┐     ┌──────────────┐     ┌─────────────────┐
│   Reason    │◀────│   Retriever  │◀────│   Memory Tree   │
│   Trace     │     │(tree traversal│     │ (layered nodes) │
└─────────────┘     └──────────────┘     └─────────────────┘
                                                  ▲
┌─────────────┐     ┌──────────────┐              │
│   SQLite    │────▶│   Builder    │──────────────┘
│   Store     │     │(LLM cluster  │
└─────────────┘     │ + fallback)  │
                    └──────────────┘
```

1. **Extract** — LLM extracts memories from chat/CLI input with confidence scoring.
2. **Secure** — Secrets and PII are redacted. Never-forget rules are applied.
3. **Store** — Memories are saved to local SQLite with layer, tags, and expiry.
4. **Build** — A hierarchical tree is built per layer (LLM clustering + keyword fallback).
5. **Retrieve** — The LLM reasons over the tree structure (not vectors) to find the best path.
6. **Trace** — Every result includes the exact reasoning chain: `root → project → backend → fastapi`.

---

## 📦 Requirements

| Requirement | Minimum |
|---|---|
| Python | 3.10+ |
| SQLite | bundled with Python |

Optional LLM backends (for extraction only):

| Backend | Setup |
|---|---|
| OpenAI | `export OPENAI_API_KEY=sk-...` |
| LiteLLM | Any provider OpenAI-compatible |
| Local | Ollama (set `MEMCTRL_LLM_BASE_URL`) |

---

## 🤝 Contributing

```bash
git clone https://github.com/KJ-AIML/memctrl.git
cd memctrl
pip install -e ".[llm,dev]"
pytest tests/ -v
```

---

## 📄 License

MIT © 2025 MemCtrl Contributors
