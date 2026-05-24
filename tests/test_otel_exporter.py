"""Tests for MemoryOTelExporter -- OpenTelemetry GenAI memory span exporter.

Covers: span creation, all CRUD operations, search, stats aggregation,
JSON export, OTLP export, store proxy wrapper, validation, error handling,
and thread safety.
"""

import json
import os
import tempfile
import threading
import time

import pytest

from memctrl.otel_exporter import (
    OTEL_GEN_AI_MEMORY_CONFIDENCE,
    OTEL_GEN_AI_MEMORY_ID,
    OTEL_GEN_AI_MEMORY_LAYER,
    OTEL_GEN_AI_MEMORY_QUERY,
    OTEL_GEN_AI_MEMORY_RESULTS_COUNT,
    OTEL_GEN_AI_MEMORY_TOP_K,
    OTEL_GEN_AI_OPERATION,
    OTEL_GEN_AI_SYSTEM,
    MemoryOTelExporter,
    MemorySpan,
    _TracedStore,
    _to_otel_value,
)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
def exporter():
    """Create a fresh exporter for each test."""
    exp = MemoryOTelExporter(service_name="test-memctrl")
    exp.start()
    yield exp
    exp.stop()
    exp.clear()


@pytest.fixture
def temp_file():
    """Provide a temporary file path."""
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as f:
        path = f.name
    yield path
    if os.path.exists(path):
        os.unlink(path)


# ---------------------------------------------------------------------------
# MemorySpan dataclass
# ---------------------------------------------------------------------------


