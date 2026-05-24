"""MemCtrl -- OpenTelemetry GenAI Memory Exporter.

Implements OpenTelemetry GenAI semantic conventions for memory operations:
- gen_ai.memory.store: Memory insertion
- gen_ai.memory.retrieve: Memory retrieval (single)
- gen_ai.memory.search: Memory search/query (multiple results)
- gen_ai.memory.update: Memory update (confidence, content, layer)
- gen_ai.memory.delete: Memory deletion

Exports memory operations as OTel spans that can be sent to:
- Datadog (native OTel GenAI support)
- Grafana/Jaeger (via OTLP)
- Honeycomb (direct OTLP ingestion)
- Any OTel-compatible backend

This positions MemCtrl as the first reference implementation for
agent memory observability standards.

Usage:
    from memctrl.otel_exporter import MemoryOTelExporter

    exporter = MemoryOTelExporter()
    exporter.start()

    # All memory operations are automatically traced
    store.insert_memory("project", "we use FastAPI", "test")

    # Get trace data
    spans = exporter.get_spans()
"""

from __future__ import annotations

import json
import sqlite3
import threading
import time
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional

# ---------------------------------------------------------------------------
# Optional opentelemetry integration -- works if installed, skips if not
# ---------------------------------------------------------------------------

_HAS_OTEL = False
try:
    from opentelemetry import trace
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.sdk.trace.export import (
        BatchSpanProcessor,
        InMemorySpanExporter as OtelSdkInMemoryExporter,
    )

    _HAS_OTEL = True
except ImportError:
    trace = None  # type: ignore[assignment]
    TracerProvider = None  # type: ignore[assignment, misc]
    BatchSpanProcessor = None  # type: ignore[assignment, misc]
    OtelSdkInMemoryExporter = None  # type: ignore[assignment, misc]


# ---------------------------------------------------------------------------
# Constants -- OTel semantic convention attribute names
# ---------------------------------------------------------------------------

OTEL_GEN_AI_SYSTEM = "gen_ai.system"
OTEL_GEN_AI_OPERATION = "gen_ai.operation.name"
OTEL_GEN_AI_MEMORY_OPERATION = "gen_ai.memory.operation"
OTEL_GEN_AI_MEMORY_ID = "gen_ai.memory.id"
OTEL_GEN_AI_MEMORY_LAYER = "gen_ai.memory.layer"
OTEL_GEN_AI_MEMORY_TYPE = "gen_ai.memory.type"
OTEL_GEN_AI_MEMORY_CONFIDENCE = "gen_ai.memory.confidence"
OTEL_GEN_AI_MEMORY_QUERY = "gen_ai.memory.query"
OTEL_GEN_AI_MEMORY_TOP_K = "gen_ai.memory.top_k"
OTEL_GEN_AI_MEMORY_RESULTS_COUNT = "gen_ai.memory.results_count"
OTEL_GEN_AI_MEMORY_BACKEND = "gen_ai.memory.backend"

VALID_OPERATIONS = {"store", "retrieve", "search", "update", "delete"}
VALID_LAYERS = {"project", "session", "user"}
VALID_MEMORY_TYPES = {"episodic", "semantic", "procedural"}
VALID_STATUS = {"ok", "error"}


# ---------------------------------------------------------------------------
# Data model
# ---------------------------------------------------------------------------


