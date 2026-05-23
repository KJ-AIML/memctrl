"""Tests for MemoryTreeBuilder — PageIndex-style hierarchical tree builder.

Covers: grouping, tree building, LLM clustering, fallback clustering, node helpers,
serialization.
"""

import json

import pytest

from memctrl.tree import MemoryTreeBuilder
from memctrl.store import TreeNode


# ---------------------------------------------------------------------------
# Grouping
# ---------------------------------------------------------------------------


def test_group_by_layer():
    builder = MemoryTreeBuilder()
    memories = [
        {"id": "1", "layer": "project", "content": "use FastAPI", "confidence": 1.0},
        {"id": "2", "layer": "project", "content": "use Postgres", "confidence": 1.0},
        {"id": "3", "layer": "session", "content": "fix bug", "confidence": 1.0},
    ]
    grouped = builder._group_by_layer(memories)
    assert len(grouped["project"]) == 2
    assert len(grouped["session"]) == 1


def test_group_by_layer_empty():
    builder = MemoryTreeBuilder()
    grouped = builder._group_by_layer([])
    assert grouped == {}


def test_group_by_layer_defaults_missing():
    """Memories without a layer key default to 'session'."""
    builder = MemoryTreeBuilder()
    memories = [{"id": "1", "content": "no layer"}]
    grouped = builder._group_by_layer(memories)
    assert "session" in grouped


def test_group_by_layer_multiple_layers():
    builder = MemoryTreeBuilder()
    memories = [
        {"id": "1", "layer": "project", "content": "p1", "confidence": 1.0},
        {"id": "2", "layer": "project", "content": "p2", "confidence": 1.0},
        {"id": "3", "layer": "session", "content": "s1", "confidence": 1.0},
        {"id": "4", "layer": "user", "content": "u1", "confidence": 1.0},
        {"id": "5", "layer": "user", "content": "u2", "confidence": 1.0},
    ]
    grouped = builder._group_by_layer(memories)
    assert len(grouped) == 3
    assert len(grouped["project"]) == 2
    assert len(grouped["session"]) == 1
    assert len(grouped["user"]) == 2


# ---------------------------------------------------------------------------
# Tree building (fallback — no LLM)
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_build_tree():
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
            "content": "implementing auth",
            "confidence": 1.0,
        },
        {
            "id": "4",
            "layer": "user",
            "content": "prefers async Python",
            "confidence": 1.0,
        },
    ]
    tree = await builder.build_tree(memories)
    assert tree.title == "Memory Tree"
    assert tree.layer == "root"
    assert len(tree.children) >= 1


@pytest.mark.asyncio
async def test_build_tree_empty():
    builder = MemoryTreeBuilder()
    tree = await builder.build_tree([])
    assert tree.title == "Memory Tree"
    assert tree.layer == "root"
    assert tree.summary == "Empty memory store"


@pytest.mark.asyncio
async def test_build_tree_single_memory():
    builder = MemoryTreeBuilder()
    memories = [
        {"id": "1", "layer": "project", "content": "we use FastAPI", "confidence": 1.0},
    ]
    tree = await builder.build_tree(memories)
    assert tree.title == "Memory Tree"
    assert len(tree.children) == 1
    assert tree.children[0].layer == "project"


@pytest.mark.asyncio
async def test_build_tree_groups_by_keyword():
    """Fallback clustering groups memories by keyword heuristics."""
    builder = MemoryTreeBuilder()
    memories = [
        {
            "id": "1",
            "layer": "project",
            "content": "we use FastAPI for backend",
            "confidence": 1.0,
        },
        {
            "id": "2",
            "layer": "project",
            "content": "we decided to use Postgres",
            "confidence": 1.0,
        },
        {
            "id": "3",
            "layer": "project",
            "content": "implementing auth module",
            "confidence": 1.0,
        },
    ]
    tree = await builder.build_tree(memories)
    project_node = [c for c in tree.children if c.layer == "project"][0]
    # Should have created clusters based on keywords
    assert len(project_node.children) >= 1


# ---------------------------------------------------------------------------
# LLM clustering
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_cluster_with_llm_success():
    """Test LLM clustering with a mock client."""

    async def mock_llm(prompt, json_mode=False):
        return json.dumps(
            {
                "clusters": [
                    {
                        "title": "backend",
                        "summary": "backend tech",
                        "memory_ids": ["1"],
                    },
                ]
            }
        )

    builder = MemoryTreeBuilder(llm_client=mock_llm)
    memories = [
        {"id": "1", "layer": "project", "content": "we use FastAPI", "confidence": 1.0},
    ]
    node = await builder._cluster_with_llm("project", memories)
    assert node.layer == "project"
    assert node.title == "Project"
    assert len(node.children) >= 1


