"""Tests for MemorySpan and SpanTracker.

Covers: span context manager, operation tracking, duration recording,
operation counts, layer access counts, multiple spans, completed span
accessibility, active span state, metadata, max completed limit,
and to_dict serialization.
"""

import time

import pytest

from memctrl.span import MemoryOperation, MemorySpan, SpanTracker


# ---------------------------------------------------------------------------
# MemoryOperation
# ---------------------------------------------------------------------------


def test_memory_operation_defaults():
    """Operation with only required field uses defaults for all others."""
    op = MemoryOperation(operation="store")
    assert op.operation == "store"
    assert op.memory_id is None
    assert op.layer is None
    assert op.content_preview is None
    assert op.confidence is None
    assert op.query is None
    assert op.timestamp == 0.0
    assert op.duration_ms == 0.0


def test_memory_operation_full():
    """Operation with all fields set."""
    op = MemoryOperation(
        operation="retrieve",
        memory_id="m1",
        layer="project",
        content_preview="auth impl",
        confidence=0.95,
        query="auth requirements",
        timestamp=1234.0,
        duration_ms=5.0,
    )
    assert op.operation == "retrieve"
    assert op.memory_id == "m1"
    assert op.layer == "project"
    assert op.content_preview == "auth impl"
    assert op.confidence == 0.95
    assert op.query == "auth requirements"
    assert op.timestamp == 1234.0
    assert op.duration_ms == 5.0


def test_memory_operation_to_dict():
    """to_dict serializes all fields correctly."""
    op = MemoryOperation(
        operation="store",
        memory_id="abc",
        layer="session",
        content_preview="test content",
        confidence=0.8,
        query="test query",
        timestamp=1000.0,
        duration_ms=2.5,
    )
    d = op.to_dict()
    assert d["operation"] == "store"
    assert d["memory_id"] == "abc"
    assert d["layer"] == "session"
    assert d["content_preview"] == "test content"
    assert d["confidence"] == 0.8
    assert d["query"] == "test query"
    assert d["timestamp"] == 1000.0
    assert d["duration_ms"] == 2.5


# ---------------------------------------------------------------------------
# MemorySpan
# ---------------------------------------------------------------------------


def test_memory_span_defaults():
    """Span with required fields uses defaults for optional."""
    span = MemorySpan(name="test", span_id="sid", start_time=1000.0)
    assert span.name == "test"
    assert span.span_id == "sid"
    assert span.start_time == 1000.0
    assert span.end_time is None
    assert span.operations == []
    assert span.metadata == {}


def test_memory_span_add_operation():
    """add_operation appends to the operations list."""
    span = MemorySpan(name="test", span_id="sid", start_time=1000.0)
    op1 = MemoryOperation(operation="store")
    op2 = MemoryOperation(operation="retrieve")
    span.add_operation(op1)
    span.add_operation(op2)
    assert len(span.operations) == 2
    assert span.operations[0] is op1
    assert span.operations[1] is op2


def test_memory_span_operation_counts():
    """operation_counts returns correct per-operation tallies."""
    span = MemorySpan(name="test", span_id="sid", start_time=1000.0)
    span.add_operation(MemoryOperation(operation="store"))
    span.add_operation(MemoryOperation(operation="store"))
    span.add_operation(MemoryOperation(operation="retrieve"))
    span.add_operation(MemoryOperation(operation="delete"))
    counts = span.operation_counts
    assert counts == {"store": 2, "retrieve": 1, "delete": 1}


def test_memory_span_operation_counts_empty():
    """operation_counts on a span with no operations returns empty dict."""
    span = MemorySpan(name="test", span_id="sid", start_time=1000.0)
    assert span.operation_counts == {}


def test_memory_span_layer_access_counts():
    """layer_access_counts returns correct per-layer tallies."""
    span = MemorySpan(name="test", span_id="sid", start_time=1000.0)
    span.add_operation(MemoryOperation(operation="store", layer="project"))
    span.add_operation(MemoryOperation(operation="store", layer="session"))
    span.add_operation(MemoryOperation(operation="retrieve", layer="project"))
    span.add_operation(MemoryOperation(operation="delete"))  # no layer
    counts = span.layer_access_counts
    assert counts == {"project": 2, "session": 1}


