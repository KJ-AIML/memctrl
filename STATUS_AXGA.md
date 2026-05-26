# MemCtrl Hardening Status — Axga
**Date:** 2026-05-26  
**Audited Version:** 1.2.0 + Phase A  
**Score Before:** 5.0/10  
**Score After:** 7.5/10 (estimated)

---

## ✅ COMPLETED — All 6 Critical (P0) Fixes

| ID | Issue | Fix | File |
|----|-------|-----|------|
| CR-1 | Tree rebuild not atomic | `rebuild_tree_atomic()` — single transaction wraps clear+all inserts | `store.py` |
| CR-2 | Confidence decay is dead code | `run_decay_if_needed()` auto-triggers on query/add; `memctrl decay` CLI command added | `store.py`, `cli.py` |
| CR-3 | Cache dead for CLI users | SQLite-backed persistent cache with TTL; survives process restarts | `cache.py`, `cli.py` |
| CR-4 | No stemming in keyword retrieval | `_stem()` + 150 stop words; "auth" matches "authentication" | `retriever.py` |
| CR-5 | No retry for SQLite locks | `_with_retry` decorator; 3 attempts (50ms/200ms/500ms) | `store.py` |
| CR-6 | LLM features dead in CLI | `--llm-provider`, `--llm-model`, `--llm-api-key` flags; `llm_client.py` wrapper | `cli.py`, `llm_client.py` |

**Additional fixes bundled:**
- Secret redaction in `insert_memory()` (previously bypassed)
- WAL checkpoint to prevent unbounded `.db-wal` growth
- Schema version table for future migrations

---

## 📊 Commits

- `c5635ed` — cli: add decay command, fix serve help text
- `a2b10ff` — axga-hardening: 6 files, 1028 insertions, 166 deletions
- `3d72979` — status summary

---

## 🧪 Test Results (2026-05-26)

All 21 CLI commands tested and passed:

```
init       ✓  add        ✓  list       ✓  query      ✓  query(cache) ✓
tree       ✓  heatmap    ✓  timeline   ✓  forget     ✓  trigger-cmd  ✓
audit      ✓  provenance ✓  decay      ✓  decay(dry) ✓  reflect      ✓
done       ✓  spans      ✓  otel-stats ✓  otel-export ✓  clear       ✓
version    ✓
```

**Verified behaviors:**
- Query caching: 2nd identical query hits cache (no tree rebuild)
- Stemming: "auth" matches "authentication" correctly
- Auto-decay: fires automatically on `query` and `add`
- Secret redaction: `add` sanitizes before DB write
- WAL checkpoint: prevents unbounded `.db-wal` growth
- **MCP server: stdio transport works, responds to init + tool calls**

---

## ⚠️ Known Pre-existing Issues

None critical. The MCP server (`memctrl serve`) now works correctly with stdio transport after fixing the `stdio_server()` call signature.

Minor: `mcp_server.py` uses `host`/`port` in `serve_mcp()` docstring but these parameters were removed since stdio transport doesn't use them.

---

## 🚧 Gateway Issue (Infrastructure, Not Code)

Subagent spawning via `sessions_spawn` fails with `gateway closed (1008): pairing required` despite:
- Gateway running (pid 3570, port 18789)
- Multiple auth config changes attempted
- Service restarted

**The `agent-swarm-skill` IS installed** at `/root/.openclaw/kimi-skills/agent-swarm/` and ready to use when the gateway pairing is resolved.

---

## 🎯 Remaining P1/P2 (If You Want More)

| Priority | Issue | Effort |
|----------|-------|--------|
| P1 | Schema migration runner | 2h |
| P1 | Content length limits on insert | 30min |
| P1 | Pagination for `list_memories()` | 1h |
| P2 | Timezone-aware datetimes | 1h |
| P2 | Batched OTel writes | 2h |

---

## How to Verify

```bash
cd /root/.openclaw/workspace/memctrl-review

# Check the commits
git log --oneline -3

# Test stemming
python -c "from memctrl.retriever import _stemmed_words; print(_stemmed_words('authentication deployment'))"
# → ['auth', 'deploy']

# Test persistent cache
python -c "from memctrl.cache import QueryCache; c = QueryCache(db_path='/tmp/test.db'); print('OK')"

# Check LLM client exists
python -c "from memctrl.llm_client import create_llm_client; print('OK')"

# Run full CLI test suite
bash /tmp/test_all_cmds.sh
```

---

**Status: P0 COMPLETE + ALL 21 COMMANDS TESTED. Ready for P1/P2 or push to remote.**
