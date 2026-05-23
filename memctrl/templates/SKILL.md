---
name: memctrl
description: >
  Rule-governed memory layer for this project. Call memctrl to store,
  retrieve, or update memory about this project, session, or user preferences.
  Use this when:
  - Starting a new session (load context)
  - Making an architecture decision (store to project layer)
  - Finishing work (trigger on_session_end)
  - Asking "what did we decide about X" (query memory tree)
---

# MemCtrl Memory System

## When to Use

- **Session start**: Run `memctrl query "current project context"` for instant context
- **Architecture decisions**: Run `memctrl add --layer project "decided to use X because Y"`
- **Session end**: Run `memctrl trigger on_session_end` to consolidate memories
- **Any context question**: Run `memctrl query "<your question>"`

## Memory Layers

| Layer | Purpose | Default Expiry |
|-------|---------|---------------|
| project | Architecture decisions, tech stack, ADRs | Never |
| session | Current task, WIP, what was done this session | 7 days |
| user | Personal preferences, working style, patterns | 90 days |

## Key Commands

```bash
# Query what you need
memctrl query "what is our tech stack?"
memctrl query "why did we choose PostgreSQL?"

# Store decisions
memctrl add "decided to use Firecracker for sandbox isolation" --layer project
memctrl add "currently implementing auth flow" --layer session
memctrl add "prefers async Python, minimal abstractions" --layer user

# Manage
memctrl tree                    # view full memory tree
memctrl trigger on_session_end  # consolidate session memories
memctrl audit                   # review what was remembered/forgotten
```

## How Retrieval Works

MemCtrl uses tree-based reasoning (like PageIndex) instead of vector similarity:
- Memories are organized in a semantic tree: project/tech_stack/database
- When you query, the system reasons about which branches to explore
- Results include a trace: root → project → tech_stack → database
- No embeddings needed — pure structured reasoning

## MCP Server

If the MCP server is running (`memctrl serve`), these tools are available:
- `memctrl_query(query, layer?)` → Retrieve memories with trace
- `memctrl_add(content, layer, source?)` → Store a memory
- `memctrl_trigger(event, context?)` → Fire a trigger
- `memctrl_tree()` → Get full memory tree
- `memctrl_audit(limit?)` → View audit log