class TestMemorySpan:
    """Tests for the MemorySpan dataclass and its methods."""

    def test_span_creation_defaults(self):
        span = MemorySpan(
            span_id="span-1",
            trace_id="trace-1",
            operation="store",
            timestamp=time.time(),
            duration_ms=1.5,
        )
        assert span.span_id == "span-1"
        assert span.trace_id == "trace-1"
        assert span.operation == "store"
        assert span.status == "ok"
        assert span.memory_id is None
        assert span.layer is None

    def test_span_creation_full(self):
        now = time.time()
        span = MemorySpan(
            span_id="s1",
            trace_id="t1",
            operation="search",
            timestamp=now,
            duration_ms=10.0,
            memory_id="mem-1",
            layer="project",
            memory_type="semantic",
            confidence=0.85,
            query="fastapi testing",
            top_k=5,
            results_count=3,
            status="ok",
            error_message=None,
            attributes={"custom": "value"},
            service_name="my-service",
        )
        assert span.operation == "search"
        assert span.layer == "project"
        assert span.memory_type == "semantic"
        assert span.confidence == 0.85
        assert span.query == "fastapi testing"
        assert span.top_k == 5
        assert span.results_count == 3
        assert span.attributes == {"custom": "value"}
        assert span.service_name == "my-service"

    def test_invalid_operation_raises(self):
        with pytest.raises(ValueError, match="Invalid operation"):
            MemorySpan(
                span_id="s1",
                trace_id="t1",
                operation="invalid_op",
                timestamp=time.time(),
                duration_ms=1.0,
            )

    def test_invalid_layer_raises(self):
        with pytest.raises(ValueError, match="Invalid layer"):
            MemorySpan(
                span_id="s1",
                trace_id="t1",
                operation="store",
                timestamp=time.time(),
                duration_ms=1.0,
                layer="bogus_layer",
            )

    def test_invalid_memory_type_raises(self):
        with pytest.raises(ValueError, match="Invalid memory_type"):
            MemorySpan(
                span_id="s1",
                trace_id="t1",
                operation="store",
                timestamp=time.time(),
                duration_ms=1.0,
                memory_type="short_term",
            )

    def test_invalid_status_raises(self):
        with pytest.raises(ValueError, match="Invalid status"):
            MemorySpan(
                span_id="s1",
                trace_id="t1",
                operation="store",
                timestamp=time.time(),
                duration_ms=1.0,
                status="failed",
            )

    def test_confidence_out_of_range_raises(self):
        with pytest.raises(ValueError, match="confidence must be between"):
            MemorySpan(
                span_id="s1",
                trace_id="t1",
                operation="store",
                timestamp=time.time(),
                duration_ms=1.0,
                confidence=1.5,
            )

    def test_confidence_negative_raises(self):
        with pytest.raises(ValueError, match="confidence must be between"):
            MemorySpan(
                span_id="s1",
                trace_id="t1",
                operation="store",
                timestamp=time.time(),
                duration_ms=1.0,
                confidence=-0.1,
            )

    def test_valid_layers(self):
        for layer in ("project", "session", "user"):
            span = MemorySpan(
                span_id="s1",
                trace_id="t1",
                operation="store",
                timestamp=time.time(),
                duration_ms=1.0,
                layer=layer,
            )
            assert span.layer == layer

    def test_valid_memory_types(self):
        for mt in ("episodic", "semantic", "procedural"):
            span = MemorySpan(
                span_id="s1",
                trace_id="t1",
                operation="store",
                timestamp=time.time(),
                duration_ms=1.0,
                memory_type=mt,
            )
            assert span.memory_type == mt

    def test_valid_operations(self):
        for op in ("store", "retrieve", "search", "update", "delete"):
            span = MemorySpan(
                span_id="s1",
                trace_id="t1",
                operation=op,
                timestamp=time.time(),
                duration_ms=1.0,
            )
            assert span.operation == op

    def test_to_dict_roundtrip(self):
        span = MemorySpan(
            span_id="s1",
            trace_id="t1",
            operation="store",
            timestamp=1234567890.123,
            duration_ms=5.5,
            memory_id="mem-1",
            layer="project",
            confidence=0.9,
            status="ok",
            attributes={"key": "val"},
        )
        d = span.to_dict()
        assert d["span_id"] == "s1"
        assert d["trace_id"] == "t1"
        assert d["operation"] == "store"
        assert d["timestamp"] == 1234567890.123
        assert d["duration_ms"] == 5.5
        assert d["memory_id"] == "mem-1"
        assert d["layer"] == "project"
        assert d["confidence"] == 0.9
        assert d["status"] == "ok"
        assert d["attributes"] == {"key": "val"}

    def test_from_dict_roundtrip(self):
        span1 = MemorySpan(
            span_id="s1",
            trace_id="t1",
            operation="retrieve",
            timestamp=time.time(),
            duration_ms=3.0,
            memory_id="mem-1",
            layer="session",
            query="test query",
            attributes={"a": 1},
        )
        d = span1.to_dict()
        span2 = MemorySpan.from_dict(d)
        assert span2.span_id == span1.span_id
        assert span2.operation == span1.operation
        assert span2.layer == span1.layer
        assert span2.query == span1.query
        assert span2.attributes == span1.attributes

    def test_to_otel_dict_structure(self):
        span = MemorySpan(
            span_id="s1",
            trace_id="t1",
            operation="store",
            timestamp=1000.0,
            duration_ms=5.0,
            memory_id="mem-1",
            layer="project",
            confidence=0.95,
            status="ok",
        )
        otel = span.to_otel_dict()
        assert otel["traceId"] == "t1"
        assert otel["spanId"] == "s1"
        assert otel["parentSpanId"] is None
        assert otel["name"] == "gen_ai.memory.store"
        assert otel["kind"] == 1  # SPAN_KIND_INTERNAL
        assert otel["status"]["code"] == 1  # OK
        attrs = {a["key"]: a["value"] for a in otel["attributes"]}
        assert attrs[OTEL_GEN_AI_SYSTEM]["stringValue"] == "memctrl"
        assert attrs[OTEL_GEN_AI_OPERATION]["stringValue"] == "store"
        assert attrs[OTEL_GEN_AI_MEMORY_ID]["stringValue"] == "mem-1"
        assert attrs[OTEL_GEN_AI_MEMORY_LAYER]["stringValue"] == "project"
        assert attrs[OTEL_GEN_AI_MEMORY_CONFIDENCE]["doubleValue"] == 0.95

    def test_to_otel_dict_error_status(self):
        span = MemorySpan(
            span_id="s1",
            trace_id="t1",
            operation="retrieve",
            timestamp=time.time(),
            duration_ms=2.0,
            status="error",
            error_message="connection timeout",
        )
        otel = span.to_otel_dict()
        assert otel["status"]["code"] == 2  # ERROR

    def test_to_otel_dict_search_attributes(self):
        span = MemorySpan(
            span_id="s1",
            trace_id="t1",
            operation="search",
            timestamp=time.time(),
            duration_ms=15.0,
            query="fastapi middleware",
            top_k=10,
            results_count=5,
            layer="session",
        )
        otel = span.to_otel_dict()
        attrs = {a["key"]: a["value"] for a in otel["attributes"]}
        assert attrs[OTEL_GEN_AI_MEMORY_QUERY]["stringValue"] == "fastapi middleware"
        assert attrs[OTEL_GEN_AI_MEMORY_TOP_K]["intValue"] == "10"
        assert attrs[OTEL_GEN_AI_MEMORY_RESULTS_COUNT]["intValue"] == "5"

    def test_to_otel_timestamps(self):
        span = MemorySpan(
            span_id="s1",
            trace_id="t1",
            operation="store",
            timestamp=1.0,
            duration_ms=100.0,
        )
        otel = span.to_otel_dict()
        assert otel["startTimeUnixNano"] == "1000000000"
        assert otel["endTimeUnixNano"] == "1100000000"