@dataclass
class MemorySpan:
    """A single memory operation span following OTel gen_ai.memory.* conventions.

    Maps to OpenTelemetry's proposed gen_ai.memory.* semantic conventions:
    - operation: store|retrieve|search|update|delete
    - memory_type: short_term|long_term|episodic|semantic|procedural
    - backend: The storage backend (sqlite, postgresql, etc.)
    - top_k: For search operations
    - min_score: Minimum relevance score for search
    - status: ok|error

    Attributes:
        span_id: Unique identifier for this span (UUID).
        trace_id: Trace this span belongs to (UUID).
        operation: Type of memory operation -- one of store, retrieve,
            search, update, delete.
        timestamp: Unix timestamp (seconds since epoch) when the
            operation started.
        duration_ms: Duration of the operation in milliseconds.
        memory_id: Identifier of the memory affected (if applicable).
        layer: Memory layer -- project, session, or user.
        memory_type: Classification -- episodic, semantic, or procedural.
        confidence: Confidence score between 0.0 and 1.0.
        query: Query string (for search operations).
        top_k: Number of top results requested (for search).
        results_count: Actual number of results returned (for search).
        status: Operation status -- "ok" or "error".
        error_message: Human-readable error description (when status is
            "error").
        attributes: Additional free-form attributes for extensibility.
        service_name: Name of the service producing this span.
    """

    span_id: str
    trace_id: str
    operation: str  # store|retrieve|search|update|delete
    timestamp: float  # Unix timestamp
    duration_ms: float
    memory_id: Optional[str] = None
    layer: Optional[str] = None  # project|session|user
    memory_type: Optional[str] = None  # episodic|semantic|procedural
    confidence: Optional[float] = None
    query: Optional[str] = None  # For search operations
    top_k: Optional[int] = None  # For search operations
    results_count: Optional[int] = None  # For search operations
    status: str = "ok"  # ok|error
    error_message: Optional[str] = None
    attributes: Dict[str, Any] = field(default_factory=dict)
    service_name: str = "memctrl"

    def __post_init__(self) -> None:
        """Validate fields after construction."""
        if self.operation not in VALID_OPERATIONS:
            raise ValueError(
                f"Invalid operation '{self.operation}'. "
                f"Must be one of: {VALID_OPERATIONS}"
            )
        if self.layer is not None and self.layer not in VALID_LAYERS:
            raise ValueError(
                f"Invalid layer '{self.layer}'. Must be one of: {VALID_LAYERS}"
            )
        if self.memory_type is not None and self.memory_type not in VALID_MEMORY_TYPES:
            raise ValueError(
                f"Invalid memory_type '{self.memory_type}'. "
                f"Must be one of: {VALID_MEMORY_TYPES}"
            )
        if self.status not in VALID_STATUS:
            raise ValueError(
                f"Invalid status '{self.status}'. Must be one of: {VALID_STATUS}"
            )
        if self.confidence is not None and not (0.0 <= self.confidence <= 1.0):
            raise ValueError(
                f"confidence must be between 0.0 and 1.0, got {self.confidence}"
            )

    def to_otel_dict(self) -> dict:
        """Convert to OTel-compatible span dictionary format.

        Produces a dictionary that mirrors the OpenTelemetry Protocol
        (OTLP) span structure for compatibility with collectors such as
        Datadog Agent, Grafana Tempo, Jaeger, and Honeycomb.

        Returns:
            Dictionary with keys matching OTel semantic conventions.
        """
        start_time_ns = int(self.timestamp * 1_000_000_000)
        duration_ns = int(self.duration_ms * 1_000_000)
        end_time_ns = start_time_ns + duration_ns

        attrs: Dict[str, Any] = {
            OTEL_GEN_AI_SYSTEM: "memctrl",
            OTEL_GEN_AI_OPERATION: self.operation,
        }
        if self.memory_id is not None:
            attrs[OTEL_GEN_AI_MEMORY_ID] = self.memory_id
        if self.layer is not None:
            attrs[OTEL_GEN_AI_MEMORY_LAYER] = self.layer
        if self.memory_type is not None:
            attrs[OTEL_GEN_AI_MEMORY_TYPE] = self.memory_type
        if self.confidence is not None:
            attrs[OTEL_GEN_AI_MEMORY_CONFIDENCE] = self.confidence
        if self.query is not None:
            attrs[OTEL_GEN_AI_MEMORY_QUERY] = self.query
        if self.top_k is not None:
            attrs[OTEL_GEN_AI_MEMORY_TOP_K] = self.top_k
        if self.results_count is not None:
            attrs[OTEL_GEN_AI_MEMORY_RESULTS_COUNT] = self.results_count

        # Merge custom attributes (they can override defaults)
        attrs.update(self.attributes)

        status_code = 1 if self.status == "ok" else 2  # OK=1, ERROR=2

        return {
            "traceId": self.trace_id,
            "spanId": self.span_id,
            "parentSpanId": None,
            "name": f"gen_ai.memory.{self.operation}",
            "kind": 1,  # SPAN_KIND_INTERNAL
            "startTimeUnixNano": str(start_time_ns),
            "endTimeUnixNano": str(end_time_ns),
            "attributes": [
                {"key": k, "value": _to_otel_value(v)} for k, v in attrs.items()
            ],
            "status": {
                "code": status_code,
                "message": self.error_message or "",
            },
            "serviceName": self.service_name,
        }

    def to_dict(self) -> dict:
        """Convert to plain dictionary for JSON serialization.

        This is a simpler, fully self-contained representation suitable
        for direct JSON export or debugging.  For OTLP-compatible
        output use :meth:`to_otel_dict`.

        Returns:
            Plain dictionary with all fields.
        """
        return {
            "span_id": self.span_id,
            "trace_id": self.trace_id,
            "operation": self.operation,
            "timestamp": self.timestamp,
            "duration_ms": self.duration_ms,
            "memory_id": self.memory_id,
            "layer": self.layer,
            "memory_type": self.memory_type,
            "confidence": self.confidence,
            "query": self.query,
            "top_k": self.top_k,
            "results_count": self.results_count,
            "status": self.status,
            "error_message": self.error_message,
            "attributes": dict(self.attributes),
            "service_name": self.service_name,
        }

    @classmethod
    def from_dict(cls, data: dict) -> "MemorySpan":
        """Reconstruct a MemorySpan from a plain dictionary.

        Args:
            data: Dictionary produced by :meth:`to_dict`.

        Returns:
            New MemorySpan instance.
        """
        # Remove keys that are not constructor params
        filtered = {k: v for k, v in data.items() if k in cls.__dataclass_fields__}
        return cls(**filtered)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _to_otel_value(value: Any) -> dict:
    """Convert a Python value to OTel AnyValue representation.

    OTLP uses a tagged union for attribute values.  This function maps
    Python scalars to the appropriate OTel type wrapper.

    Args:
        value: Python value (str, int, float, bool, or list).

    Returns:
        Dictionary with a single key indicating the OTel type.
    """
    if isinstance(value, bool):
        return {"boolValue": value}
    if isinstance(value, int):
        return {"intValue": str(value)}
    if isinstance(value, float):
        return {"doubleValue": value}
    if isinstance(value, list):
        return {"arrayValue": {"values": [_to_otel_value(v) for v in value]}}
    return {"stringValue": str(value)}


