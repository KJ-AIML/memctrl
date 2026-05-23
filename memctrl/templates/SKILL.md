---
name: memctrl
description: |
  MANDATORY: Before starting ANY work in this project, load context from
  memctrl. After finishing work, store what was learned. This is the project's
  cognitive memory layer — not optional.
  
  CRITICAL RULES:
  1. ALWAYS run `memctrl query "current project context"` at session start
  2. ALWAYS run `memctrl add` after architecture decisions or discoveries
  3. ALWAYS run `memctrl trigger on_session_end` before ending session
  4. NEVER say "I don't remember" — query memctrl first
---

# MemCtrl — Mandatory Memory Protocol

## ⚡ Session Start Protocol (REQUIRED)

Run this **before** any coding, planning, or discussion:

```bash
memctrl query "current project context"
memctrl query "what decisions have been made"
memctrl query "what is the tech stack"
```

If no context exists (new project), initialize:
```bash
memctrl init --force
```

## 📝 During Work — Store Automatically

After ANY of these events, run `memctrl add` **immediately**:

| Event | Example Command |
|-------|----------------|
| Architecture decision | `memctrl add "Using FastAPI + async SQLAlchemy" --layer project` |
| Bug discovered | `memctrl add "JWT validation fails on expired tokens — use jose` --layer project` |
| Tech choice | `memctrl add "Chose PostgreSQL over SQLite for concurrent writes" --layer project` |
| Current task | `memctrl add "Implementing OAuth2 login flow" --layer session` |
| User preference | `memctrl add "Prefers explicit types over generics" --layer user` |
| Refactoring | `memctrl add "Renamed UserService -> AuthService" --layer project` |

## 🏁 Session End Protocol (REQUIRED)

Before ending ANY session:

```bash
memctrl trigger on_session_end
memctrl audit
```

This consolidates session memories and cleans up expired entries.

## 🔍 When User Asks Context Questions

If user asks anything like:
- "Why did we choose X?"
- "What was the decision about Y?"
- "What were we doing last time?"
- "What's our tech stack?"

**NEVER guess.** Always run:
```bash
memctrl query "<user's question>"
```

## 🧠 Memory Layers

| Layer | Purpose | When to Use | Expiry |
|-------|---------|-------------|--------|
| **project** | Architecture, ADRs, tech stack, refactors | Permanent decisions | Never |
| **session** | Current task, WIP, what was done today | Temporary work | 7 days |
| **user** | Preferences, coding style, patterns | Personal habits | 90 days |

## 📋 Quick Commands

```bash
memctrl query "<question>"              # Retrieve with reasoning trace
memctrl add "<fact>" --layer project    # Store permanent knowledge
memctrl add "<fact>" --layer session    # Store temporary work
memctrl tree                             # View full memory hierarchy
memctrl trigger on_session_end          # Consolidate & cleanup
memctrl audit                            # Review memory log
```

## 🌐 MCP Server (if running)

If `memctrl serve` is active, use these tools directly:
- `memctrl_query(query, layer?)` — Retrieve memories
- `memctrl_add(content, layer, source?)` — Store memory
- `memctrl_trigger(event, context?)` — Fire rule trigger
- `memctrl_tree()` — Get memory hierarchy
- `memctrl_audit(limit?)` — View audit log