def test_memory_span_layer_access_counts_empty():
    """layer_access_counts on a span with no layer operations returns empty dict."""
    span = MemorySpan(name="test", span_id="sid", start_time=1000.0)
    assert span.layer_access_counts == {}


def test_memory_span_duration_completed():
    """duration_ms for a completed span returns end_time - start_time."""
    span = MemorySpan(name="test", span_id="sid", start_time=1000.0, end_time=1001.0)
    assert span.duration_ms == pytest.approx(1000.0, rel=1e-9)


def test_memory_span_duration_active():
    """duration_ms for an active (unended) span returns a positive value."""
    start = time.time()
    span = MemorySpan(name="test", span_id="sid", start_time=start)
    dur = span.duration_ms
    assert dur >= 0.0
    # Should be very small since we measure immediately after creation
    assert dur < 100.0  # less than 100ms


def test_memory_span_to_dict():
    """to_dict serializes all fields and computed properties."""
    span = MemorySpan(
        name="test_span",
        span_id="uuid-123",
        start_time=1000.0,
        end_time=1002.0,
        metadata={"agent": "dev1"},
    )
    span.add_operation(MemoryOperation(operation="store", layer="project"))
    d = span.to_dict()
    assert d["name"] == "test_span"
    assert d["span_id"] == "uuid-123"
    assert d["duration_ms"] == pytest.approx(2000.0, rel=1e-9)
    assert d["operation_counts"] == {"store": 1}
    assert d["layer_access_counts"] == {"project": 1}
    assert len(d["operations"]) == 1
    assert d["operations"][0]["operation"] == "store"
    assert d["metadata"] == {"agent": "dev1"}


# ---------------------------------------------------------------------------
# SpanTracker -- context manager
# ---------------------------------------------------------------------------


def test_tracker_span_records_operations():
    """Operations recorded inside the span context are attached to the span."""
    tracker = SpanTracker()
    with tracker.span("test_scope") as span:
        tracker.record_operation("store", memory_id="m1", layer="project")
        tracker.record_operation("retrieve", memory_id="m2", layer="session")
    assert len(span.operations) == 2
    assert span.operations[0].operation == "store"
    assert span.operations[0].memory_id == "m1"
    assert span.operations[0].layer == "project"
    assert span.operations[1].operation == "retrieve"
    assert span.operations[1].memory_id == "m2"
    assert span.operations[1].layer == "session"


def test_tracker_span_no_ops_is_valid():
    """A span with no recorded operations is valid."""
    tracker = SpanTracker()
    with tracker.span("empty_scope") as span:
        pass
    assert len(span.operations) == 0
    assert span.operation_counts == {}


def test_tracker_span_duration():
    """Span records meaningful duration."""
    tracker = SpanTracker()
    with tracker.span("duration_test") as span:
        time.sleep(0.01)  # 10ms
    assert span.duration_ms >= 10.0


def test_tracker_active_span_inside_context():
    """get_active_span returns the current span inside a context."""
    tracker = SpanTracker()
    with tracker.span("inside") as span:
        active = tracker.get_active_span()
        assert active is span
        assert active.name == "inside"


def test_tracker_active_span_outside_context():
    """get_active_span returns None outside any context."""
    tracker = SpanTracker()
    assert tracker.get_active_span() is None
    with tracker.span("test"):
        pass
    assert tracker.get_active_span() is None


def test_tracker_completed_spans_after_exit():
    """Completed spans are accessible after the context exits."""
    tracker = SpanTracker()
    with tracker.span("scope1"):
        tracker.record_operation("store")
    completed = tracker.get_completed_spans()
    assert len(completed) == 1
    assert completed[0].name == "scope1"
    assert len(completed[0].operations) == 1


def test_tracker_multiple_spans_tracked_independently():
    """Multiple spans are tracked independently with separate operations."""
    tracker = SpanTracker()
    with tracker.span("scope1"):
        tracker.record_operation("store", memory_id="m1")
    with tracker.span("scope2"):
        tracker.record_operation("retrieve", memory_id="m2")
    completed = tracker.get_completed_spans()
    assert len(completed) == 2
    assert completed[0].name == "scope1"
    assert len(completed[0].operations) == 1
    assert completed[0].operations[0].memory_id == "m1"
    assert completed[1].name == "scope2"
    assert len(completed[1].operations) == 1
    assert completed[1].operations[0].memory_id == "m2"


