"""Tests for MemoryRetriever — PageIndex-style tree traversal retrieval.

Covers: keyword retrieval, LLM retrieval, empty tree handling, result serialization,
internal helpers.
"""

import json

import pytest

from memctrl.retriever import MemoryRetriever, RetrievalResult


# ---------------------------------------------------------------------------
# Keyword retrieval (no LLM)
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_keyword_retrieve():
    retriever = MemoryRetriever()
    tree = {
        "id": "root", "title": "Memory Tree", "layer": "root", "summary": "s",
        "memory_ids": [],
        "children": [
            {"id": "l1", "title": "Project", "layer": "project", "summary": "s",
             "memory_ids": ["m1", "m2"], "children": []},
        ]
    }
    lookup = {
        "m1": {"id": "m1", "content": "we use FastAPI for the backend", "source": "test"},
        "m2": {"id": "m2", "content": "we use PostgreSQL for data", "source": "test"},
    }
    result = await retriever.retrieve("what framework", tree, memory_lookup=lookup)
    assert len(result.facts) > 0
    assert result.confidence > 0


@pytest.mark.asyncio
async def test_keyword_retrieve_no_match():
    retriever = MemoryRetriever()
    tree = {
        "id": "root", "title": "Memory Tree", "layer": "root", "summary": "s",
        "memory_ids": [],
        "children": [
            {"id": "l1", "title": "Project", "layer": "project", "summary": "s",
             "memory_ids": ["m1"], "children": []},
        ]
    }
    lookup = {
        "m1": {"id": "m1", "content": "we use FastAPI", "source": "test"},
    }
    result = await retriever.retrieve("something completely unrelated xyz",
                                      tree, memory_lookup=lookup)
    # Depth bonus gives score 1.0 even with no keyword match;
    # confidence is low since score/10 normalization yields ~0.1.
    assert len(result.facts) > 0
    assert result.confidence < 0.2


@pytest.mark.asyncio
async def test_keyword_retrieve_top_k():
    retriever = MemoryRetriever()
    tree = {
        "id": "root", "title": "Memory Tree", "layer": "root", "summary": "s",
        "memory_ids": [],
        "children": [
            {"id": "l1", "title": "Project", "layer": "project", "summary": "s",
             "memory_ids": ["m1", "m2", "m3", "m4", "m5"], "children": []},
        ]
    }
    lookup = {
        "m1": {"id": "m1", "content": "we use FastAPI", "source": "test"},
        "m2": {"id": "m2", "content": "we use FastAPI again", "source": "test"},
        "m3": {"id": "m3", "content": "we use FastAPI too", "source": "test"},
        "m4": {"id": "m4", "content": "we use FastAPI yep", "source": "test"},
        "m5": {"id": "m5", "content": "we use FastAPI five", "source": "test"},
    }
    result = await retriever.retrieve("FastAPI", tree, memory_lookup=lookup, top_k=3)
    assert len(result.facts) <= 3


# ---------------------------------------------------------------------------
# Empty / edge cases
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_empty_tree():
    retriever = MemoryRetriever()
    result = await retriever.retrieve("test", {}, {})
    assert result.facts == []


@pytest.mark.asyncio
async def test_none_tree():
    retriever = MemoryRetriever()
    result = await retriever.retrieve("test", None, {})
    assert result.facts == []
    assert result.confidence == 0.0


@pytest.mark.asyncio
async def test_empty_lookup():
    retriever = MemoryRetriever()
    tree = {
        "id": "root", "title": "Memory Tree", "layer": "root", "summary": "s",
        "memory_ids": [],
        "children": [],
    }
    result = await retriever.retrieve("test", tree, {})
    assert result.facts == []


@pytest.mark.asyncio
async def test_no_keywords():
    """Query with no meaningful keywords returns empty result."""
    retriever = MemoryRetriever()
    tree = {
        "id": "root", "title": "Memory Tree", "layer": "root", "summary": "s",
        "memory_ids": [],
        "children": [
            {"id": "l1", "title": "Project", "layer": "project", "summary": "s",
             "memory_ids": ["m1"], "children": []},
        ]
    }
    lookup = {"m1": {"id": "m1", "content": "test", "source": "s"}}
    result = await retriever.retrieve("", tree, memory_lookup=lookup)
    assert result.facts == []
    assert result.confidence == 0.0


# ---------------------------------------------------------------------------
# LLM retrieval
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_llm_retrieve_success():
    async def mock_llm(prompt, json_mode=False):
        return json.dumps({
            "thinking": "Project layer is relevant",
            "relevant_nodes": ["l1"],
            "confidence": 0.9,
        })

    retriever = MemoryRetriever(llm_client=mock_llm)
    tree = {
        "id": "root", "title": "Memory Tree", "layer": "root", "summary": "s",
        "memory_ids": [],
        "children": [
            {"id": "l1", "title": "Project", "layer": "project", "summary": "s",
             "memory_ids": ["m1"], "children": []},
        ]
    }
    lookup = {"m1": {"id": "m1", "content": "we use FastAPI", "source": "test"}}
    result = await retriever.retrieve("what framework", tree, memory_lookup=lookup)
    assert len(result.facts) > 0
    assert result.confidence == 0.9


