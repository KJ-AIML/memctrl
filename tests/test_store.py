"""Tests for MemoryStore — SQLite data layer.

Covers: memory CRUD, expiration, consolidation, tree nodes, trigger logs, stats.
"""

import os
import tempfile
from datetime import datetime, timedelta

import pytest

from memctrl.store import MemoryStore, Memory, TreeNode, TriggerLog


@pytest.fixture
def store():
    """Create a temporary MemoryStore for each test."""
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    s = MemoryStore(db_path)
    yield s
    s.close()
    os.unlink(db_path)


# ---------------------------------------------------------------------------
# Memory CRUD
# ---------------------------------------------------------------------------


def test_insert_and_get_memory(store):
    mid = store.insert_memory("project", "we use FastAPI", "test")
    mem = store.get_memory(mid)
    assert mem is not None
    assert mem.content == "we use FastAPI"
    assert mem.layer == "project"
    assert mem.confidence == 1.0
    assert mem.source == "test"


def test_insert_memory_with_tags(store):
    mid = store.insert_memory(
        "project", "tagged content", "test", tags=["important", "arch"]
    )
    mem = store.get_memory(mid)
    assert mem.tags == ["important", "arch"]


def test_insert_memory_with_confidence(store):
    mid = store.insert_memory("project", "inferred", "test", confidence=0.7)
    mem = store.get_memory(mid)
    assert mem.confidence == 0.7


def test_get_memory_missing(store):
    assert store.get_memory("nonexistent-id") is None


def test_list_memories(store):
    store.insert_memory("project", "content A", "test")
    store.insert_memory("session", "content B", "test")
    store.insert_memory("user", "content C", "test")
    all_mems = store.list_memories()
    assert len(all_mems) == 3
    project_mems = store.list_memories("project")
    assert len(project_mems) == 1


def test_list_memories_ordered_by_created(store):
    mid1 = store.insert_memory("session", "first", "test")
    mid2 = store.insert_memory("session", "second", "test")
    mems = store.list_memories("session")
    assert mems[0].id == mid2  # DESC order
    assert mems[1].id == mid1


def test_delete_memory(store):
    mid = store.insert_memory("session", "to delete", "test")
    assert store.delete_memory(mid) is True
    assert store.get_memory(mid) is None


def test_delete_memory_missing(store):
    assert store.delete_memory("nonexistent-id") is False


def test_update_memory_layer(store):
    mid = store.insert_memory("session", "will move", "test")
    assert store.update_memory_layer(mid, "project") is True
    mem = store.get_memory(mid)
    assert mem.layer == "project"


def test_update_memory_layer_missing(store):
    assert store.update_memory_layer("nonexistent-id", "project") is False


# ---------------------------------------------------------------------------
# Expiration
# ---------------------------------------------------------------------------


def test_expire_old_memories(store):
    expired = datetime.now() - timedelta(days=1)
    store.insert_memory("session", "old", "test", expires_at=expired)
    store.insert_memory("session", "new", "test")
    count = store.expire_old_memories()
    assert count == 1
    remaining = store.list_memories()
    assert len(remaining) == 1
    assert remaining[0].content == "new"


def test_expire_old_memories_none_expired(store):
    future = datetime.now() + timedelta(days=7)
    store.insert_memory("session", "future", "test", expires_at=future)
    count = store.expire_old_memories()
    assert count == 0


def test_expire_old_memories_no_expiry(store):
    store.insert_memory("session", "no expiry", "test")
    count = store.expire_old_memories()
    assert count == 0


# ---------------------------------------------------------------------------
# Consolidation
# ---------------------------------------------------------------------------


def test_consolidate(store):
    store.insert_memory("session", "task 1", "test")
    store.insert_memory("session", "task 2", "test")
    ids = store.consolidate("session", "project")
    assert len(ids) == 2
    assert len(store.list_memories("session")) == 0
    assert len(store.list_memories("project")) == 2