# ---------------------------------------------------------------------------
# _to_otel_value helper
# ---------------------------------------------------------------------------


class TestToOtelValue:
    def test_string_value(self):
        assert _to_otel_value("hello") == {"stringValue": "hello"}

    def test_int_value(self):
        assert _to_otel_value(42) == {"intValue": "42"}

    def test_float_value(self):
        assert _to_otel_value(3.14) == {"doubleValue": 3.14}

    def test_bool_value(self):
        assert _to_otel_value(True) == {"boolValue": True}
        assert _to_otel_value(False) == {"boolValue": False}

    def test_list_value(self):
        assert _to_otel_value([1, 2]) == {
            "arrayValue": {"values": [{"intValue": "1"}, {"intValue": "2"}]}
        }


# ---------------------------------------------------------------------------
# MemoryOTelExporter -- lifecycle
# ---------------------------------------------------------------------------


class TestExporterLifecycle:
    def test_start_sets_active(self, exporter):
        assert exporter.is_active is True

    def test_stop_sets_inactive(self, exporter):
        exporter.stop()
        assert exporter.is_active is False

    def test_start_generates_new_trace_id(self, exporter):
        old_id = exporter._trace_id
        exporter.stop()
        exporter.start()
        assert exporter._trace_id != old_id
        assert len(exporter._trace_id) == 36  # UUID hex format

    def test_context_manager(self):
        with MemoryOTelExporter() as exp:
            assert exp.is_active is True
        assert exp.is_active is False


# ---------------------------------------------------------------------------
# MemoryOTelExporter -- recording spans
# ---------------------------------------------------------------------------


class TestRecordStore:
    def test_record_store_basic(self, exporter):
        span = exporter.record_store(
            memory_id="mem-1",
            layer="project",
            content="we use FastAPI",
            confidence=1.0,
            duration_ms=5.0,
        )
        assert span.operation == "store"
        assert span.memory_id == "mem-1"
        assert span.layer == "project"
        assert span.confidence == 1.0
        assert span.status == "ok"
        assert span.attributes["content"] == "we use FastAPI"

    def test_record_store_error(self, exporter):
        span = exporter.record_store(
            memory_id=None,
            layer="session",
            content="failed insert",
            confidence=0.5,
            duration_ms=2.0,
            status="error",
            error_message="DB locked",
        )
        assert span.status == "error"
        assert span.error_message == "DB locked"

    def test_record_store_no_external_deps(self):
        """Verify the exporter works without opentelemetry installed."""
        exp = MemoryOTelExporter(service_name="no-otel")
        exp.start()
        span = exp.record_store(
            memory_id="m1",
            layer="project",
            content="test",
            confidence=0.9,
            duration_ms=1.0,
        )
        assert span is not None
        assert span.operation == "store"
        exp.stop()


class TestRecordRetrieve:
    def test_record_retrieve_basic(self, exporter):
        span = exporter.record_retrieve(
            memory_id="mem-1",
            layer="project",
            duration_ms=3.0,
        )
        assert span.operation == "retrieve"
        assert span.memory_id == "mem-1"
        assert span.layer == "project"
        assert span.query is None

    def test_record_retrieve_with_query(self, exporter):
        span = exporter.record_retrieve(
            memory_id="mem-1",
            layer="session",
            duration_ms=2.5,
            query="how do we test?",
        )
        assert span.query == "how do we test?"

    def test_record_retrieve_error(self, exporter):
        span = exporter.record_retrieve(
            memory_id="mem-x",
            layer="user",
            duration_ms=1.0,
            status="error",
            error_message="not found",
        )
        assert span.status == "error"
        assert span.error_message == "not found"


