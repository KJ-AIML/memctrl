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

## 📊 Commit

```bash
git log --oneline -1
# a2b10ff axga-hardening: atomic rebuild, retry logic, stemming, persistent cache, LLM CLI, auto-decay, secret redaction, WAL checkpoint
```

**Files changed:** 6  
**Insertions:** 1,028  
**Deletions:** 166  

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

# Check the commit
git show --stat HEAD

# Test stemming
python -c "from memctrl.retriever import _stemmed_words; print(_stemmed_words('authentication deployment'))"
# → ['auth', 'deploy']

# Test persistent cache
python -c "from memctrl.cache import QueryCache; c = QueryCache(db_path='/tmp/test.db'); print('OK')"

# Check LLM client exists
python -c "from memctrl.llm_client import create_llm_client; print('OK')"
```

---

**Status: P0 COMPLETE. Ready for P1/P2 or push to remote.**
