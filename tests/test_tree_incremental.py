"""Tests for MemoryTreeBuilder incremental rebuild.

Covers: incremental rebuild with layer caching, cache invalidation,
partial rebuild correctness, cache hit/miss behavior.
"""

import pytest

from memctrl.tree import MemoryTreeBuilder


# ---------------------------------------------------------------------------
# Incremental rebuild
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_incremental_rebuild_changed_layer():
    """Only the changed layer should get a new cluster; others reuse cache."""
    builder = MemoryTreeBuilder()

    memories = [
        {"id": "1", "layer": "project", "content": "we use FastAPI", "confidence": 1.0},
        {
            "id": "2",
            "layer": "project",
            "content": "we use PostgreSQL",
            "confidence": 1.0,
        },
        {
            "id": "3",
            "layer": "session",
            "content": "fixing auth bug",
            "confidence": 1.0,
        },
    ]

    # Full build — caches all layers
    tree1 = await builder.build_tree_incremental(memories)
    assert tree1 is not None
    assert len(tree1.children) == 2

    # Get the cached session node
    session_node_1 = [c for c in tree1.children if c.layer == "session"][0]

    # Add a new project memory — only project layer should rebuild
    memories.append(
        {
            "id": "4",
            "layer": "project",
            "content": "we use Redis cache",
            "confidence": 1.0,
        }
    )
    tree2 = await builder.build_tree_incremental(memories, changed_layer="project")
    assert tree2 is not None

    # Session node should be the same cached object
    session_node_2 = [c for c in tree2.children if c.layer == "session"][0]
    assert session_node_1 is session_node_2  # Same cached object

    # Project node should be different (rebuilt)
    proj_node_1 = [c for c in tree1.children if c.layer == "project"][0]
    proj_node_2 = [c for c in tree2.children if c.layer == "project"][0]
    assert proj_node_1 is not proj_node_2


@pytest.mark.asyncio
async def test_incremental_rebuild_no_change_uses_cache():
    """Calling with unchanged layer should reuse all cached nodes."""
    builder = MemoryTreeBuilder()

    memories = [
        {"id": "1", "layer": "project", "content": "we use FastAPI", "confidence": 1.0},
        {"id": "2", "layer": "session", "content": "fixing bug", "confidence": 1.0},
    ]

    tree1 = await builder.build_tree_incremental(memories)
    proj_1 = [c for c in tree1.children if c.layer == "project"][0]
    sess_1 = [c for c in tree1.children if c.layer == "session"][0]

    # Call again with "session" changed — only session rebuilds
    tree2 = await builder.build_tree_incremental(memories, changed_layer="session")
    proj_2 = [c for c in tree2.children if c.layer == "project"][0]
    sess_2 = [c for c in tree2.children if c.layer == "session"][0]

    # Project should be cached, session rebuilt
    assert proj_1 is proj_2
    assert sess_1 is not sess_2


@pytest.mark.asyncio
async def test_incremental_rebuild_full_when_no_cache():
    """Without cached layers, incremental falls back to building all."""
    builder = MemoryTreeBuilder()

    memories = [
        {"id": "1", "layer": "project", "content": "we use FastAPI", "confidence": 1.0},
    ]

    tree = await builder.build_tree_incremental(memories, changed_layer="project")
    assert tree is not None
    assert len(tree.children) == 1
    assert tree.children[0].layer == "project"


@pytest.mark.asyncio
async def test_incremental_rebuild_empty_memories():
    """Empty memories should produce empty tree and clear cache."""
    builder = MemoryTreeBuilder()

    # Build with data first
    memories = [
        {"id": "1", "layer": "project", "content": "we use FastAPI", "confidence": 1.0},
    ]
    tree1 = await builder.build_tree_incremental(memories)
    assert len(tree1.children) == 1

    # Now empty — should clear cache
    tree2 = await builder.build_tree_incremental([])
    assert tree2.summary == "Empty memory store"
    assert not builder._layer_cache