class TestRecordSearch:
    def test_record_search_basic(self, exporter):
        span = exporter.record_search(
            query="fastapi middleware",
            top_k=10,
            results_count=5,
            layer="project",
            duration_ms=25.0,
        )
        assert span.operation == "search"
        assert span.query == "fastapi middleware"
        assert span.top_k == 10
        assert span.results_count == 5
        assert span.layer == "project"

    def test_record_search_no_layer(self, exporter):
        span = exporter.record_search(
            query="test",
            top_k=5,
            results_count=0,
            layer=None,
            duration_ms=10.0,
        )
        assert span.layer is None
        assert span.results_count == 0

    def test_record_search_error(self, exporter):
        span = exporter.record_search(
            query="test",
            top_k=5,
            results_count=0,
            layer="session",
            duration_ms=100.0,
            status="error",
            error_message="timeout",
        )
        assert span.status == "error"
        assert span.duration_ms == 100.0


class TestRecordUpdate:
    def test_record_update_basic(self, exporter):
        span = exporter.record_update(
            memory_id="mem-1",
            layer="project",
            duration_ms=2.0,
            update_type="layer",
        )
        assert span.operation == "update"
        assert span.memory_id == "mem-1"
        assert span.attributes["update_type"] == "layer"

    def test_record_update_confidence(self, exporter):
        span = exporter.record_update(
            memory_id="mem-1",
            layer="session",
            duration_ms=1.5,
            update_type="confidence",
            new_confidence=0.8,
        )
        assert span.attributes["update_type"] == "confidence"
        assert span.attributes["new_confidence"] == 0.8

    def test_record_update_error(self, exporter):
        span = exporter.record_update(
            memory_id="mem-x",
            layer="project",
            duration_ms=0.5,
            status="error",
            error_message="concurrent modification",
        )
        assert span.status == "error"
        assert span.error_message == "concurrent modification"


class TestRecordDelete:
    def test_record_delete_basic(self, exporter):
        span = exporter.record_delete(
            memory_id="mem-1",
            layer="session",
            duration_ms=1.0,
        )
        assert span.operation == "delete"
        assert span.memory_id == "mem-1"
        assert span.layer == "session"
        assert span.status == "ok"

    def test_record_delete_error(self, exporter):
        span = exporter.record_delete(
            memory_id="mem-x",
            layer="project",
            duration_ms=0.5,
            status="error",
            error_message="FK constraint",
        )
        assert span.status == "error"
        assert span.error_message == "FK constraint"


# ---------------------------------------------------------------------------
# MemoryOTelExporter -- span queries
# ---------------------------------------------------------------------------


class TestGetSpans:
    def test_get_spans_empty(self, exporter):
        assert exporter.get_spans() == []

    def test_get_spans_returns_all(self, exporter):
        exporter.record_store("m1", "project", "content", 1.0, 1.0)
        exporter.record_retrieve("m2", "session", 2.0)
        exporter.record_delete("m3", "user", 0.5)
        spans = exporter.get_spans()
        assert len(spans) == 3

    def test_get_spans_is_snapshot(self, exporter):
        exporter.record_store("m1", "project", "c", 1.0, 1.0)
        spans1 = exporter.get_spans()
        exporter.record_retrieve("m2", "session", 2.0)
        spans2 = exporter.get_spans()
        assert len(spans1) == 1
        assert len(spans2) == 2

    def test_get_spans_by_operation(self, exporter):
        exporter.record_store("m1", "project", "c", 1.0, 1.0)
        exporter.record_store("m2", "session", "c", 1.0, 1.0)
        exporter.record_retrieve("m3", "project", 2.0)
        store_spans = exporter.get_spans_by_operation("store")
        retrieve_spans = exporter.get_spans_by_operation("retrieve")
        assert len(store_spans) == 2
        assert len(retrieve_spans) == 1
        for s in store_spans:
            assert s.operation == "store"

    def test_get_spans_by_operation_invalid(self, exporter):
        with pytest.raises(ValueError, match="Invalid operation"):
            exporter.get_spans_by_operation("bogus")

    def test_get_spans_by_operation_all_valid_types(self, exporter):
        exporter.record_store("m1", "project", "c", 1.0, 1.0)
        exporter.record_retrieve("m2", "session", 2.0)
        exporter.record_search("q", 5, 3, "project", 10.0)
        exporter.record_update("m3", "user", 1.0)
        exporter.record_delete("m4", "project", 0.5)
        for op in ("store", "retrieve", "search", "update", "delete"):
            spans = exporter.get_spans_by_operation(op)
            assert len(spans) == 1
            assert spans[0].operation == op