def test_consolidate_empty_layer(store):
    ids = store.consolidate("session", "project")
    assert ids == []


def test_consolidate_with_tags(store):
    store.insert_memory("session", "tagged", "test", tags=["todo"])
    store.insert_memory("session", "untagged", "test")
    ids = store.consolidate("session", "project")
    assert len(ids) == 2
    for mem in store.list_memories("project"):
        assert mem.layer == "project"


# ---------------------------------------------------------------------------
# Tree nodes
# ---------------------------------------------------------------------------


def test_tree_nodes(store):
    node = TreeNode(id="n1", title="tech", layer="project", summary="tech stuff")
    store.insert_tree_node(node)
    nodes = store.get_tree_nodes()
    assert len(nodes) == 1
    assert nodes[0]["id"] == "n1"
    assert nodes[0]["title"] == "tech"


def test_tree_nodes_by_layer(store):
    node_proj = TreeNode(id="n1", title="proj", layer="project", summary="p")
    node_sess = TreeNode(id="n2", title="sess", layer="session", summary="s")
    store.insert_tree_node(node_proj)
    store.insert_tree_node(node_sess)
    proj_nodes = store.get_tree_nodes("project")
    assert len(proj_nodes) == 1
    assert proj_nodes[0]["title"] == "proj"


def test_tree_nodes_with_parent(store):
    parent = TreeNode(id="p1", title="parent", layer="project", summary="p")
    child = TreeNode(id="c1", title="child", layer="project", summary="c")
    store.insert_tree_node(parent)
    store.insert_tree_node(child, parent_id="p1")
    nodes = store.get_tree_nodes()
    child_node = [n for n in nodes if n["id"] == "c1"][0]
    assert child_node["parent_id"] == "p1"


def test_build_tree_from_nodes(store):
    root = TreeNode(id="r1", title="Root", layer="project", summary="root")
    child = TreeNode(id="c1", title="Child", layer="project", summary="child")
    store.insert_tree_node(root)
    store.insert_tree_node(child, parent_id="r1")
    tree = store.build_tree_from_nodes()
    assert tree is not None
    assert tree.id == "r1"
    assert len(tree.children) == 1


def test_build_tree_from_nodes_multiple_roots(store):
    r1 = TreeNode(id="r1", title="R1", layer="project", summary="s")
    r2 = TreeNode(id="r2", title="R2", layer="session", summary="s")
    store.insert_tree_node(r1)
    store.insert_tree_node(r2)
    tree = store.build_tree_from_nodes()
    assert tree.id == "root"
    assert len(tree.children) == 2


def test_build_tree_from_nodes_empty(store):
    assert store.build_tree_from_nodes() is None


def test_clear_tree_nodes(store):
    node = TreeNode(id="n1", title="x", layer="project", summary="y")
    store.insert_tree_node(node)
    store.clear_tree_nodes()
    assert len(store.get_tree_nodes()) == 0


# ---------------------------------------------------------------------------
# Trigger log
# ---------------------------------------------------------------------------


def test_trigger_log(store):
    tid = store.log_trigger("on_commit", "consolidate", ["m1", "m2"])
    assert tid is not None
    logs = store.get_trigger_log()
    assert len(logs) == 1
    assert logs[0].event == "on_commit"
    assert logs[0].action == "consolidate"
    assert logs[0].memories_affected == ["m1", "m2"]


def test_trigger_log_limit(store):
    store.log_trigger("event1", "action1", ["a"])
    store.log_trigger("event2", "action2", ["b"])
    store.log_trigger("event3", "action3", ["c"])
    logs = store.get_trigger_log(limit=2)
    assert len(logs) == 2


def test_trigger_log_returns_triggerlog_type(store):
    store.log_trigger("e", "a", ["m1"])
    logs = store.get_trigger_log()
    assert isinstance(logs[0], TriggerLog)


