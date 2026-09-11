"""Regression tests pinning confirmed reliability defects in MemCtrl before fixing them.

Phase 0 requirements:
1. Reflection persistence sanitization (synthetic secret in reflection is sanitized)
2. Expired retrieval (expired memories ineligible for normal retrieval)
3. Cache scope (different layers/configurations do not share cache entries)
4. Creation timestamp (created_at is immutable upon access/reinforcement)
5. Decay floor (memories reaching configured floor are review-eligible)
6. Persistent decay scheduling (maintenance state persists across process boundaries)
7. Diagnostic source trust (source label != verification; retrieval trace != provenance coverage)
"""

from __future__ import annotations

import time
from datetime import datetime, timedelta

import pytest

from memctrl.cache import QueryCache
from memctrl.decay import ConfidenceDecay
from memctrl.doctor import analyze_store_health
from memctrl.retriever import MemoryRetriever, RetrievalResult
from memctrl.store import MemoryStore


@pytest.fixture
def tmp_db(tmp_path):
    db_file = tmp_path / "test_regressions.db"
    return str(db_file)


# ---------------------------------------------------------------------------
# 1. Reflection persistence sanitization
# ---------------------------------------------------------------------------


def test_reflection_persistence_sanitization(tmp_db):
    """LLM reflection returning a synthetic secret must be sanitized before SQLite persistence."""
    store = MemoryStore(tmp_db)
    # Seed session memory
    store.insert_memory(
        layer="session",
        content="Discussed cloud configuration",
        source="manual",
    )

    # Synthetic secret in reflection output
    secret_text = "API Key: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ123456"
    reflection_content = f"Consolidated summary: {secret_text}"

    # Consolidate with audit
    _, rid = store.consolidate_with_audit(
        from_layer="session",
        to_layer="project",
        reflection_content=reflection_content,
        reflection_source="reflection",
        event="session_end",
        action="consolidate",
    )

    assert rid is not None
    reflected_mem = store.get_memory(rid)
    assert reflected_mem is not None
    # Must be sanitized at persistence boundary — raw secret must NOT be stored
    assert "ghp_" not in reflected_mem.content
    assert "[REDACTED" in reflected_mem.content


# ---------------------------------------------------------------------------
# 2. Expired retrieval
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_expired_memory_excluded_from_normal_retrieval(tmp_db):
    """Expired memory (expires_at < now) must not appear in normal get/list/query retrieval."""
    store = MemoryStore(tmp_db)
    now = datetime.now()
    yesterday = now - timedelta(days=1)
    tomorrow = now + timedelta(days=1)

    exp_id = store.insert_memory(
        layer="project",
        content="Temporary credential or feature flag",
        source="manual",
        expires_at=yesterday,
    )
    valid_id = store.insert_memory(
        layer="project",
        content="Permanent architecture decision",
        source="manual",
        expires_at=tomorrow,
    )

    # Default get_memory should exclude expired records unless include_expired=True
    assert store.get_memory(exp_id, include_expired=False) is None
    assert store.get_memory(exp_id, include_expired=True) is not None

    # Default list_memories should exclude expired records
    active_mems = store.list_memories(include_expired=False)
    active_ids = [m.id for m in active_mems]
    assert exp_id not in active_ids
    assert valid_id in active_ids

    # Retriever using store's active memories should exclude expired records
    active_lookup = {m.id: m.to_dict() for m in store.list_memories()}
    tree = {
        "id": "root",
        "title": "Memory Tree",
        "layer": "root",
        "summary": "root",
        "memory_ids": list(active_lookup.keys()),
        "children": [],
    }
    retriever = MemoryRetriever()
    result = await retriever.retrieve("Temporary credential", tree=tree, memory_lookup=active_lookup)
    assert exp_id not in result.sources


# ---------------------------------------------------------------------------
# 3. Cache scope
# ---------------------------------------------------------------------------