# ---------------------------------------------------------------------------
# MemoryOTelExporter -- statistics
# ---------------------------------------------------------------------------


class TestGetStats:
    def test_stats_empty(self, exporter):
        stats = exporter.get_stats()
        assert stats["total_spans"] == 0
        assert stats["by_operation"] == {}
        assert stats["total_duration_ms"] == 0.0
        assert stats["avg_duration_ms"] == 0.0
        assert stats["error_count"] == 0
        assert stats["error_rate"] == 0.0
        assert stats["by_layer"] == {}

    def test_stats_basic(self, exporter):
        exporter.record_store("m1", "project", "c", 1.0, 10.0)
        exporter.record_retrieve("m2", "session", 5.0)
        exporter.record_search("q", 5, 3, "project", 20.0)
        stats = exporter.get_stats()
        assert stats["total_spans"] == 3
        assert stats["by_operation"] == {"store": 1, "retrieve": 1, "search": 1}
        assert stats["total_duration_ms"] == 35.0
        assert stats["avg_duration_ms"] == 11.667
        assert stats["error_count"] == 0
        assert stats["error_rate"] == 0.0
        assert stats["by_layer"] == {"project": 2, "session": 1}

    def test_stats_with_errors(self, exporter):
        exporter.record_store("m1", "project", "c", 1.0, 1.0)
        exporter.record_store(
            "m2", "session", "c", 1.0, 2.0, status="error", error_message="fail"
        )
        exporter.record_retrieve("m3", "project", 3.0)
        stats = exporter.get_stats()
        assert stats["total_spans"] == 3
        assert stats["error_count"] == 1
        assert stats["error_rate"] == pytest.approx(0.3333, abs=0.001)

    def test_stats_multiple_same_operation(self, exporter):
        exporter.record_store("m1", "project", "c", 1.0, 1.0)
        exporter.record_store("m2", "project", "c", 1.0, 2.0)
        exporter.record_store("m3", "session", "c", 1.0, 3.0)
        stats = exporter.get_stats()
        assert stats["by_operation"] == {"store": 3}
        assert stats["total_duration_ms"] == 6.0
        assert stats["avg_duration_ms"] == 2.0


# ---------------------------------------------------------------------------
# MemoryOTelExporter -- JSON export
# ---------------------------------------------------------------------------


class TestExportJson:
    def test_export_json_creates_file(self, exporter, temp_file):
        exporter.record_store("m1", "project", "test content", 1.0, 5.0)
        exporter.export_json(temp_file)
        assert os.path.exists(temp_file)

    def test_export_json_valid_json(self, exporter, temp_file):
        exporter.record_store("m1", "project", "content", 1.0, 5.0)
        exporter.export_json(temp_file)
        with open(temp_file, "r") as f:
            data = json.load(f)
        assert data["service_name"] == "test-memctrl"
        assert data["span_count"] == 1
        assert "trace_id" in data
        assert "exported_at" in data
        assert len(data["spans"]) == 1
        assert data["spans"][0]["operation"] == "store"

    def test_export_json_multiple_spans(self, exporter, temp_file):
        exporter.record_store("m1", "project", "c1", 1.0, 1.0)
        exporter.record_retrieve("m2", "session", 2.0)
        exporter.record_search("q", 5, 3, "project", 10.0)
        exporter.export_json(temp_file)
        with open(temp_file, "r") as f:
            data = json.load(f)
        assert data["span_count"] == 3

    def test_export_json_empty(self, exporter, temp_file):
        exporter.export_json(temp_file)
        with open(temp_file, "r") as f:
            data = json.load(f)
        assert data["span_count"] == 0
        assert data["spans"] == []


# ---------------------------------------------------------------------------
# MemoryOTelExporter -- OTLP JSON export
# ---------------------------------------------------------------------------