def _now() -> float:
    """Current Unix timestamp as float seconds."""
    return time.time()


def _iso_now() -> str:
    """Current timestamp as ISO 8601 string with UTC timezone."""
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


# ---------------------------------------------------------------------------
# Traced store proxy
# ---------------------------------------------------------------------------


class _TracedStore:
    """Proxy wrapper that intercepts MemoryStore calls to record spans.

    This class implements the proxy pattern -- it delegates all attribute
    accesses to the underlying store while wrapping CRUD methods with
    span recording.  Non-CRUD attributes are passed through unchanged.

    Attributes:
        _store: The underlying MemoryStore instance.
        _exporter: The MemoryOTelExporter to record spans into.
    """

    def __init__(self, store: Any, exporter: "MemoryOTelExporter") -> None:
        """Initialize the traced store proxy.

        Args:
            store: MemoryStore instance to wrap.
            exporter: Exporter to record spans into.
        """
        object.__setattr__(self, "_store", store)
        object.__setattr__(self, "_exporter", exporter)

    def __getattr__(self, name: str) -> Any:
        """Delegate unknown attributes to the underlying store."""
        return getattr(self._store, name)

    def __setattr__(self, name: str, value: Any) -> None:
        """Set attributes on the underlying store."""
        if name in ("_store", "_exporter"):
            object.__setattr__(self, name, value)
        else:
            setattr(self._store, name, value)

    def insert_memory(self, *args: Any, **kwargs: Any) -> str:
        """Insert a memory and record a store span."""
        start = _now()
        error_message: Optional[str] = None
        memory_id: Optional[str] = None
        try:
            memory_id = self._store.insert_memory(*args, **kwargs)
            return memory_id
        except Exception as exc:
            error_message = str(exc)
            raise
        finally:
            duration_ms = (_now() - start) * 1000
            layer = args[0] if args else kwargs.get("layer", "unknown")
            confidence = kwargs.get("confidence")
            self._exporter.record_store(
                memory_id=memory_id,
                layer=layer,
                content=args[1] if len(args) > 1 else kwargs.get("content", ""),
                confidence=confidence if confidence is not None else 1.0,
                duration_ms=duration_ms,
                status="error" if error_message else "ok",
                error_message=error_message,
            )

    def get_memory(self, *args: Any, **kwargs: Any) -> Any:
        """Get a memory and record a retrieve span."""
        start = _now()
        error_message: Optional[str] = None
        try:
            return self._store.get_memory(*args, **kwargs)
        except Exception as exc:
            error_message = str(exc)
            raise
        finally:
            duration_ms = (_now() - start) * 1000
            memory_id = args[0] if args else kwargs.get("id", "")
            self._exporter.record_retrieve(
                memory_id=memory_id,
                layer="",
                duration_ms=duration_ms,
                query="",
                status="error" if error_message else "ok",
                error_message=error_message,
            )

    def list_memories(self, *args: Any, **kwargs: Any) -> Any:
        """List memories and record a search span."""
        start = _now()
        error_message: Optional[str] = None
        results_count = 0
        try:
            results = self._store.list_memories(*args, **kwargs)
            results_count = len(results)
            return results
        except Exception as exc:
            error_message = str(exc)
            raise
        finally:
            duration_ms = (_now() - start) * 1000
            layer = kwargs.get("layer") or (args[0] if args else None)
            self._exporter.record_search(
                query="list_all",
                top_k=results_count if results_count else 0,
                results_count=results_count,
                layer=layer,
                duration_ms=duration_ms,
                status="error" if error_message else "ok",
                error_message=error_message,
            )

    def delete_memory(self, *args: Any, **kwargs: Any) -> Any:
        """Delete a memory and record a delete span."""
        start = _now()
        error_message: Optional[str] = None
        try:
            return self._store.delete_memory(*args, **kwargs)
        except Exception as exc:
            error_message = str(exc)
            raise
        finally:
            duration_ms = (_now() - start) * 1000
            memory_id = args[0] if args else kwargs.get("id", "")
            self._exporter.record_delete(
                memory_id=memory_id,
                layer="",
                duration_ms=duration_ms,
                status="error" if error_message else "ok",
                error_message=error_message,
            )

    def update_memory_layer(self, *args: Any, **kwargs: Any) -> Any:
        """Update memory layer and record an update span."""
        start = _now()
        error_message: Optional[str] = None
        try:
            return self._store.update_memory_layer(*args, **kwargs)
        except Exception as exc:
            error_message = str(exc)
            raise
        finally:
            duration_ms = (_now() - start) * 1000
            memory_id = args[0] if args else kwargs.get("id", "")
            new_layer = args[1] if len(args) > 1 else kwargs.get("new_layer", "")
            self._exporter.record_update(
                memory_id=memory_id,
                layer=new_layer,
                duration_ms=duration_ms,
                update_type="layer",
                status="error" if error_message else "ok",
                error_message=error_message,
            )

    def update_memory_confidence(self, *args: Any, **kwargs: Any) -> Any:
        """Update memory confidence and record an update span."""
        start = _now()
        error_message: Optional[str] = None
        try:
            return self._store.update_memory_confidence(*args, **kwargs)
        except Exception as exc:
            error_message = str(exc)
            raise
        finally:
            duration_ms = (_now() - start) * 1000
            memory_id = args[0] if args else kwargs.get("id", "")
            new_confidence = (
                args[1] if len(args) > 1 else kwargs.get("new_confidence", 0.0)
            )
            self._exporter.record_update(
                memory_id=memory_id,
                layer="",
                duration_ms=duration_ms,
                update_type="confidence",
                new_confidence=new_confidence,
                status="error" if error_message else "ok",
                error_message=error_message,
            )