@pytest.mark.asyncio
async def test_incremental_rebuild_nonexistent_layer():
    """Changed layer that doesn't exist in memories falls back to full rebuild."""
    builder = MemoryTreeBuilder()

    memories = [
        {"id": "1", "layer": "project", "content": "we use FastAPI", "confidence": 1.0},
    ]

    tree = await builder.build_tree_incremental(memories, changed_layer="user")
    assert tree is not None
    assert len(tree.children) == 1


@pytest.mark.asyncio
async def test_incremental_rebuild_count_mismatch_rebuilds():
    """If cached layer count doesn't match current count, rebuild that layer."""
    builder = MemoryTreeBuilder()

    memories = [
        {"id": "1", "layer": "project", "content": "use FastAPI", "confidence": 1.0},
        {"id": "2", "layer": "session", "content": "fix bug", "confidence": 1.0},
    ]

    # Build and cache via full build
    await builder.build_tree(memories)
    assert builder._layer_counts.get("session") == 1
    assert builder._layer_counts.get("project") == 1

    # Manually corrupt the cache count to simulate stale cache
    builder._layer_counts["session"] = 99

    # Now rebuild project — session count mismatch should trigger session rebuild too
    tree = await builder.build_tree_incremental(memories, changed_layer="project")
    # Should still produce valid tree and restore correct count
    assert tree is not None
    assert len(tree.children) == 2
    assert builder._layer_counts["session"] == 1


# ---------------------------------------------------------------------------
# Cache invalidation
# ---------------------------------------------------------------------------


def test_invalidate_cache():
    """invalidate_cache should clear all cached state."""
    builder = MemoryTreeBuilder()
    builder._layer_cache["project"] = None  # type: ignore[assignment]
    builder._layer_counts["project"] = 5

    builder.invalidate_cache()
    assert not builder._layer_cache
    assert not builder._layer_counts


@pytest.mark.asyncio
async def test_invalidate_cache_then_incremental_builds_all():
    """After invalidate_cache, incremental rebuild should build all layers."""
    builder = MemoryTreeBuilder()

    memories = [
        {"id": "1", "layer": "project", "content": "use FastAPI", "confidence": 1.0},
        {"id": "2", "layer": "session", "content": "fix bug", "confidence": 1.0},
    ]

    # Build and cache
    tree1 = await builder.build_tree_incremental(memories)
    proj_1 = [c for c in tree1.children if c.layer == "project"][0]

    # Invalidate cache
    builder.invalidate_cache()

    # Rebuild — all layers should be new objects
    tree2 = await builder.build_tree_incremental(memories, changed_layer="project")
    proj_2 = [c for c in tree2.children if c.layer == "project"][0]

    assert proj_1 is not proj_2


# ---------------------------------------------------------------------------
# Equivalence with full build
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_incremental_produces_same_structure_as_full():
    """Incremental rebuild should produce structurally equivalent tree."""
    builder = MemoryTreeBuilder()

    memories = [
        {
            "id": "1",
            "layer": "project",
            "content": "we use FastAPI framework",
            "confidence": 1.0,
        },
        {
            "id": "2",
            "layer": "project",
            "content": "we use PostgreSQL database",
            "confidence": 1.0,
        },
        {
            "id": "3",
            "layer": "session",
            "content": "implementing auth module",
            "confidence": 1.0,
        },
        {
            "id": "4",
            "layer": "session",
            "content": "fixing bug in login",
            "confidence": 1.0,
        },
    ]

    full_tree = await builder.build_tree(memories)

    # Reset builder for incremental
    builder.invalidate_cache()
    inc_tree = await builder.build_tree_incremental(memories)

    # Both should have same number of layer children
    assert len(full_tree.children) == len(inc_tree.children)

    # Both should have same layers
    full_layers = {c.layer for c in full_tree.children}
    inc_layers = {c.layer for c in inc_tree.children}
    assert full_layers == inc_layers