class TestExportOtlpJson:
    def test_export_otlp_structure(self, exporter, temp_file):
        exporter.record_store("m1", "project", "content", 1.0, 5.0)
        exporter.export_otlp_json(temp_file)
        with open(temp_file, "r") as f:
            data = json.load(f)
        assert "resourceSpans" in data
        assert len(data["resourceSpans"]) == 1
        resource = data["resourceSpans"][0]["resource"]
        attrs = {a["key"]: a["value"] for a in resource["attributes"]}
        assert attrs["service.name"]["stringValue"] == "test-memctrl"
        assert attrs["telemetry.sdk.name"]["stringValue"] == "memctrl"
        assert attrs["telemetry.sdk.language"]["stringValue"] == "python"

    def test_export_otlp_scope_spans(self, exporter, temp_file):
        exporter.record_store("m1", "project", "content", 1.0, 5.0)
        exporter.record_retrieve("m2", "session", 2.0)
        exporter.export_otlp_json(temp_file)
        with open(temp_file, "r") as f:
            data = json.load(f)
        scope_spans = data["resourceSpans"][0]["scopeSpans"]
        assert len(scope_spans) == 1
        assert scope_spans[0]["scope"]["name"] == "memctrl"
        assert scope_spans[0]["scope"]["version"] == "1.1.0"
        assert len(scope_spans[0]["spans"]) == 2

    def test_export_otlp_span_format(self, exporter, temp_file):
        exporter.record_store(
            "m1", "project", "content", 0.95, 5.0, memory_type="semantic"
        )
        exporter.export_otlp_json(temp_file)
        with open(temp_file, "r") as f:
            data = json.load(f)
        span = data["resourceSpans"][0]["scopeSpans"][0]["spans"][0]
        assert "traceId" in span
        assert "spanId" in span
        assert span["name"] == "gen_ai.memory.store"
        assert span["kind"] == 1
        assert "startTimeUnixNano" in span
        assert "endTimeUnixNano" in span
        assert "attributes" in span
        assert "status" in span
        attrs = {a["key"]: a["value"] for a in span["attributes"]}
        assert OTEL_GEN_AI_SYSTEM in attrs
        assert OTEL_GEN_AI_OPERATION in attrs

    def test_export_otlp_error_span(self, exporter, temp_file):
        exporter.record_retrieve(
            "m1", "project", 2.0, status="error", error_message="timeout"
        )
        exporter.export_otlp_json(temp_file)
        with open(temp_file, "r") as f:
            data = json.load(f)
        span = data["resourceSpans"][0]["scopeSpans"][0]["spans"][0]
        assert span["status"]["code"] == 2

    def test_export_otlp_empty(self, exporter, temp_file):
        exporter.export_otlp_json(temp_file)
        with open(temp_file, "r") as f:
            data = json.load(f)
        assert len(data["resourceSpans"][0]["scopeSpans"][0]["spans"]) == 0


# ---------------------------------------------------------------------------
# MemoryOTelExporter -- clear
# ---------------------------------------------------------------------------


class TestClear:
    def test_clear_removes_spans(self, exporter):
        exporter.record_store("m1", "project", "c", 1.0, 1.0)
        assert len(exporter.get_spans()) == 1
        exporter.clear()
        assert len(exporter.get_spans()) == 0

    def test_clear_generates_new_trace_id(self, exporter):
        old_id = exporter._trace_id
        exporter.record_store("m1", "project", "c", 1.0, 1.0)
        exporter.clear()
        assert exporter._trace_id != old_id


# ---------------------------------------------------------------------------
# MemoryOTelExporter -- inactivity after stop
# ---------------------------------------------------------------------------


class TestInactivityAfterStop:
    def test_record_after_stop_ignored(self, exporter):
        exporter.record_store("m1", "project", "c", 1.0, 1.0)
        assert len(exporter.get_spans()) == 1
        exporter.stop()
        exporter.record_retrieve("m2", "session", 2.0)
        # Span should not be added
        assert len(exporter.get_spans()) == 1


# ---------------------------------------------------------------------------
# MemoryOTelExporter -- traced store proxy
# ---------------------------------------------------------------------------


