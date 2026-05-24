"""Tests for MemCtrl LangGraph integration.

These tests verify that the LangGraph integration classes work correctly:
- MemCtrlMemory: high-level memory manager
- MemoryNode: LangGraph node wrapper
- MemCtrlSaver: checkpoint saver (langgraph required)

If langgraph is not installed, tests that depend on it are skipped.
"""

from __future__ import annotations

import os
import tempfile

import pytest

from memctrl.integrations.langgraph import (
    LANGGRAPH_AVAILABLE,
    MemCtrlMemory,
    MemoryNode,
)

# ---------------------------------------------------------------------------
# MemCtrlMemory tests
# ---------------------------------------------------------------------------


@pytest.fixture
def memory():
    """Create a temporary MemCtrlMemory instance for each test."""
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    mem = MemCtrlMemory(db_path)
    yield mem
    os.unlink(db_path)


def test_import_and_instantiate():
    """Test that MemCtrlMemory can be imported and instantiated."""
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    try:
        mem = MemCtrlMemory(db_path)
        assert mem is not None
        assert mem.store is not None
        assert mem.builder is not None
        assert mem.retriever is not None
        assert mem.engine is not None
    finally:
        os.unlink(db_path)


def test_remember_stores_memory(memory):
    """Test that remember() stores a memory and returns an ID."""
    mid = memory.remember("User prefers dark mode", layer="user", tags=["preference"])
    assert mid is not None
    assert isinstance(mid, str)
    assert len(mid) > 0

    # Verify it was stored
    mem = memory.store.get_memory(mid)
    assert mem is not None
    assert mem.content == "User prefers dark mode"
    assert mem.layer == "user"
    assert mem.tags == ["preference"]


def test_recall_retrieves_memories(memory):
    """Test that recall() retrieves relevant memories."""
    memory.remember("Tech stack: FastAPI + PostgreSQL + Redis", layer="project")
    memory.remember("Fixed CORS bug on /login endpoint", layer="session")
    memory.remember("User prefers dark mode in all tools", layer="user")

    facts = memory.recall("what is our tech stack?", top_k=3)
    assert isinstance(facts, list)
    # Should retrieve at least one relevant fact
    assert len(facts) > 0


def test_recall_with_trace_returns_facts_and_trace(memory):
    """Test that recall_with_trace() returns facts plus trace metadata."""
    memory.remember("Tech stack: FastAPI + PostgreSQL + Redis", layer="project")
    memory.remember("Fixed CORS bug on /login endpoint", layer="session")

    result = memory.recall_with_trace("what is our tech stack?", top_k=3)
    assert isinstance(result, dict)
    assert "facts" in result
    assert "trace" in result
    assert "confidence" in result
    assert isinstance(result["facts"], list)
    assert isinstance(result["trace"], list)


def test_recall_empty_store(memory):
    """Test recall() on an empty store returns empty list."""
    facts = memory.recall("anything?")
    assert facts == []


def test_recall_with_trace_empty_store(memory):
    """Test recall_with_trace() on empty store returns sensible defaults."""
    result = memory.recall_with_trace("anything?")
    assert result["facts"] == []
    assert result["trace"] == ["empty"]
    assert result["confidence"] == 0.0


def test_get_stats_returns_stats(memory):
    """Test that get_stats() returns store statistics."""
    stats = memory.get_stats()
    assert isinstance(stats, dict)
    # Should have at least a memories count
    assert "memories" in stats

    # After inserting
    memory.remember("Some fact", layer="session")
    stats = memory.get_stats()
    assert stats["memories"] >= 1


# ---------------------------------------------------------------------------
# MemoryNode tests
# ---------------------------------------------------------------------------


@pytest.fixture
def node():
    """Create a temporary MemoryNode instance for each test."""
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    n = MemoryNode(db_path, auto_extract=True)
    yield n
    os.unlink(db_path)


def test_memory_node_instantiate():
    """Test that MemoryNode can be instantiated."""
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    try:
        node = MemoryNode(db_path, auto_extract=True)
        assert node is not None
        assert node.memory is not None
        assert node.auto_extract is True
    finally:
        os.unlink(db_path)