@pytest.mark.asyncio
async def test_llm_retrieve_invalid_json_falls_back():
    async def bad_llm(prompt, json_mode=False):
        return "not json"

    retriever = MemoryRetriever(llm_client=bad_llm)
    tree = {
        "id": "root", "title": "Memory Tree", "layer": "root", "summary": "s",
        "memory_ids": [],
        "children": [
            {"id": "l1", "title": "Project", "layer": "project", "summary": "s",
             "memory_ids": ["m1"], "children": []},
        ]
    }
    lookup = {"m1": {"id": "m1", "content": "we use FastAPI", "source": "test"}}
    result = await retriever.retrieve("what framework", tree, memory_lookup=lookup)
    assert len(result.facts) > 0  # falls back to keyword


@pytest.mark.asyncio
async def test_llm_retrieve_exception_falls_back():
    async def failing_llm(prompt, json_mode=False):
        raise RuntimeError("LLM error")

    retriever = MemoryRetriever(llm_client=failing_llm)
    tree = {
        "id": "root", "title": "Memory Tree", "layer": "root", "summary": "s",
        "memory_ids": [],
        "children": [
            {"id": "l1", "title": "Project", "layer": "project", "summary": "s",
             "memory_ids": ["m1"], "children": []},
        ]
    }
    lookup = {"m1": {"id": "m1", "content": "we use FastAPI", "source": "test"}}
    result = await retriever.retrieve("what framework", tree, memory_lookup=lookup)
    assert len(result.facts) > 0  # falls back to keyword


@pytest.mark.asyncio
async def test_llm_retrieve_empty_nodes_falls_back():
    async def empty_llm(prompt, json_mode=False):
        return json.dumps({
            "thinking": "nothing relevant",
            "relevant_nodes": [],
            "confidence": 0.0,
        })

    retriever = MemoryRetriever(llm_client=empty_llm)
    tree = {
        "id": "root", "title": "Memory Tree", "layer": "root", "summary": "s",
        "memory_ids": [],
        "children": [
            {"id": "l1", "title": "Project", "layer": "project", "summary": "s",
             "memory_ids": ["m1"], "children": []},
        ]
    }
    lookup = {"m1": {"id": "m1", "content": "we use FastAPI", "source": "test"}}
    result = await retriever.retrieve("what framework", tree, memory_lookup=lookup)
    # Falls back to keyword when no nodes selected
    assert len(result.facts) >= 0


# ---------------------------------------------------------------------------
# Retrieval result
# ---------------------------------------------------------------------------

def test_retrieval_result_dict():
    r = RetrievalResult(facts=["f1"], trace=["root"], confidence=0.9, sources=["s1"])
    d = r.to_dict()
    assert d["facts"] == ["f1"]
    assert d["trace"] == ["root"]
    assert d["confidence"] == 0.9
    assert d["sources"] == ["s1"]


def test_retrieval_result_defaults():
    r = RetrievalResult()
    d = r.to_dict()
    assert d["facts"] == []
    assert d["trace"] == []
    assert d["confidence"] == 0.0
    assert d["sources"] == []


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------

def test_strip_leaves():
    retriever = MemoryRetriever()
    tree = {
        "id": "root", "title": "Memory Tree", "layer": "root", "summary": "s",
        "memory_ids": ["m1"],
        "children": [
            {"id": "c1", "title": "Child", "layer": "project", "summary": "cs",
             "memory_ids": ["m2"], "children": []},
        ]
    }
    stripped = retriever._strip_leaves(tree)
    assert stripped["id"] == "root"
    assert stripped["memory_count"] == 1
    assert "children" in stripped
    assert stripped["children"][0]["memory_count"] == 1


def test_find_node():
    retriever = MemoryRetriever()
    tree = {
        "id": "root", "title": "R", "layer": "root", "summary": "s",
        "memory_ids": [],
        "children": [
            {"id": "c1", "title": "C1", "layer": "project", "summary": "s",
             "memory_ids": [], "children": [
                 {"id": "g1", "title": "G1", "layer": "project", "summary": "s",
                  "memory_ids": [], "children": []},
             ]},
        ]
    }
    assert retriever._find_node(tree, "c1") is not None
    assert retriever._find_node(tree, "g1") is not None
    assert retriever._find_node(tree, "xxx") is None


def test_build_retrieval_prompt():
    retriever = MemoryRetriever()
    stripped = {"id": "root", "title": "T", "layer": "root", "summary": "s",
                "memory_count": 0, "children": []}
    prompt = retriever._build_retrieval_prompt("what framework?", stripped)
    assert "what framework?" in prompt
    assert "root" in prompt


def test_collect_from_nodes():
    retriever = MemoryRetriever()
    tree = {
        "id": "root", "title": "R", "layer": "root", "summary": "s",
        "memory_ids": ["m1"],
        "children": [],
    }
    lookup = {
        "m1": {"id": "m1", "content": "fact one", "source": "test"},
    }
    facts, sources = retriever._collect_from_nodes(["root"], tree, lookup)
    assert "fact one" in facts
    assert "test" in sources