# ---------------------------------------------------------------------------
# Main exporter
# ---------------------------------------------------------------------------


class MemoryOTelExporter:
    """OpenTelemetry exporter for MemCtrl memory operations.

    Traces all memory operations (CRUD + search) as OTel-compatible spans.
    Can export to any OTel backend via OTLP.  Works without any external
    dependencies -- if the ``opentelemetry`` SDK is installed it will be
    used automatically; otherwise the exporter falls back to its own
    lightweight span recording.

    The exporter is thread-safe and can be shared across multiple threads
    or async tasks.

    Usage::

        exporter = MemoryOTelExporter(service_name="memctrl")
        exporter.start()

        # Automatic tracing via wrapper:
        traced_store = exporter.trace_store(store)
        traced_store.insert_memory("project", "fact", "source")

        # Get spans:
        spans = exporter.get_spans()

        # Export to JSON (for debugging):
        exporter.export_json("spans.json")

        # Clear:
        exporter.stop()
    """

    def __init__(
        self,
        service_name: str = "memctrl",
        db_path: Optional[str] = None,
        max_spans: int = 10000,
    ) -> None:
        """Initialize the exporter.

        Args:
            service_name: Service name attached to every span.  This
                becomes the ``service.name`` resource attribute in OTLP
                export and is visible in backends such as Datadog and
                Honeycomb.
            db_path: Optional path to a SQLite database for persistent
                span storage. When provided, every recorded span is also
                written to the ``otel_spans`` table, and :meth:`get_spans`
                loads from SQLite instead of in-memory list. This prevents
                data loss across process restarts and bounds memory usage.
            max_spans: Maximum number of spans to keep in memory when
                *db_path* is not provided. When the limit is exceeded,
                oldest spans are dropped (FIFO). With *db_path*, this
                controls the SQLite table size via automatic pruning.
        """
        self.service_name = service_name
        self._db_path = db_path
        self._max_spans = max_spans
        self._spans: List[MemorySpan] = []
        self._active = False
        self._trace_id = str(uuid.uuid4())
        self._lock = threading.Lock()
        self._otel_provider: Any = None
        self._otel_exporter: Any = None
        self._otel_tracer: Any = None
        self._span_count: int = 0  # approximate counter for pruning

    def start(self) -> None:
        """Start collecting spans.

        Activates the exporter so that subsequent ``record_*`` calls are
        stored.  If the OpenTelemetry SDK is available, a real tracer
        provider is configured in addition to the in-memory fallback.
        """
        with self._lock:
            self._active = True
            self._trace_id = str(uuid.uuid4())

        if _HAS_OTEL:
            self._otel_provider = TracerProvider()
            self._otel_exporter = OtelSdkInMemoryExporter()
            self._otel_provider.add_span_processor(
                BatchSpanProcessor(self._otel_exporter)
            )
            trace.set_tracer_provider(self._otel_provider)
            self._otel_tracer = trace.get_tracer("memctrl")

    def stop(self) -> None:
        """Stop collecting spans.

        After calling ``stop``, new ``record_*`` calls are silently
        ignored.  Existing spans remain accessible via
        :meth:`get_spans` until :meth:`clear` is called.
        """
        with self._lock:
            self._active = False

    @property
    def is_active(self) -> bool:
        """Whether the exporter is currently collecting spans."""
        return self._active

    # -- Recording helpers --------------------------------------------------

    def _make_span(
        self,
        operation: str,
        duration_ms: float,
        memory_id: Optional[str] = None,
        layer: Optional[str] = None,
        memory_type: Optional[str] = None,
        confidence: Optional[float] = None,
        query: Optional[str] = None,
        top_k: Optional[int] = None,
        results_count: Optional[int] = None,
        status: str = "ok",
        error_message: Optional[str] = None,
        extra_attributes: Optional[Dict[str, Any]] = None,
    ) -> MemorySpan:
        """Create and store a MemorySpan (internal helper).

        Args:
            operation: One of store, retrieve, search, update, delete.
            duration_ms: Operation duration in milliseconds.
            memory_id: Affected memory ID.
            layer: Memory layer.
            memory_type: Memory type classification.
            confidence: Confidence score.
            query: Search query string.
            top_k: Top-k parameter for search.
            results_count: Number of results returned.
            status: "ok" or "error".
            error_message: Error description (when status is "error").
            extra_attributes: Additional custom attributes.

        Returns:
            The created MemorySpan.
        """
        span = MemorySpan(
            span_id=str(uuid.uuid4()),
            trace_id=self._trace_id,
            operation=operation,
            timestamp=_now(),
            duration_ms=duration_ms,
            memory_id=memory_id,
            layer=layer,
            memory_type=memory_type,
            confidence=confidence,
            query=query,
            top_k=top_k,
            results_count=results_count,
            status=status,
            error_message=error_message,
            attributes=extra_attributes or {},
            service_name=self.service_name,
        )

        with self._lock:
            if self._active:
                self._spans.append(span)
                # Enforce in-memory bound (FIFO)
                if len(self._spans) > self._max_spans:
                    self._spans = self._spans[-self._max_spans :]
                self._span_count += 1

        # Persist to SQLite for cross-process durability
        if self._db_path is not None:
            try:
                self._persist_span(span)
            except Exception:
                # Persistence failure must not break tracing
                pass

        return span

    def _persist_span(self, span: MemorySpan) -> None:
        """Write a single span to SQLite.

        Uses a direct sqlite3 connection to avoid coupling to
        MemoryStore. The ``otel_spans`` table is created lazily if
        it does not exist.
        """
        conn = sqlite3.connect(self._db_path)
        try:
            conn.execute(
                """CREATE TABLE IF NOT EXISTS otel_spans (
                    id          TEXT PRIMARY KEY,
                    trace_id    TEXT NOT NULL,
                    span_id     TEXT NOT NULL,
                    operation   TEXT NOT NULL,
                    timestamp   REAL NOT NULL,
                    duration_ms REAL NOT NULL,
                    memory_id   TEXT,
                    layer       TEXT,
                    memory_type TEXT,
                    confidence  REAL,
                    query       TEXT,
                    top_k       INTEGER,
                    results_count INTEGER,
                    status      TEXT NOT NULL,
                    error_message TEXT,
                    attributes_json TEXT,
                    service_name TEXT NOT NULL
                )"""
            )
            conn.execute(
                """CREATE INDEX IF NOT EXISTS idx_otel_spans_trace
                   ON otel_spans(trace_id)"""
            )
            conn.execute(
                """CREATE INDEX IF NOT EXISTS idx_otel_spans_op
                   ON otel_spans(operation)"""
            )
            sid = str(uuid.uuid4())
            conn.execute(
                """INSERT INTO otel_spans (id, trace_id, span_id, operation,
                                            timestamp, duration_ms, memory_id,
                                            layer, memory_type, confidence, query,
                                            top_k, results_count, status,
                                            error_message, attributes_json,
                                            service_name)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
                (
                    sid,
                    span.trace_id,
                    span.span_id,
                    span.operation,
                    span.timestamp,
                    span.duration_ms,
                    span.memory_id,
                    span.layer,
                    span.memory_type,
                    span.confidence,
                    span.query,
                    span.top_k,
                    span.results_count,
                    span.status,
                    span.error_message,
                    json.dumps(span.attributes),
                    span.service_name,
                ),
            )
            # Prune oldest rows to keep table bounded
            conn.execute(
                """DELETE FROM otel_spans
                   WHERE id NOT IN (
                       SELECT id FROM otel_spans
                       ORDER BY timestamp DESC LIMIT ?
                   )""",
                (self._max_spans,),
            )
            conn.commit()
        finally:
            conn.close()

    # -- Public recording API -----------------------------------------------

    def record_store(
        self,
        memory_id: Optional[str],
        layer: str,
        content: str,
        confidence: float,
        duration_ms: float,
        status: str = "ok",
        error_message: Optional[str] = None,
        memory_type: Optional[str] = None,
        extra_attributes: Optional[Dict[str, Any]] = None,
    ) -> MemorySpan:
        """Record a store (insert) operation span.

        Args:
            memory_id: ID of the newly created memory (may be None if
                the insert failed).
            layer: Target memory layer.
            content: Memory content (stored as an attribute).
            confidence: Confidence score.
            duration_ms: Insertion duration in milliseconds.
            status: "ok" or "error".
            error_message: Error description when status is "error".
            memory_type: Classification of the memory.
            extra_attributes: Additional custom attributes.

        Returns:
            The recorded MemorySpan.
        """
        attrs = {"content": content}
        if extra_attributes:
            attrs.update(extra_attributes)
        return self._make_span(
            operation="store",
            duration_ms=duration_ms,
            memory_id=memory_id,
            layer=layer if layer in VALID_LAYERS else None,
            confidence=confidence,
            status=status,
            error_message=error_message,
            memory_type=memory_type,
            extra_attributes=attrs,
        )

    def record_retrieve(
        self,
        memory_id: str,
        layer: str,
        duration_ms: float,
        query: str = "",
        status: str = "ok",
        error_message: Optional[str] = None,
        memory_type: Optional[str] = None,
        extra_attributes: Optional[Dict[str, Any]] = None,
    ) -> MemorySpan:
        """Record a retrieve (single memory lookup) operation span.

        Args:
            memory_id: ID of the retrieved memory.
            layer: Memory layer.
            duration_ms: Retrieval duration in milliseconds.
            query: Optional query string that led to this retrieval.
            status: "ok" or "error".
            error_message: Error description when status is "error".
            memory_type: Classification of the memory.
            extra_attributes: Additional custom attributes.

        Returns:
            The recorded MemorySpan.
        """
        return self._make_span(
            operation="retrieve",
            duration_ms=duration_ms,
            memory_id=memory_id,
            layer=layer if layer in VALID_LAYERS else None,
            query=query if query else None,
            status=status,
            error_message=error_message,
            memory_type=memory_type,
            extra_attributes=extra_attributes,
        )

    def record_search(
        self,
        query: str,
        top_k: int,
        results_count: int,
        layer: Optional[str],
        duration_ms: float,
        status: str = "ok",
        error_message: Optional[str] = None,
        extra_attributes: Optional[Dict[str, Any]] = None,
    ) -> MemorySpan:
        """Record a search (multi-result query) operation span.

        Args:
            query: Search query string.
            top_k: Maximum number of results requested.
            results_count: Actual number of results returned.
            layer: Filtered layer (if any).
            duration_ms: Search duration in milliseconds.
            status: "ok" or "error".
            error_message: Error description when status is "error".
            extra_attributes: Additional custom attributes.

        Returns:
            The recorded MemorySpan.
        """
        return self._make_span(
            operation="search",
            duration_ms=duration_ms,
            query=query,
            top_k=top_k,
            results_count=results_count,
            layer=layer if layer in VALID_LAYERS else None,
            status=status,
            error_message=error_message,
            extra_attributes=extra_attributes,
        )

    def record_update(
        self,
        memory_id: str,
        layer: str,
        duration_ms: float,
        update_type: str = "content",
        new_confidence: Optional[float] = None,
        status: str = "ok",
        error_message: Optional[str] = None,
        extra_attributes: Optional[Dict[str, Any]] = None,
    ) -> MemorySpan:
        """Record an update (modify) operation span.

        Args:
            memory_id: ID of the updated memory.
            layer: Memory layer.
            duration_ms: Update duration in milliseconds.
            update_type: What was updated -- "content", "confidence",
                or "layer".
            new_confidence: New confidence value (if updated).
            status: "ok" or "error".
            error_message: Error description when status is "error".
            extra_attributes: Additional custom attributes.

        Returns:
            The recorded MemorySpan.
        """
        attrs: Dict[str, Any] = {"update_type": update_type}
        if new_confidence is not None:
            attrs["new_confidence"] = new_confidence
        if extra_attributes:
            attrs.update(extra_attributes)
        return self._make_span(
            operation="update",
            duration_ms=duration_ms,
            memory_id=memory_id,
            layer=layer if layer in VALID_LAYERS else None,
            status=status,
            error_message=error_message,
            extra_attributes=attrs,
        )

    def record_delete(
        self,
        memory_id: str,
        layer: str,
        duration_ms: float,
        status: str = "ok",
        error_message: Optional[str] = None,
        extra_attributes: Optional[Dict[str, Any]] = None,
    ) -> MemorySpan:
        """Record a delete operation span.

        Args:
            memory_id: ID of the deleted memory.
            layer: Memory layer.
            duration_ms: Deletion duration in milliseconds.
            status: "ok" or "error".
            error_message: Error description when status is "error".
            extra_attributes: Additional custom attributes.

        Returns:
            The recorded MemorySpan.
        """
        return self._make_span(
            operation="delete",
            duration_ms=duration_ms,
            memory_id=memory_id,
            layer=layer if layer in VALID_LAYERS else None,
            status=status,
            error_message=error_message,
            extra_attributes=extra_attributes,
        )

    # -- Store wrapper ------------------------------------------------------

    def trace_store(self, store: Any) -> _TracedStore:
        """Wrap a MemoryStore to automatically trace all operations.

        Returns a proxy object that intercepts all memory CRUD
        operations and records a span for each call.  The proxy
        transparently delegates all other attribute accesses to the
        underlying store.

        Args:
            store: A :class:`memctrl.store.MemoryStore` instance.

        Returns:
            A proxy store that records spans on every mutating call.

        Example::

            store = MemoryStore()
            traced = exporter.trace_store(store)
            traced.insert_memory("project", "we use FastAPI", "test")
            assert len(exporter.get_spans()) == 1
        """
        return _TracedStore(store, self)

    # -- Queries ------------------------------------------------------------

    def get_spans(self) -> List[MemorySpan]:
        """Get all recorded spans.

        When ``db_path`` was provided at construction, this loads from
        SQLite (survives restarts). Otherwise returns the in-memory list.

        Returns:
            List of MemorySpan objects in chronological order.
        """
        if self._db_path is not None:
            return self._load_spans_from_db()
        with self._lock:
            return list(self._spans)

    def _load_spans_from_db(self) -> List[MemorySpan]:
        """Load spans from SQLite and reconstruct MemorySpan objects.

        Loads ALL spans regardless of trace_id so that audit history
        survives across exporter restarts (each restart gets a new
        trace_id, but the user expects to see previous spans).
        """
        conn = sqlite3.connect(self._db_path)
        try:
            conn.row_factory = sqlite3.Row
            rows = conn.execute(
                """SELECT * FROM otel_spans
                   ORDER BY timestamp ASC"""
            ).fetchall()
            spans: List[MemorySpan] = []
            for r in rows:
                try:
                    spans.append(
                        MemorySpan(
                            span_id=r["span_id"],
                            trace_id=r["trace_id"],
                            operation=r["operation"],
                            timestamp=r["timestamp"],
                            duration_ms=r["duration_ms"],
                            memory_id=r["memory_id"],
                            layer=r["layer"],
                            memory_type=r["memory_type"],
                            confidence=r["confidence"],
                            query=r["query"],
                            top_k=r["top_k"],
                            results_count=r["results_count"],
                            status=r["status"],
                            error_message=r["error_message"],
                            attributes=json.loads(r["attributes_json"])
                            if r["attributes_json"]
                            else {},
                            service_name=r["service_name"],
                        )
                    )
                except Exception:
                    # Skip corrupted rows
                    continue
            return spans
        finally:
            conn.close()

    def get_spans_by_operation(self, operation: str) -> List[MemorySpan]:
        """Get spans filtered by operation type.

        When ``db_path`` was provided at construction, this loads from
        SQLite (survives restarts) before filtering. Otherwise returns
        from the in-memory list.

        Args:
            operation: One of ``store``, ``retrieve``, ``search``,
                ``update``, ``delete``.

        Returns:
            Subset of spans matching the given operation.

        Raises:
            ValueError: If *operation* is not a valid operation name.
        """
        if operation not in VALID_OPERATIONS:
            raise ValueError(
                f"Invalid operation '{operation}'. Must be one of: {VALID_OPERATIONS}"
            )
        spans = self.get_spans()
        return [s for s in spans if s.operation == operation]

    def get_stats(self) -> dict:
        """Get aggregate statistics from all recorded spans.

        Computes per-operation counts, total duration, average
        duration, and error rate.

        Returns:
            Dictionary with keys:
                - ``total_spans``: Total number of spans.
                - ``by_operation``: Dict mapping operation -> count.
                - ``total_duration_ms``: Sum of all durations.
                - ``avg_duration_ms``: Average duration.
                - ``error_count``: Number of spans with status "error".
                - ``error_rate``: Fraction of spans with errors (0.0-1.0).
                - ``by_layer``: Dict mapping layer -> count.
        """
        spans = self.get_spans()

        total = len(spans)
        if total == 0:
            return {
                "total_spans": 0,
                "by_operation": {},
                "total_duration_ms": 0.0,
                "avg_duration_ms": 0.0,
                "error_count": 0,
                "error_rate": 0.0,
                "by_layer": {},
            }

        by_operation: Dict[str, int] = {}
        by_layer: Dict[str, int] = {}
        total_duration = 0.0
        error_count = 0

        for span in spans:
            by_operation[span.operation] = by_operation.get(span.operation, 0) + 1
            total_duration += span.duration_ms
            if span.status == "error":
                error_count += 1
            if span.layer:
                by_layer[span.layer] = by_layer.get(span.layer, 0) + 1

        return {
            "total_spans": total,
            "by_operation": by_operation,
            "total_duration_ms": round(total_duration, 3),
            "avg_duration_ms": round(total_duration / total, 3),
            "error_count": error_count,
            "error_rate": round(error_count / total, 4),
            "by_layer": by_layer,
        }

    # -- Export -------------------------------------------------------------

    def export_json(self, path: str) -> None:
        """Export spans to a JSON file for inspection and debugging.

        Each span is serialized with :meth:`MemorySpan.to_dict`, producing
        a human-friendly representation.

        Args:
            path: File path to write JSON to.
        """
        spans = self.get_spans()
        data = {
            "service_name": self.service_name,
            "trace_id": self._trace_id,
            "exported_at": _iso_now(),
            "span_count": len(spans),
            "spans": [s.to_dict() for s in spans],
        }

        with open(path, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=2, default=str)

    def export_otlp_json(self, path: str) -> None:
        """Export spans in OTLP-compatible JSON format.

        Produces a JSON payload that mirrors the OpenTelemetry Protocol
        (OTLP) structure.  This format can be ingested directly by:

        - **Datadog Agent** OTel gRPC/HTTP receiver
        - **Grafana Tempo** OTLP endpoint
        - **Jaeger** OTLP collector
        - **Honeycomb** OTLP HTTP endpoint

        The output follows the ``ExportTraceServiceRequest`` structure::

            {
              "resourceSpans": [
                {
                  "resource": { "attributes": [...] },
                  "scopeSpans": [
                    {
                      "scope": { "name": "memctrl", ... },
                      "spans": [ { "traceId": ..., "spanId": ..., ... } ]
                    }
                  ]
                }
              ]
            }

        Args:
            path: File path to write OTLP JSON to.
        """
        spans = self.get_spans()
        service = self.service_name

        otel_spans = [s.to_otel_dict() for s in spans]

        payload = {
            "resourceSpans": [
                {
                    "resource": {
                        "attributes": [
                            {
                                "key": "service.name",
                                "value": {"stringValue": service},
                            },
                            {
                                "key": "telemetry.sdk.name",
                                "value": {"stringValue": "memctrl"},
                            },
                            {
                                "key": "telemetry.sdk.language",
                                "value": {"stringValue": "python"},
                            },
                        ]
                    },
                    "scopeSpans": [
                        {
                            "scope": {
                                "name": "memctrl",
                                "version": "1.1.0",
                            },
                            "spans": otel_spans,
                        }
                    ],
                }
            ]
        }

        with open(path, "w", encoding="utf-8") as f:
            json.dump(payload, f, indent=2, default=str)

    def clear(self) -> None:
        """Clear all recorded spans and reset trace ID."""
        with self._lock:
            self._spans.clear()
            self._trace_id = str(uuid.uuid4())
            self._span_count = 0
        if self._db_path is not None:
            try:
                conn = sqlite3.connect(self._db_path)
                conn.execute("DELETE FROM otel_spans")
                conn.commit()
                conn.close()
            except Exception:
                pass

    # -- Context manager support --------------------------------------------

    def __enter__(self) -> "MemoryOTelExporter":
        """Enter context manager -- starts the exporter."""
        self.start()
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        """Exit context manager -- stops the exporter."""
        self.stop()