def test_tracker_metadata_stored():
    """Metadata passed to span() is stored on the MemorySpan."""
    tracker = SpanTracker()
    with tracker.span("meta_test", agent="dev1", task="auth") as span:
        pass
    assert span.metadata == {"agent": "dev1", "task": "auth"}
    completed = tracker.get_completed_spans()
    assert completed[0].metadata == {"agent": "dev1", "task": "auth"}


def test_tracker_get_span_by_name():
    """get_span_by_name finds the most recent matching span."""
    tracker = SpanTracker()
    with tracker.span("auth_debug"):
        tracker.record_operation("store")
    found = tracker.get_span_by_name("auth_debug")
    assert found is not None
    assert found.name == "auth_debug"


def test_tracker_get_span_by_name_not_found():
    """get_span_by_name returns None when no match exists."""
    tracker = SpanTracker()
    assert tracker.get_span_by_name("nonexistent") is None


def test_tracker_get_span_by_name_most_recent():
    """get_span_by_name returns the most recent span with matching name."""
    tracker = SpanTracker()
    with tracker.span("same_name"):
        tracker.record_operation("store")
    with tracker.span("same_name"):
        tracker.record_operation("retrieve")
    found = tracker.get_span_by_name("same_name")
    assert found is not None
    assert len(found.operations) == 1
    assert found.operations[0].operation == "retrieve"


def test_tracker_max_completed_limit():
    """Only max_completed most recent spans are retained."""
    tracker = SpanTracker(max_completed=3)
    for i in range(5):
        with tracker.span(f"span_{i}"):
            tracker.record_operation("store")
    completed = tracker.get_completed_spans()
    assert len(completed) == 3
    assert completed[0].name == "span_2"
    assert completed[1].name == "span_3"
    assert completed[2].name == "span_4"


def test_tracker_max_completed_default():
    """Default max_completed is 50."""
    tracker = SpanTracker()
    assert tracker._max_completed == 50


def test_tracker_record_operation_no_active_span():
    """record_operation silently ignores when no span is active."""
    tracker = SpanTracker()
    # Should not raise
    tracker.record_operation("store", memory_id="m1")
    assert tracker.get_completed_spans() == []


def test_tracker_stats_empty():
    """get_stats on a fresh tracker returns zeroes."""
    tracker = SpanTracker()
    stats = tracker.get_stats()
    assert stats["total_spans"] == 0
    assert stats["total_operations"] == 0
    assert stats["avg_operations_per_span"] == 0.0
    assert stats["total_duration_ms"] == 0.0


def test_tracker_stats_with_spans():
    """get_stats aggregates correctly across completed spans."""
    tracker = SpanTracker()
    with tracker.span("s1"):
        tracker.record_operation("store")
        tracker.record_operation("retrieve")
    with tracker.span("s2"):
        tracker.record_operation("store")
    stats = tracker.get_stats()
    assert stats["total_spans"] == 2
    assert stats["total_operations"] == 3
    assert stats["avg_operations_per_span"] == 1.5
    assert stats["total_duration_ms"] >= 0.0


def test_tracker_clear():
    """clear removes all completed and active span state."""
    tracker = SpanTracker()
    with tracker.span("to_clear"):
        tracker.record_operation("store")
    assert len(tracker.get_completed_spans()) == 1
    tracker.clear()
    assert tracker.get_completed_spans() == []
    assert tracker.get_active_span() is None


def test_tracker_nested_contexts_not_supported():
    """Nested spans: inner span becomes active, outer is still completed."""
    tracker = SpanTracker()
    with tracker.span("outer"):
        tracker.record_operation("store", memory_id="outer_op")
        with tracker.span("inner"):
            tracker.record_operation("retrieve", memory_id="inner_op")
            active = tracker.get_active_span()
            assert active is not None
            assert active.name == "inner"
        # After inner exits, active is None (outer was overwritten)
        assert tracker.get_active_span() is None
    # Both spans should be in completed
    completed = tracker.get_completed_spans()
    assert len(completed) == 2


# ---------------------------------------------------------------------------
# SpanTracker -- to_dict serialization
# ---------------------------------------------------------------------------