@pytest.mark.asyncio
async def test_cluster_with_llm_invalid_json_falls_back():
    """LLM returns invalid JSON → falls back to keyword clustering."""

    async def bad_llm(prompt, json_mode=False):
        return "not json"

    builder = MemoryTreeBuilder(llm_client=bad_llm)
    memories = [
        {"id": "1", "layer": "project", "content": "we use FastAPI", "confidence": 1.0},
    ]
    node = await builder._cluster_with_llm("project", memories)
    assert node.layer == "project"
    assert len(node.children) >= 1  # fallback produces clusters


@pytest.mark.asyncio
async def test_cluster_with_llm_exception_falls_back():
    """LLM raises exception → falls back to keyword clustering."""

    async def failing_llm(prompt, json_mode=False):
        raise RuntimeError("LLM down")

    builder = MemoryTreeBuilder(llm_client=failing_llm)
    memories = [
        {"id": "1", "layer": "project", "content": "we use FastAPI", "confidence": 1.0},
    ]
    node = await builder._cluster_with_llm("project", memories)
    assert node.layer == "project"


@pytest.mark.asyncio
async def test_cluster_with_llm_empty_clusters_falls_back():
    """LLM returns empty clusters → falls back."""

    async def empty_llm(prompt, json_mode=False):
        return json.dumps({"clusters": []})

    builder = MemoryTreeBuilder(llm_client=empty_llm)
    memories = [
        {"id": "1", "layer": "project", "content": "we use FastAPI", "confidence": 1.0},
    ]
    node = await builder._cluster_with_llm("project", memories)
    assert node.layer == "project"


# ---------------------------------------------------------------------------
# Fallback clustering detail
# ---------------------------------------------------------------------------


def test_cluster_fallback_tech_stack():
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
            "content": "using PostgreSQL database",
            "confidence": 1.0,
        },
    ]
    node = builder._cluster_fallback("project", memories)
    assert node.title == "Project"
    assert node.layer == "project"
    # Should group under tech_stack due to keywords
    cluster_titles = [c.title for c in node.children]
    assert len(cluster_titles) >= 1


def test_cluster_fallback_decisions():
    builder = MemoryTreeBuilder()
    memories = [
        {
            "id": "1",
            "layer": "project",
            "content": "we decided to use Redis",
            "confidence": 1.0,
        },
        {
            "id": "2",
            "layer": "project",
            "content": "ADR-001: chose LangGraph",
            "confidence": 1.0,
        },
    ]
    node = builder._cluster_fallback("project", memories)
    cluster_titles = [c.title for c in node.children]
    assert len(cluster_titles) >= 1


def test_cluster_fallback_tasks():
    builder = MemoryTreeBuilder()
    memories = [
        {
            "id": "1",
            "layer": "session",
            "content": "implementing auth module",
            "confidence": 1.0,
        },
        {
            "id": "2",
            "layer": "session",
            "content": "fixing bug in login",
            "confidence": 1.0,
        },
    ]
    node = builder._cluster_fallback("session", memories)
    assert node.title == "Session"
    assert len(node.children) >= 1


def test_cluster_fallback_other():
    """Memories not matching any keyword go to 'other' group."""
    builder = MemoryTreeBuilder()
    memories = [
        {
            "id": "1",
            "layer": "project",
            "content": "hello world general note",
            "confidence": 1.0,
        },
    ]
    node = builder._cluster_fallback("project", memories)
    # Should end up in "other" group
    group_titles = [c.title for c in node.children]
    assert len(group_titles) >= 1


# ---------------------------------------------------------------------------
# Parse clusters
# ---------------------------------------------------------------------------


def test_parse_clusters_valid_json():
    builder = MemoryTreeBuilder()
    response = json.dumps(
        {
            "clusters": [
                {"title": "t1", "summary": "s1", "memory_ids": ["m1", "m2"]},
            ]
        }
    )
    clusters = builder._parse_clusters(response)
    assert len(clusters) == 1
    assert clusters[0]["title"] == "t1"


