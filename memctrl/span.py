"""MemCtrl -- Memory Span context manager for operation tracing.

Provides a context manager that traces all memory operations within a scope:

    with tracker.span("debug_auth_issue") as span:
        agent.recall("auth requirements")
        agent.store("discovered jwt bug")
        agent.recall("jwt implementation")

This creates a traceable scope around agent activities, enabling:
- Debugging: See exactly what memories were accessed during a task
- Compliance: Audit trail for memory-influenced decisions
- Observability: Export to OTel-compatible backends
- Cost tracking: Understand which operations consume resources

Inspired by OpenTelemetry span patterns but lightweight and zero-dependency.
"""

from __future__ import annotations

import time
import uuid
from contextlib import contextmanager
from dataclasses import dataclass, field
from typing import Any, Dict, Generator, List, Optional


# ---------------------------------------------------------------------------
# Data models
# ---------------------------------------------------------------------------


@dataclass
class MemoryOperation:
    """A single memory operation within a span."""

    operation: str  # store|retrieve|search|update|delete
    memory_id: Optional[str] = None
    layer: Optional[str] = None
    content_preview: Optional[str] = None  # Truncated content
    confidence: Optional[float] = None
    query: Optional[str] = None
    timestamp: float = 0.0
    duration_ms: float = 0.0

    def to_dict(self) -> dict:
        """Serialize the operation to a dictionary."""
        return {
            "operation": self.operation,
            "memory_id": self.memory_id,
            "layer": self.layer,
            "content_preview": self.content_preview,
            "confidence": self.confidence,
            "query": self.query,
            "timestamp": self.timestamp,
            "duration_ms": self.duration_ms,
        }


@dataclass
class MemorySpan:
    """A traced scope of memory operations.

    Contains all memory operations that occurred within a single
    logical scope (e.g., "debugging auth issue", "implementing feature X").
    """

    name: str
    span_id: str
    start_time: float
    end_time: Optional[float] = None
    operations: List[MemoryOperation] = field(default_factory=list)
    metadata: Dict[str, Any] = field(default_factory=dict)

    @property
    def duration_ms(self) -> float:
        """Total span duration in milliseconds.

        Returns elapsed time from start_time to end_time if the span has
        completed, or from start_time to now if it is still active.
        """
        if self.end_time is None:
            return (time.time() - self.start_time) * 1000
        return (self.end_time - self.start_time) * 1000

    @property
    def operation_counts(self) -> Dict[str, int]:
        """Count of each operation type."""
        counts: Dict[str, int] = {}
        for op in self.operations:
            counts[op.operation] = counts.get(op.operation, 0) + 1
        return counts

    @property
    def layer_access_counts(self) -> Dict[str, int]:
        """Count of accesses per layer."""
        counts: Dict[str, int] = {}
        for op in self.operations:
            if op.layer:
                counts[op.layer] = counts.get(op.layer, 0) + 1
        return counts

    def add_operation(self, op: MemoryOperation) -> None:
        """Add an operation to this span."""
        self.operations.append(op)

    def to_dict(self) -> dict:
        """Serialize the span to a dictionary."""
        return {
            "name": self.name,
            "span_id": self.span_id,
            "duration_ms": self.duration_ms,
            "operation_counts": self.operation_counts,
            "layer_access_counts": self.layer_access_counts,
            "operations": [op.to_dict() for op in self.operations],
            "metadata": self.metadata,
        }


# ---------------------------------------------------------------------------
# Span tracker
# ---------------------------------------------------------------------------


class SpanTracker:
    """Tracks memory spans across scopes.

    Usage:
        tracker = SpanTracker()

        # Create a span (context manager)
        with tracker.span("implement_oauth"):
            # Memory operations are tracked
            store.insert_memory("project", "using OAuth2", "dev")
            facts = retriever.retrieve("auth requirements")

        # Get completed spans
        for span in tracker.get_completed_spans():
            print(f"{span.name}: {span.duration_ms:.0f}ms, {len(span.operations)} ops")
    """

    def __init__(self, max_completed: int = 50):
        """Initialize tracker.

        Args:
            max_completed: Maximum number of completed spans to retain.
                Older spans are discarded when the limit is exceeded.
        """
        self._active_span: Optional[MemorySpan] = None
        self._completed: List[MemorySpan] = []
        self._max_completed = max_completed

    @contextmanager
    def span(self, name: str, **metadata: Any) -> Generator[MemorySpan, None, None]:
        """Create a memory span context manager.

        All memory operations recorded via :meth:`record_operation` while
        inside the ``with`` block are attached to this span.

        Usage:
            with tracker.span("debug_issue", agent="dev1") as span:
                # All memory ops here are tracked
                store.insert_memory(...)
                retriever.retrieve(...)

        Args:
            name: Human-readable name for the span (e.g., "debug_auth_issue").
            **metadata: Arbitrary key-value metadata to attach to the span.

        Yields:
            The :class:`MemorySpan` instance being tracked.
        """
        span = MemorySpan(
            name=name,
            span_id=str(uuid.uuid4()),
            start_time=time.time(),
            metadata=metadata,
        )
        self._active_span = span
        try:
            yield span
        finally:
            span.end_time = time.time()
            self._completed.append(span)
            # Enforce max completed limit
            if len(self._completed) > self._max_completed:
                self._completed = self._completed[-self._max_completed :]
            self._active_span = None

    def record_operation(self, operation: str, **kwargs: Any) -> None:
        """Record a memory operation in the active span.

        Called automatically by wrapped store/retriever operations.
        If no active span, the operation is silently ignored.

        Args:
            operation: Operation type (store|retrieve|search|update|delete).
            **kwargs: Additional fields matching :class:`MemoryOperation`
                attributes (memory_id, layer, content_preview, confidence,
                query, etc.).
        """
        if self._active_span is None:
            return

        op = MemoryOperation(
            operation=operation,
            timestamp=time.time(),
            **kwargs,
        )
        self._active_span.add_operation(op)

    def get_active_span(self) -> Optional[MemorySpan]:
        """Get currently active span, if any.

        Returns:
            The active :class:`MemorySpan` when inside a ``with`` block,
            or ``None`` otherwise.
        """
        return self._active_span

    def get_completed_spans(self) -> List[MemorySpan]:
        """Get all completed spans.

        Returns:
            List of completed :class:`MemorySpan` instances, ordered from
            oldest to newest.
        """
        return list(self._completed)

    def get_span_by_name(self, name: str) -> Optional[MemorySpan]:
        """Find the most recent completed span by name.

        Args:
            name: Span name to search for.

        Returns:
            The matching :class:`MemorySpan` or ``None``.
        """
        for span in reversed(self._completed):
            if span.name == name:
                return span
        return None

    def get_stats(self) -> dict:
        """Get aggregate stats across all completed spans.

        Returns:
            Dictionary with total spans, total operations, average
            operations per span, and total duration.
        """
        if not self._completed:
            return {
                "total_spans": 0,
                "total_operations": 0,
                "avg_operations_per_span": 0.0,
                "total_duration_ms": 0.0,
            }

        total_ops = sum(len(s.operations) for s in self._completed)
        total_duration = sum(s.duration_ms for s in self._completed)

        return {
            "total_spans": len(self._completed),
            "total_operations": total_ops,
            "avg_operations_per_span": total_ops / len(self._completed),
            "total_duration_ms": total_duration,
        }

    def clear(self) -> None:
        """Clear all completed spans and any active span reference."""
        self._completed.clear()
        self._active_span = None
