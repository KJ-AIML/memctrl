---
name: memctrl
description: |
  Project memory for this repository: decisions and their reasons, conventions, environment
  quirks and known pitfalls, stored locally by memctrl. Use it when you need project context
  you can't work out from the code ("why did we choose X?", "how do tests run here?"), and
  whenever you learn something a future session should know.
---

# memctrl: project memory

memctrl keeps durable project knowledge in a local SQLite store. Each memory carries a layer, a claim type, a verification state and, optionally, citations to the code it describes. Results come back as JSON.

## At session start

If the Claude Code hook is installed (`memctrl install --tool claude --hook`), a brief of the highest-trust memories is already in your context under "Project memory (memctrl)". Otherwise, load it yourself:

```bash
memctrl context
```

Entries under **Check before trusting** are disputed, or their cited code changed after they were saved. Confirm them against the code before relying on them.

## Look things up

Search with the specific words a memory would contain: names, file paths, commands, error text. Generic phrases like "project context" match little.

```bash
memctrl query "integration tests postgres"
memctrl query "SQLx Diesel"
memctrl query "ไลเซนส์"            # Thai, Chinese and Japanese work too
```

Each fact comes with `why`: the matched terms, the score and, for cited memories, `freshness` (`fresh`, `stale` or `uncited`). Treat `stale` as a lead to verify, not a fact.

## Save what a future session needs

Save knowledge that isn't obvious from the code: decisions with their reasons, conventions, quirks, dead ends. Never save secrets; memctrl redacts known patterns, but the habit matters.

```bash
memctrl add "We chose SQLx over Diesel: compile-time checked queries caught 3 prod bugs" \
  --layer project --claim-type decision --cite src/db/mod.rs:12-40
memctrl add "Integration tests need Postgres: docker compose up -d db" --layer project --cite docker-compose.yml
memctrl add "Prefers small PRs with one concern each" --layer user
memctrl add "Migrating auth to sessions; login done, logout next" --layer session
```

| Layer | For | Default expiry |
| --- | --- | --- |
| `project` | Decisions, conventions, architecture, pitfalls | none |
| `user` | The person's preferences and working style | 90 days |
| `session` | Current task and work in progress | 7 days |

Claim types are `decision`, `assertion` (default), `observation` and `hypothesis`. `--cite path[:line[-line]]` (repeatable) links a memory to code, so memctrl can tell when that code changes.

A memory containing hidden Unicode or instruction-like text ("ignore previous instructions") is saved as a **candidate**, not accepted. The output says `"status": "quarantined"`, and a human reviews it with `memctrl candidates` and `memctrl accept <id>`.

## Keep memory correct

```bash
memctrl supersede <old_id> <new_id>        # the new memory replaces the old one
memctrl refute <id> -r "reason"            # wrong; refutation is permanent
memctrl dispute <id>                       # doubtful; shown under "Check before trusting"
memctrl stale                              # memories whose cited code changed
memctrl refresh <id>                       # after re-checking, re-anchor its citations
```

## Share with the team

```bash
memctrl export            # writes .memctrl/memory.md for review in pull requests
memctrl import .memctrl/memory.md
```

## Session end

```bash
memctrl trigger on_session_end    # applies configured consolidation rules
```

## MCP tools

When memctrl runs as an MCP server (`memctrl serve`), the same operations are available as tools:
- `memctrl_context`: the session brief
- `memctrl_query(query, layer?, limit?)`
- `memctrl_add(content, layer, source?, claim_type?, tags?, cite?)`
- `memctrl_stale(layer?)`
- `memctrl_candidates(layer?, limit?)` and `memctrl_review(id, action, reason?)`
- `memctrl_trigger(event, context?)`, `memctrl_tree()` and `memctrl_audit(limit?)`