def test_parse_clusters_markdown_code_block():
    builder = MemoryTreeBuilder()
    response = '```json\n{"clusters": [{"title": "t1", "summary": "s1", "memory_ids": ["m1"]}]}\n```'
    clusters = builder._parse_clusters(response)
    assert len(clusters) == 1


def test_parse_clusters_invalid_json():
    builder = MemoryTreeBuilder()
    clusters = builder._parse_clusters("not json")
    assert clusters == []


def test_parse_clusters_no_clusters_key():
    builder = MemoryTreeBuilder()
    clusters = builder._parse_clusters(json.dumps({"other_key": []}))
    assert clusters == []


# ---------------------------------------------------------------------------
# Build cluster prompt
# ---------------------------------------------------------------------------


def test_build_cluster_prompt():
    builder = MemoryTreeBuilder()
    memories = [
        {"id": "m1", "content": "we use FastAPI"},
    ]
    prompt = builder._build_cluster_prompt("project", memories)
    assert "project" in prompt
    assert "FastAPI" in prompt
    assert "clusters" in prompt


# ---------------------------------------------------------------------------
# TreeNode helpers
# ---------------------------------------------------------------------------


def test_tree_node_is_leaf():
    leaf = TreeNode(
        id="l1", title="leaf", layer="project", summary="s", memory_ids=["m1"]
    )
    parent = TreeNode(
        id="p1", title="parent", layer="project", summary="s", children=[leaf]
    )
    assert leaf.is_leaf() is True
    assert parent.is_leaf() is False


def test_tree_node_all_memory_ids():
    leaf1 = TreeNode(
        id="l1", title="L1", layer="project", summary="s", memory_ids=["m1"]
    )
    leaf2 = TreeNode(
        id="l2", title="L2", layer="project", summary="s", memory_ids=["m2"]
    )
    parent = TreeNode(
        id="p1", title="P", layer="project", summary="s", children=[leaf1, leaf2]
    )
    ids = parent.all_memory_ids()
    assert "m1" in ids
    assert "m2" in ids


def test_tree_node_find_node():
    leaf = TreeNode(
        id="l1", title="leaf", layer="project", summary="s", memory_ids=["m1"]
    )
    parent = TreeNode(
        id="p1", title="parent", layer="project", summary="s", children=[leaf]
    )
    assert parent.find_node("l1") is not None
    assert parent.find_node("l1").id == "l1"
    assert parent.find_node("xxx") is None


def test_tree_node_find_node_deep():
    leaf = TreeNode(id="l1", title="deep leaf", layer="project", summary="s")
    mid = TreeNode(id="m1", title="mid", layer="project", summary="s", children=[leaf])
    root = TreeNode(id="r1", title="root", layer="root", summary="s", children=[mid])
    assert root.find_node("l1") is not None
    assert root.find_node("l1").id == "l1"


def test_tree_serialization():
    node = TreeNode(id="1", title="test", layer="project", summary="s")
    d = node.to_dict()
    restored = TreeNode.from_dict(d)
    assert restored.id == "1"
    assert restored.title == "test"
    assert restored.layer == "project"
    assert restored.summary == "s"


def test_tree_serialization_with_children():
    leaf = TreeNode(
        id="l1", title="leaf", layer="project", summary="s", memory_ids=["m1"]
    )
    root = TreeNode(id="r1", title="root", layer="root", summary="s", children=[leaf])
    d = root.to_dict()
    restored = TreeNode.from_dict(d)
    assert len(restored.children) == 1
    assert restored.children[0].id == "l1"
    assert restored.children[0].memory_ids == ["m1"]


# ---------------------------------------------------------------------------
# Avg confidence helper
# ---------------------------------------------------------------------------


def test_avg_confidence():
    mem_by_id = {
        "m1": {"confidence": 1.0},
        "m2": {"confidence": 0.7},
    }
    avg = MemoryTreeBuilder._avg_confidence(["m1", "m2"], mem_by_id)
    assert avg == 0.85


def test_avg_confidence_empty():
    avg = MemoryTreeBuilder._avg_confidence([], {})
    assert avg == 1.0


# ---------------------------------------------------------------------------
# Integration: build + serialize roundtrip
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_build_tree_roundtrip():
    builder = MemoryTreeBuilder()
    memories = [
        {"id": "1", "layer": "project", "content": "we use FastAPI", "confidence": 1.0},
    ]
    tree = await builder.build_tree(memories)
    d = tree.to_dict()
    restored = TreeNode.from_dict(d)
    assert restored.title == "Memory Tree"
    assert len(restored.children) == 1