class TestTracedStore:
    def test_trace_store_returns_proxy(self, exporter):
        # Create a minimal mock store
        class MockStore:
            def insert_memory(self, layer, content, source="test"):
                return "mock-id-123"

        store = MockStore()
        traced = exporter.trace_store(store)
        assert isinstance(traced, _TracedStore)

    def test_traced_store_insert_memory(self, exporter):
        class MockStore:
            def insert_memory(self, layer, content, source="test"):
                return "mock-id-123"

        store = MockStore()
        traced = exporter.trace_store(store)
        mid = traced.insert_memory("project", "we use FastAPI", "test")
        assert mid == "mock-id-123"
        spans = exporter.get_spans()
        assert len(spans) >= 1
        store_spans = exporter.get_spans_by_operation("store")
        assert len(store_spans) == 1

    def test_traced_store_get_memory(self, exporter):
        class MockMem:
            def __init__(self):
                self.id = "m1"
                self.layer = "project"

        class MockStore:
            def get_memory(self, id):
                return MockMem()

        store = MockStore()
        traced = exporter.trace_store(store)
        result = traced.get_memory("m1")
        assert result.id == "m1"
        retrieve_spans = exporter.get_spans_by_operation("retrieve")
        assert len(retrieve_spans) == 1
        assert retrieve_spans[0].memory_id == "m1"

    def test_traced_store_delete_memory(self, exporter):
        class MockStore:
            def delete_memory(self, id):
                return True

        store = MockStore()
        traced = exporter.trace_store(store)
        result = traced.delete_memory("m1")
        assert result is True
        delete_spans = exporter.get_spans_by_operation("delete")
        assert len(delete_spans) == 1
        assert delete_spans[0].memory_id == "m1"

    def test_traced_store_list_memories(self, exporter):
        class MockMem:
            def __init__(self, id, layer):
                self.id = id
                self.layer = layer

        class MockStore:
            def list_memories(self, layer=None):
                return [MockMem("m1", "project"), MockMem("m2", "project")]

        store = MockStore()
        traced = exporter.trace_store(store)
        result = traced.list_memories("project")
        assert len(result) == 2
        search_spans = exporter.get_spans_by_operation("search")
        assert len(search_spans) == 1
        assert search_spans[0].results_count == 2

    def test_traced_store_update_memory_layer(self, exporter):
        class MockStore:
            def update_memory_layer(self, id, new_layer):
                return True

        store = MockStore()
        traced = exporter.trace_store(store)
        result = traced.update_memory_layer("m1", "project")
        assert result is True
        update_spans = exporter.get_spans_by_operation("update")
        assert len(update_spans) == 1
        assert update_spans[0].attributes["update_type"] == "layer"

    def test_traced_store_update_memory_confidence(self, exporter):
        class MockStore:
            def update_memory_confidence(self, id, new_confidence):
                return True

        store = MockStore()
        traced = exporter.trace_store(store)
        result = traced.update_memory_confidence("m1", 0.8)
        assert result is True
        update_spans = exporter.get_spans_by_operation("update")
        assert len(update_spans) == 1
        assert update_spans[0].attributes["update_type"] == "confidence"
        assert update_spans[0].attributes["new_confidence"] == 0.8

    def test_traced_store_passes_through_non_crud(self, exporter):
        class MockStore:
            def __init__(self):
                self.some_attr = "hello"

            def custom_method(self):
                return 42

        store = MockStore()
        traced = exporter.trace_store(store)
        assert traced.some_attr == "hello"
        assert traced.custom_method() == 42


# ---------------------------------------------------------------------------
# MemoryOTelExporter -- thread safety
# ---------------------------------------------------------------------------


class TestThreadSafety:
    def test_concurrent_record_store(self, exporter):
        """Multiple threads recording spans simultaneously."""
        num_threads = 10
        spans_per_thread = 20

        def worker(tid):
            for i in range(spans_per_thread):
                exporter.record_store(
                    memory_id=f"m-{tid}-{i}",
                    layer="project" if i % 2 == 0 else "session",
                    content=f"content-{i}",
                    confidence=1.0,
                    duration_ms=1.0,
                )

        threads = [
            threading.Thread(target=worker, args=(t,)) for t in range(num_threads)
        ]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        spans = exporter.get_spans()
        assert len(spans) == num_threads * spans_per_thread

    def test_concurrent_mixed_operations(self, exporter):
        """Multiple threads with different operation types."""
        operations = [
            lambda: exporter.record_store("m1", "project", "c", 1.0, 1.0),
            lambda: exporter.record_retrieve("m2", "session", 2.0),
            lambda: exporter.record_search("q", 5, 3, "project", 10.0),
            lambda: exporter.record_update("m3", "user", 1.0),
            lambda: exporter.record_delete("m4", "project", 0.5),
        ]

        def worker(op_fn):
            for _ in range(10):
                op_fn()

        threads = [threading.Thread(target=worker, args=(op,)) for op in operations]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        spans = exporter.get_spans()
        assert len(spans) == 50
        stats = exporter.get_stats()
        assert stats["total_spans"] == 50
        assert all(count == 10 for count in stats["by_operation"].values())