def test_tracker_span_to_dict_roundtrip():
    """A span's to_dict contains all expected keys and types."""
    tracker = SpanTracker()
    with tracker.span("roundtrip", env="test") as span:
        tracker.record_operation(
            "store",
            memory_id="m1",
            layer="project",
            content_preview="content",
            confidence=1.0,
        )
        tracker.record_operation(
            "retrieve",
            memory_id="m2",
            layer="session",
            query="auth",
        )
    d = span.to_dict()
    assert d["name"] == "roundtrip"
    assert isinstance(d["span_id"], str)
    assert isinstance(d["duration_ms"], float)
    assert d["metadata"] == {"env": "test"}
    assert d["operation_counts"] == {"store": 1, "retrieve": 1}
    assert d["layer_access_counts"] == {"project": 1, "session": 1}
    assert len(d["operations"]) == 2
    # Check each operation dict
    for op_dict in d["operations"]:
        assert "operation" in op_dict
        assert "timestamp" in op_dict


def test_memory_operation_to_dict_defaults():
    """to_dict on an operation with only defaults still works."""
    op = MemoryOperation(operation="delete")
    d = op.to_dict()
    assert d["operation"] == "delete"
    assert d["memory_id"] is None
    assert d["layer"] is None
    assert d["content_preview"] is None
    assert d["confidence"] is None
    assert d["query"] is None
    assert d["timestamp"] == 0.0
    assert d["duration_ms"] == 0.0


# ---------------------------------------------------------------------------
# SpanTracker -- operation_counts with all operation types
# ---------------------------------------------------------------------------


def test_tracker_all_operation_types():
    """All supported operation types are tracked and counted."""
    tracker = SpanTracker()
    with tracker.span("all_ops"):
        tracker.record_operation("store")
        tracker.record_operation("retrieve")
        tracker.record_operation("search")
        tracker.record_operation("update")
        tracker.record_operation("delete")
    span = tracker.get_completed_spans()[0]
    assert span.operation_counts == {
        "store": 1,
        "retrieve": 1,
        "search": 1,
        "update": 1,
        "delete": 1,
    }


# ---------------------------------------------------------------------------
# SpanTracker -- exception safety
# ---------------------------------------------------------------------------


def test_tracker_span_exception_safety():
    """Span is still completed even if an exception is raised inside."""
    tracker = SpanTracker()
    try:
        with tracker.span("failing"):
            tracker.record_operation("store")
            raise ValueError("boom")
    except ValueError:
        pass
    completed = tracker.get_completed_spans()
    assert len(completed) == 1
    assert completed[0].name == "failing"
    assert len(completed[0].operations) == 1
    assert completed[0].end_time is not None
    assert tracker.get_active_span() is None


# ---------------------------------------------------------------------------
# SpanTracker -- all kwargs forwarded to record_operation
# ---------------------------------------------------------------------------


def test_tracker_record_operation_all_kwargs():
    """record_operation forwards all kwargs to MemoryOperation."""
    tracker = SpanTracker()
    with tracker.span("kwarg_test"):
        tracker.record_operation(
            "store",
            memory_id="mid-1",
            layer="project",
            content_preview="short preview",
            confidence=0.85,
            query="my query",
        )
    span = tracker.get_completed_spans()[0]
    op = span.operations[0]
    assert op.memory_id == "mid-1"
    assert op.layer == "project"
    assert op.content_preview == "short preview"
    assert op.confidence == 0.85
    assert op.query == "my query"
    assert op.timestamp > 0.0  # auto-set by record_operation


# ---------------------------------------------------------------------------
# SpanTracker -- multiple operations same span
# ---------------------------------------------------------------------------


def test_tracker_many_operations_in_span():
    """A span can hold many operations."""
    tracker = SpanTracker()
    with tracker.span("many_ops"):
        for i in range(100):
            tracker.record_operation("store", memory_id=f"m{i}")
    span = tracker.get_completed_spans()[0]
    assert len(span.operations) == 100
    assert span.operation_counts == {"store": 100}


# ---------------------------------------------------------------------------
# SpanTracker -- duration_ms increases with time
# ---------------------------------------------------------------------------


def test_tracker_span_duration_increases_during_context():
    """duration_ms increases as time passes during the span."""
    tracker = SpanTracker()
    with tracker.span("duration_increases") as span:
        dur1 = span.duration_ms
        time.sleep(0.02)  # 20ms
        dur2 = span.duration_ms
    assert dur2 > dur1
    assert dur2 >= 20.0