# ---------------------------------------------------------------------------
# Stats
# ---------------------------------------------------------------------------


def test_stats(store):
    store.insert_memory("project", "test", "test")
    store.insert_memory("session", "test2", "test")
    node = TreeNode(id="n1", title="x", layer="project", summary="y")
    store.insert_tree_node(node)
    store.log_trigger("e", "a", ["m1"])
    stats = store.stats()
    assert stats["memories"] == 2
    assert stats["tree_nodes"] == 1
    assert stats["triggers"] == 1


def test_stats_empty(store):
    stats = store.stats()
    assert stats["memories"] == 0
    assert stats["tree_nodes"] == 0
    assert stats["triggers"] == 0


# ---------------------------------------------------------------------------
# Memory dataclass
# ---------------------------------------------------------------------------


def test_memory_to_dict(store):
    mid = store.insert_memory("project", "test", "test", tags=["a"])
    mem = store.get_memory(mid)
    d = mem.to_dict()
    assert d["content"] == "test"
    assert d["layer"] == "project"
    assert d["tags"] == ["a"]


def test_memory_from_row_requires_row():
    """Memory.from_row requires a sqlite3.Row — tested indirectly via store."""
    mid = "test-id"
    import sqlite3

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    conn.execute("""
        CREATE TABLE test_mem (id TEXT, layer TEXT, content TEXT, source TEXT,
                               confidence REAL, created_at TEXT, expires_at TEXT, tags TEXT)
    """)
    conn.execute(
        "INSERT INTO test_mem VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        (mid, "project", "c", "s", 1.0, datetime.now().isoformat(), None, "[]"),
    )
    conn.commit()
    row = conn.execute("SELECT * FROM test_mem WHERE id = ?", (mid,)).fetchone()
    mem = Memory.from_row(row)
    assert mem.id == mid
    assert mem.layer == "project"
    conn.close()
    os.unlink(db_path)


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
    assert set(parent.all_memory_ids()) == {"m1", "m2"}


def test_tree_node_find_node():
    leaf = TreeNode(
        id="l1", title="leaf", layer="project", summary="s", memory_ids=["m1"]
    )
    parent = TreeNode(
        id="p1", title="parent", layer="project", summary="s", children=[leaf]
    )
    assert parent.find_node("l1") is not None
    assert parent.find_node("xxx") is None


def test_tree_node_serialization():
    node = TreeNode(id="1", title="test", layer="project", summary="s")
    d = node.to_dict()
    restored = TreeNode.from_dict(d)
    assert restored.id == "1"
    assert restored.title == "test"
    assert restored.layer == "project"


def test_tree_node_roundtrip_with_children():
    leaf = TreeNode(
        id="l1",
        title="leaf",
        layer="project",
        summary="leaf summary",
        memory_ids=["m1"],
        confidence=0.8,
    )
    root = TreeNode(
        id="r1",
        title="root",
        layer="root",
        summary="root summary",
        children=[leaf],
        confidence=1.0,
    )
    d = root.to_dict()
    restored = TreeNode.from_dict(d)
    assert len(restored.children) == 1
    assert restored.children[0].id == "l1"
    assert restored.children[0].confidence == 0.8


# ---------------------------------------------------------------------------
# TriggerLog dataclass
# ---------------------------------------------------------------------------


def test_triggerlog_to_dict():
    log = TriggerLog(
        id="t1",
        event="on_commit",
        action="consolidate",
        memories_affected=["m1"],
        timestamp=datetime.now(),
    )
    d = log.to_dict()
    assert d["event"] == "on_commit"
    assert d["memories_affected"] == ["m1"]


# ---------------------------------------------------------------------------
# Atomic consolidation with audit
# ---------------------------------------------------------------------------