# ---------------------------------------------------------------------------
# MemoryOTelExporter -- service_name customization
# ---------------------------------------------------------------------------


class TestServiceName:
    def test_default_service_name(self):
        exp = MemoryOTelExporter()
        assert exp.service_name == "memctrl"

    def test_custom_service_name(self):
        exp = MemoryOTelExporter(service_name="my-agent")
        assert exp.service_name == "my-agent"

    def test_service_name_in_span(self):
        exp = MemoryOTelExporter(service_name="custom-svc")
        exp.start()
        span = exp.record_store("m1", "project", "c", 1.0, 1.0)
        assert span.service_name == "custom-svc"
        exp.stop()

    def test_service_name_in_otel_dict(self):
        exp = MemoryOTelExporter(service_name="custom-svc")
        exp.start()
        span = exp.record_store("m1", "project", "c", 1.0, 1.0)
        otel = span.to_otel_dict()
        assert otel["serviceName"] == "custom-svc"
        exp.stop()

    def test_service_name_in_export_json(self, exporter, temp_file):
        exporter.export_json(temp_file)
        with open(temp_file, "r") as f:
            data = json.load(f)
        assert data["service_name"] == "test-memctrl"

    def test_service_name_in_export_otlp(self, exporter, temp_file):
        exporter.export_otlp_json(temp_file)
        with open(temp_file, "r") as f:
            data = json.load(f)
        attrs = {
            a["key"]: a["value"]
            for a in data["resourceSpans"][0]["resource"]["attributes"]
        }
        assert attrs["service.name"]["stringValue"] == "test-memctrl"


# ---------------------------------------------------------------------------
# MemoryOTelExporter -- no-op when not started
# ---------------------------------------------------------------------------


class TestNoOpNotStarted:
    def test_record_before_start_ignored(self):
        exp = MemoryOTelExporter()
        # Don't start
        exp.record_store("m1", "project", "c", 1.0, 1.0)
        assert len(exp.get_spans()) == 0

    def test_record_after_stop_ignored(self):
        exp = MemoryOTelExporter()
        exp.start()
        exp.record_store("m1", "project", "c", 1.0, 1.0)
        assert len(exp.get_spans()) == 1
        exp.stop()
        exp.record_store("m2", "session", "c", 1.0, 1.0)
        assert len(exp.get_spans()) == 1  # Second not recorded


# ---------------------------------------------------------------------------
# SQLite persistence
# ---------------------------------------------------------------------------


class TestSQLitePersistence:
    def test_spans_persist_to_sqlite(self):
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
            db_path = f.name
        exp = MemoryOTelExporter(db_path=db_path, max_spans=100)
        exp.start()
        exp.record_store("m1", "project", "we use FastAPI", 1.0, 5.0)
        exp.stop()

        # New exporter instance loads from same DB
        exp2 = MemoryOTelExporter(db_path=db_path, max_spans=100)
        exp2.start()
        spans = exp2.get_spans()
        assert len(spans) == 1
        assert spans[0].operation == "store"
        exp2.stop()
        os.unlink(db_path)

    def test_in_memory_bound_enforced(self):
        exp = MemoryOTelExporter(max_spans=3)
        exp.start()
        for i in range(5):
            exp.record_store(f"m{i}", "project", "c", 1.0, 1.0)
        spans = exp.get_spans()
        assert len(spans) == 3
        exp.stop()

    def test_sqlite_prune_oldest(self):
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
            db_path = f.name
        exp = MemoryOTelExporter(db_path=db_path, max_spans=2)
        exp.start()
        for i in range(5):
            exp.record_store(f"m{i}", "project", "c", 1.0, 1.0)
        exp.stop()

        exp2 = MemoryOTelExporter(db_path=db_path, max_spans=2)
        exp2.start()
        spans = exp2.get_spans()
        # Should only have the last 2 spans because pruning happens per-insert
        assert len(spans) <= 2
        exp2.stop()
        os.unlink(db_path)

    def test_clear_clears_sqlite(self):
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
            db_path = f.name
        exp = MemoryOTelExporter(db_path=db_path)
        exp.start()
        exp.record_store("m1", "project", "c", 1.0, 1.0)
        exp.clear()

        exp2 = MemoryOTelExporter(db_path=db_path)
        exp2.start()
        assert len(exp2.get_spans()) == 0
        exp2.stop()
        os.unlink(db_path)