def test_memory_node_call_with_query(node):
    """Test MemoryNode called with state containing a memory query."""
    # Pre-seed with a memory
    node.memory.remember("We chose FastAPI for the backend API", layer="project")

    state = {
        "messages": [],
        "memory_query": "what did we choose for the backend?",
    }
    new_state = node(state)

    assert isinstance(new_state, dict)
    assert "memory_facts" in new_state
    assert "memory_trace" in new_state
    assert "memory_confidence" in new_state
    assert isinstance(new_state["memory_facts"], list)


def test_memory_node_call_auto_extract(node):
    """Test MemoryNode auto-extracts from messages when enabled."""
    state = {
        "messages": [
            {
                "role": "assistant",
                "content": "We decided to use FastAPI with PostgreSQL for the backend.",
            },
        ],
    }
    new_state = node(state)
    assert isinstance(new_state, dict)
    # Message should have been stored (auto_extract = True)
    stats = node.memory.get_stats()
    assert stats["memories"] >= 1


def test_memory_node_call_no_auto_extract():
    """Test MemoryNode does not auto-extract when disabled."""
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    try:
        node = MemoryNode(db_path, auto_extract=False)
        state = {
            "messages": [
                {
                    "role": "assistant",
                    "content": "We decided to use FastAPI with PostgreSQL for the backend.",
                },
            ],
        }
        new_state = node(state)
        assert isinstance(new_state, dict)
        stats = node.memory.get_stats()
        assert stats["memories"] == 0
    finally:
        os.unlink(db_path)


def test_memory_node_call_with_consolidation(node):
    """Test MemoryNode handles consolidation flag."""
    # Seed some session memories
    node.memory.remember("Fixed CORS bug", layer="session")
    node.memory.remember("Added pytest fixtures", layer="session")

    state = {
        "messages": [],
        "memory_consolidate": True,
    }
    new_state = node(state)
    assert isinstance(new_state, dict)
    assert "memory_consolidated" in new_state


# ---------------------------------------------------------------------------
# MemCtrlSaver tests (langgraph required)
# ---------------------------------------------------------------------------


@pytest.mark.skipif(not LANGGRAPH_AVAILABLE, reason="langgraph not installed")
def test_memctrl_saver_import():
    """Test that MemCtrlSaver can be imported when langgraph is available."""
    from memctrl.integrations.langgraph import MemCtrlSaver

    assert MemCtrlSaver is not None


@pytest.mark.skipif(not LANGGRAPH_AVAILABLE, reason="langgraph not installed")
def test_memctrl_saver_instantiate():
    """Test that MemCtrlSaver can be instantiated."""
    from memctrl.integrations.langgraph import MemCtrlSaver

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    try:
        saver = MemCtrlSaver(db_path)
        assert saver is not None
        assert saver.store is not None
    finally:
        os.unlink(db_path)


@pytest.mark.skipif(not LANGGRAPH_AVAILABLE, reason="langgraph not installed")
def test_memctrl_saver_raises_without_langgraph():
    """Test that MemCtrlSaver raises ImportError when langgraph is missing.

    This test verifies the fallback behaviour: when langgraph is not installed,
    BaseCheckpointSaver becomes `object`, so instantiation should still work
    but the saver explicitly checks LANGGRAPH_AVAILABLE and raises.
    """
    if LANGGRAPH_AVAILABLE:
        pytest.skip("langgraph is installed; this test requires it to be absent")
    from memctrl.integrations.langgraph import MemCtrlSaver

    with pytest.raises(ImportError):
        MemCtrlSaver()


def test_langgraph_not_installed_graceful():
    """Test that the module loads gracefully when langgraph is absent.

    All non-saver functionality should work without langgraph.
    Skip if langgraph is actually installed in this environment.
    """
    if LANGGRAPH_AVAILABLE:
        pytest.skip("langgraph is installed in this environment")
    # MemCtrlMemory and MemoryNode should work fine
    mem = MemCtrlMemory()
    mid = mem.remember("test fact", layer="session")
    assert mid is not None