def test_consolidate_with_audit_moves_and_reflects(store):
    m1 = store.insert_memory("session", "discussed auth", "test")
    m2 = store.insert_memory("session", "chose FastAPI", "test")

    consolidated_ids, rid = store.consolidate_with_audit(
        from_layer="session",
        to_layer="project",
        reflection_content="Session reflection (explicit): worked on auth",
        reflection_source="reflection",
        event="explicit",
        action="reflection_consolidate",
    )

    assert set(consolidated_ids) == {m1, m2}
    assert rid is not None

    # Memories moved to project
    for mid in [m1, m2]:
        mem = store.get_memory(mid)
        assert mem.layer == "project"

    # Reflection memory created
    rmem = store.get_memory(rid)
    assert rmem.layer == "project"
    assert "worked on auth" in rmem.content
    assert rmem.source == "reflection"

    # Trigger logged
    logs = store.get_trigger_log(limit=10)
    assert any(log.event == "explicit" for log in logs)


def test_consolidate_with_audit_empty_session(store):
    consolidated_ids, rid = store.consolidate_with_audit(
        from_layer="session",
        to_layer="project",
        reflection_content="No work done",
        reflection_source="reflection",
        event="explicit",
        action="reflection_consolidate",
    )
    assert consolidated_ids == []
    assert rid is None


# ---------------------------------------------------------------------------
# Provenance persistence
# ---------------------------------------------------------------------------


def test_save_and_get_provenance(store):
    pid = store.save_provenance(
        {
            "query": "what is our stack?",
            "timestamp": datetime.now().isoformat(),
            "method": "keyword",
            "tree_version": 3,
            "total_memories_searched": 12,
            "avg_confidence": 0.85,
            "sources": [{"memory_id": "m1", "layer": "project", "confidence": 1.0}],
        }
    )
    assert pid

    records = store.get_provenance(limit=10)
    assert len(records) == 1
    assert records[0]["query"] == "what is our stack?"
    assert records[0]["avg_confidence"] == 0.85
    assert len(records[0]["sources"]) == 1


def test_clear_provenance(store):
    store.save_provenance({"query": "q1", "sources": []})
    store.clear_provenance()
    assert store.get_provenance() == []


# ---------------------------------------------------------------------------
# OTel span persistence
# ---------------------------------------------------------------------------


def test_save_and_get_otel_spans(store):
    sid = store.save_otel_span(
        {
            "trace_id": "t1",
            "span_id": "s1",
            "operation": "store",
            "timestamp": 1700000000.0,
            "duration_ms": 5.0,
            "memory_id": "m1",
            "layer": "project",
            "status": "ok",
            "attributes": {"content": "we use FastAPI"},
            "service_name": "memctrl",
        }
    )
    assert sid

    spans = store.get_otel_spans(limit=10)
    assert len(spans) == 1
    assert spans[0]["operation"] == "store"
    assert spans[0]["attributes"]["content"] == "we use FastAPI"


def test_get_otel_spans_by_trace_id(store):
    store.save_otel_span(
        {
            "trace_id": "trace-a",
            "span_id": "s1",
            "operation": "store",
            "timestamp": 1.0,
            "duration_ms": 1.0,
            "status": "ok",
            "service_name": "memctrl",
        }
    )
    store.save_otel_span(
        {
            "trace_id": "trace-b",
            "span_id": "s2",
            "operation": "retrieve",
            "timestamp": 2.0,
            "duration_ms": 1.0,
            "status": "ok",
            "service_name": "memctrl",
        }
    )
    spans = store.get_otel_spans(trace_id="trace-a")
    assert len(spans) == 1
    assert spans[0]["span_id"] == "s1"


def test_prune_otel_spans(store):
    for i in range(5):
        store.save_otel_span(
            {
                "trace_id": "t1",
                "span_id": f"s{i}",
                "operation": "store",
                "timestamp": float(i),
                "duration_ms": 1.0,
                "status": "ok",
                "service_name": "memctrl",
            }
        )
    deleted = store.prune_otel_spans(max_rows=2)
    assert deleted == 3
    assert len(store.get_otel_spans()) == 2