def test_cache_scope_isolation(tmp_path):
    """Queries for different layers or retrieval scopes must not share cached results."""
    cache_db = str(tmp_path / "cache.db")
    cache = QueryCache(db_path=cache_db)

    result_project = RetrievalResult(
        facts=["project auth fact"],
        trace=["searched project tree"],
        confidence=1.0,
        sources=["m1"],
    )

    # Cache result for project layer
    cache.set("auth bug", result_project, layer="project")

    # Querying for session layer must NOT return project cached result
    session_hit = cache.get("auth bug", layer="session")
    assert session_hit is None

    # In persistent cache, restarting cache with same DB must also preserve layer scope
    cache2 = QueryCache(db_path=cache_db)
    assert cache2.get("auth bug", layer="session") is None
    project_hit = cache2.get("auth bug", layer="project")
    assert project_hit is not None
    assert project_hit.facts == ["project auth fact"]


# ---------------------------------------------------------------------------
# 4. Creation timestamp immutability
# ---------------------------------------------------------------------------


def test_created_at_is_immutable(tmp_db):
    """Access or reinforcement must never rewrite created_at."""
    store = MemoryStore(tmp_db)
    mid = store.insert_memory(
        layer="session",
        content="User likes dark mode",
        source="inferred",
        confidence=0.7,
    )
    original = store.get_memory(mid, include_expired=True)
    assert original is not None
    original_created_at = original.created_at

    # Simulate delay / reinforcement
    time.sleep(0.05)
    decay = ConfidenceDecay(store)
    decay.reinforce_memory(mid)

    after_reinforce = store.get_memory(mid, include_expired=True)
    assert after_reinforce is not None
    # created_at must remain strictly identical
    assert after_reinforce.created_at == original_created_at


# ---------------------------------------------------------------------------
# 5. Decay floor review state
# ---------------------------------------------------------------------------


def test_decay_floor_reaches_review_state(tmp_db):
    """Reaching or decaying to configured floor must make the memory review-eligible."""
    store = MemoryStore(tmp_db)
    # Session layer default floor is 0.3
    mid = store.insert_memory(
        layer="session",
        content="Session observation",
        source="inferred",
        confidence=0.5,
    )
    decay = ConfidenceDecay(store)

    # Decay heavily over 100 days so it converges to the floor (0.3)
    decay.decay_memories(days_elapsed=100)

    mem = store.get_memory(mid, include_expired=True)
    assert mem is not None
    assert mem.confidence <= 0.3

    # Flagged memories must include memories that reached the floor
    flagged = decay.get_flagged_memories()
    flagged_ids = [m.id for m in flagged]
    assert mid in flagged_ids


# ---------------------------------------------------------------------------
# 6. Persistent decay scheduling
# ---------------------------------------------------------------------------


def test_persistent_decay_scheduling(tmp_db):
    """Overdue maintenance schedule must not reset to 'now' when a new store process starts."""
    store1 = MemoryStore(tmp_db)
    store1.insert_memory("session", "inferred session fact", confidence=0.8)
    # Simulate a decay run recorded 48 hours ago in maintenance_state
    two_days_ago = datetime.now() - timedelta(hours=48)
    store1.record_maintenance("confidence_decay", two_days_ago)

    # Fresh process / fresh MemoryStore instance
    store2 = MemoryStore(tmp_db)
    # Last maintenance run should be retrieved from SQLite, not reset to now
    last_run = store2.get_last_maintenance("confidence_decay")
    assert last_run is not None
    assert (datetime.now() - last_run).total_seconds() >= 47 * 3600

    # Overdue decay should trigger
    decay = ConfidenceDecay(store2)
    ran = store2.run_decay_if_needed(decay, min_hours=24.0)
    assert ran is True


# ---------------------------------------------------------------------------
# 7. Diagnostic source trust & retrieval exposure
# ---------------------------------------------------------------------------


def test_diagnostic_source_trust_and_retrieval_exposure(tmp_db):
    """Doctor must not label sources as verified/trusted facts, and must distinguish retrieval exposure from provenance."""
    store = MemoryStore(tmp_db)
    store.insert_memory(
        layer="project",
        content="Derived reflection fact",
        source="reflection",
    )

    report = analyze_store_health(store)

    # Provenance section must describe retrieval exposure accurately
    assert "retrieval_exposure" in report or "exposure" in report.get("provenance", {})
    # Sources like reflection or mcp must not automatically be classified as inherently verified or trusted
    assert "trusted_sources" not in report or "reflection" not in report.get("trusted_sources", [])