def test_clear_otel_spans(store):
    store.save_otel_span(
        {
            "trace_id": "t1",
            "span_id": "s1",
            "operation": "store",
            "timestamp": 1.0,
            "duration_ms": 1.0,
            "status": "ok",
            "service_name": "memctrl",
        }
    )
    store.clear_otel_spans()
    assert store.get_otel_spans() == []


# ---------------------------------------------------------------------------
# Stats include provenance and spans
# ---------------------------------------------------------------------------


def test_stats_include_new_tables(store):
    store.save_provenance({"query": "q", "sources": []})
    store.save_otel_span(
        {
            "trace_id": "t1",
            "span_id": "s1",
            "operation": "store",
            "timestamp": 1.0,
            "duration_ms": 1.0,
            "status": "ok",
            "service_name": "memctrl",
        }
    )
    s = store.stats()
    assert "provenance_records" in s
    assert "otel_spans" in s
    assert s["provenance_records"] == 1
    assert s["otel_spans"] == 1


def test_concurrent_writes_dont_crash(store):
    """Multiple threads writing simultaneously should not raise unhandled
    OperationalError thanks to exponential backoff retry."""
    import threading

    errors = []
    ids = []

    def worker(i):
        try:
            mid = store.insert_memory("session", f"thread {i}", "test")
            ids.append(mid)
        except Exception as exc:
            errors.append(exc)

    threads = [threading.Thread(target=worker, args=(i,)) for i in range(20)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    assert len(errors) == 0, f"Concurrent writes raised errors: {errors}"
    assert len(ids) == 20
    mems = store.list_memories()
    assert len(mems) == 20


# ---------------------------------------------------------------------------
# Tree persistence
# ---------------------------------------------------------------------------


def test_rebuild_tree_atomic_roundtrip(store):
    """Persist a full tree hierarchy and reload it intact."""
    root = TreeNode(
        id="root",
        title="Root",
        layer="root",
        summary="root",
        children=[
            TreeNode(
                id="layer_project",
                title="Project",
                layer="project",
                summary="p",
                memory_ids=["m1", "m2"],
                children=[
                    TreeNode(
                        id="mem_m1",
                        title="m1",
                        layer="project",
                        summary="s",
                        memory_ids=["m1"],
                    ),
                    TreeNode(
                        id="mem_m2",
                        title="m2",
                        layer="project",
                        summary="s",
                        memory_ids=["m2"],
                    ),
                ],
            ),
        ],
    )
    store.rebuild_tree_atomic([root])
    loaded = store.build_tree_from_nodes()
    assert loaded is not None
    assert loaded.id == "root"
    assert len(loaded.children) == 1
    assert loaded.children[0].id == "layer_project"
    assert set(loaded.all_memory_ids()) == {"m1", "m2"}


def test_init_db_retry(store):
    """_init_db retries on 'database is locked' and eventually succeeds."""
    import sqlite3
    from unittest.mock import patch

    executescript_calls = 0

    class ConnectionWrapper:
        """Wraps a real sqlite3 connection and intercepts executescript."""

        def __init__(self, real_conn):
            self._real = real_conn

        def executescript(self, script):
            nonlocal executescript_calls
            executescript_calls += 1
            if executescript_calls == 1:
                raise sqlite3.OperationalError("database is locked")
            return self._real.executescript(script)

        def __getattr__(self, name):
            return getattr(self._real, name)

    original_connect = sqlite3.connect

    def mock_connect(path, **kwargs):
        real_conn = original_connect(path, **kwargs)
        return ConnectionWrapper(real_conn)

    with patch("memctrl.store.sqlite3.connect", side_effect=mock_connect):
        new_store = MemoryStore(store.db_path)
        # _init_db should have retried at least once
        assert executescript_calls >= 2
        new_store.close()
