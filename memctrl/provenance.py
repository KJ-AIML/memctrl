"""MemCtrl -- Retrieval Provenance System.

Tracks the complete provenance of every retrieval operation:
- Which memories were retrieved
- Why they were retrieved (match reason)
- Their source (explicit/inferred/reflection/mentioned)
- Their confidence at time of retrieval
- The query that triggered retrieval
- The trace path through the memory tree

This enables:
- Trust: users can verify why a memory was returned
- Debugging: see exactly how retrieval decisions were made
- Audit: compliance trail for memory-influenced decisions
- Security: detect if poisoned memories are being retrieved
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime
from typing import Dict, List, Optional


@dataclass
class MemorySource:
    """Source information for a single retrieved memory.

    WHY: Every retrieved memory needs a full provenance trail so users
    can understand *why* a particular memory was returned. This includes
    not just what matched, but how it matched and where it came from.
    """

    memory_id: str
    content: str
    layer: str  # project/session/user
    source_type: str  # explicit/inferred/mentioned/reflection
    confidence: float
    match_reason: str  # why this memory matched the query
    trace_path: List[str] = field(
        default_factory=list
    )  # e.g. ["root", "project", "tech_stack"]

    def to_dict(self) -> dict:
        """Serialize to dict with truncated content for readability."""
        return {
            "memory_id": self.memory_id,
            "content": self.content[:200] + "..."
            if len(self.content) > 200
            else self.content,
            "layer": self.layer,
            "source_type": self.source_type,
            "confidence": self.confidence,
            "match_reason": self.match_reason,
            "trace_path": self.trace_path,
        }


@dataclass
class RetrievalProvenance:
    """Complete provenance record for a single retrieval operation.

    WHY: A single retrieval may touch dozens of memories across multiple
    layers. This record captures the complete context so retrieval
    decisions can be audited, debugged, and trusted.
    """

    query: str
    timestamp: datetime
    sources: List[MemorySource] = field(default_factory=list)
    total_memories_searched: int = 0
    retrieval_method: str = ""  # llm/keyword/hybrid
    tree_version: int = 0

    @property
    def avg_confidence(self) -> float:
        """Average confidence of retrieved sources.

        WHY: Low average confidence signals that retrieval may be
        returning stale or poorly-matched memories. This property
        enables quick health checks on retrieval quality.
        """
        if not self.sources:
            return 0.0
        return sum(s.confidence for s in self.sources) / len(self.sources)

    @property
    def layer_breakdown(self) -> Dict[str, int]:
        """Count of sources per layer.

        WHY: Understanding which layers contribute most to retrieval
        helps identify if, e.g., session memories dominate when
        project-level knowledge should be used.
        """
        counts: Dict[str, int] = {}
        for s in self.sources:
            counts[s.layer] = counts.get(s.layer, 0) + 1
        return counts

    @property
    def source_type_breakdown(self) -> Dict[str, int]:
        """Count of sources by source type.

        WHY: A heavy skew toward "inferred" or "mentioned" sources
        may indicate that explicit knowledge is missing. This
        breakdown helps surface knowledge gaps.
        """
        counts: Dict[str, int] = {}
        for s in self.sources:
            counts[s.source_type] = counts.get(s.source_type, 0) + 1
        return counts

    def to_dict(self) -> dict:
        """Serialize complete provenance record to dict."""
        return {
            "query": self.query,
            "timestamp": self.timestamp.isoformat(),
            "sources": [s.to_dict() for s in self.sources],
            "total_memories_searched": self.total_memories_searched,
            "retrieval_method": self.retrieval_method,
            "tree_version": self.tree_version,
            "avg_confidence": self.avg_confidence,
            "layer_breakdown": self.layer_breakdown,
            "source_type_breakdown": self.source_type_breakdown,
        }


class ProvenanceTracker:
    """Tracks provenance for all retrieval operations.

    Usage:
        tracker = ProvenanceTracker()

        # After retrieval:
        prov = tracker.record_retrieval(
            query="what is our stack?",
            results=retrieved_memories,
            method="keyword",
            tree_version=5,
        )

        # Get provenance report:
        report = tracker.get_provenance_report(query="what is our stack?")

    WHY: Without explicit provenance tracking, retrieval becomes a black
    box. This class provides the audit trail needed for trust, debugging,
    compliance, and security analysis of memory retrieval.
    """

    def __init__(self, max_history: int = 100):
        """Initialize with max history size.

        Args:
            max_history: Maximum number of retrieval operations to keep
                in memory. Older records are discarded. This prevents
                unbounded memory growth in long-running sessions.
        """
        self._history: List[RetrievalProvenance] = []
        self._max_history = max_history

    def record_retrieval(
        self,
        query: str,
        results: List[dict],  # list of memory dicts from retriever
        method: str,
        tree_version: int,
        total_memories_searched: int = 0,
        trace_paths: Optional[Dict[str, List[str]]] = None,
        match_reasons: Optional[Dict[str, str]] = None,
    ) -> RetrievalProvenance:
        """Record provenance for a retrieval operation.

        WHY: This is the core entry point. After any retrieval (LLM,
        keyword, or hybrid), call this to create an immutable provenance
        record that captures exactly what was retrieved and why.

        Args:
            query: The original query string.
            results: Retrieved memory dicts, each expected to have keys:
                id, content, layer, source, confidence.
            method: Retrieval method used -- "llm", "keyword", or "hybrid".
            tree_version: Current tree version at retrieval time.
            total_memories_searched: Total memories examined during search.
            trace_paths: Optional dict mapping memory_id -> trace path list.
            match_reasons: Optional dict mapping memory_id -> match reason string.

        Returns:
            RetrievalProvenance record for this operation.
        """
        sources: List[MemorySource] = []
        for mem in results:
            mid = mem.get("id", "")
            content = mem.get("content", "")
            layer = mem.get("layer", "unknown")
            source_type = mem.get("source", "unknown")
            confidence = mem.get("confidence", 0.0)

            # Derive trace path if provided, else default
            trace_path = trace_paths.get(mid, ["root"]) if trace_paths else ["root"]

            # Derive match reason if provided, else infer from method
            if match_reasons and mid in match_reasons:
                match_reason = match_reasons[mid]
            else:
                match_reason = f"matched via {method} retrieval"

            sources.append(
                MemorySource(
                    memory_id=mid,
                    content=content,
                    layer=layer,
                    source_type=source_type,
                    confidence=confidence,
                    match_reason=match_reason,
                    trace_path=trace_path,
                )
            )

        provenance = RetrievalProvenance(
            query=query,
            timestamp=datetime.now(),
            sources=sources,
            total_memories_searched=total_memories_searched,
            retrieval_method=method,
            tree_version=tree_version,
        )

        self._history.append(provenance)

        # Enforce max history to prevent unbounded growth
        if len(self._history) > self._max_history:
            self._history = self._history[-self._max_history :]

        return provenance

    def get_provenance_report(self, query: Optional[str] = None) -> dict:
        """Get provenance report for a specific query or all queries.

        WHY: Users and auditors need a human-readable summary of
        retrieval provenance. This aggregates across operations to
        show patterns in what memories are being retrieved.

        Args:
            query: If provided, filter to records matching this query.
                   If None, aggregate across all recorded retrievals.

        Returns:
            Dict with retrieval count, source breakdown, layer breakdown,
            average confidence, and method breakdown.
        """
        records = self._history
        if query:
            records = [r for r in records if r.query == query]

        if not records:
            return {
                "query": query,
                "retrieval_count": 0,
                "sources": [],
                "layer_breakdown": {},
                "source_type_breakdown": {},
                "avg_confidence": 0.0,
                "method_breakdown": {},
            }

        all_sources: List[MemorySource] = []
        for r in records:
            all_sources.extend(r.sources)

        # Aggregate layer breakdown
        layer_counts: Dict[str, int] = {}
        for s in all_sources:
            layer_counts[s.layer] = layer_counts.get(s.layer, 0) + 1

        # Aggregate source type breakdown
        source_type_counts: Dict[str, int] = {}
        for s in all_sources:
            source_type_counts[s.source_type] = (
                source_type_counts.get(s.source_type, 0) + 1
            )

        # Aggregate method breakdown
        method_counts: Dict[str, int] = {}
        for r in records:
            method_counts[r.retrieval_method] = (
                method_counts.get(r.retrieval_method, 0) + 1
            )

        # Average confidence across all sources
        avg_conf = (
            sum(s.confidence for s in all_sources) / len(all_sources)
            if all_sources
            else 0.0
        )

        return {
            "query": query,
            "retrieval_count": len(records),
            "sources": [s.to_dict() for s in all_sources],
            "layer_breakdown": layer_counts,
            "source_type_breakdown": source_type_counts,
            "avg_confidence": avg_conf,
            "method_breakdown": method_counts,
        }

    def get_history(self) -> List[RetrievalProvenance]:
        """Get all provenance records.

        WHY: Direct access to the full history enables custom analysis
        and debugging of retrieval behavior over time.
        """
        return list(self._history)

    def detect_low_confidence_retrievals(
        self, threshold: float = 0.5
    ) -> List[RetrievalProvenance]:
        """Find retrievals where average confidence was below threshold.

        WHY: Low-confidence retrievals are a security and quality signal.
        They may indicate:
        - Poisoned memories being injected with low confidence to evade detection
        - Stale memories that should have been expired
        - Knowledge gaps where no strong match exists

        Args:
            threshold: Confidence floor. Any retrieval with avg_confidence
                       below this is flagged.

        Returns:
            List of RetrievalProvenance records below the threshold.
        """
        return [r for r in self._history if r.avg_confidence < threshold]

    def detect_source_type_imbalance(
        self, ratio_threshold: float = 0.9
    ) -> Optional[dict]:
        """Detect if retrieval relies too heavily on one source type.

        WHY: A healthy memory system draws from diverse source types.
        If 90%+ of retrieved memories come from a single source type
        (e.g., "inferred"), it suggests:
        - Explicit knowledge may be missing
        - The system may be over-relying on weak signals
        - Users may need to add more direct memories

        Args:
            ratio_threshold: If any source type exceeds this ratio of
                             total retrieved sources, flag as imbalanced.

        Returns:
            Dict with the imbalanced source type and its ratio, or None
            if no imbalance is detected.
        """
        all_sources: List[MemorySource] = []
        for r in self._history:
            all_sources.extend(r.sources)

        if not all_sources:
            return None

        source_type_counts: Dict[str, int] = {}
        for s in all_sources:
            source_type_counts[s.source_type] = (
                source_type_counts.get(s.source_type, 0) + 1
            )

        total = len(all_sources)
        for source_type, count in source_type_counts.items():
            ratio = count / total
            if ratio > ratio_threshold:
                return {
                    "source_type": source_type,
                    "ratio": ratio,
                    "count": count,
                    "total_sources": total,
                    "message": (
                        f"Retrieval is imbalanced: {ratio:.0%} of retrieved "
                        f"memories are '{source_type}'. This may indicate "
                        f"a knowledge gap in explicit memories."
                    ),
                }

        return None

    def clear(self) -> None:
        """Clear all history.

        WHY: Useful for testing and for privacy compliance -- when a
        user requests deletion of their data, the provenance trail
        must also be cleared.
        """
        self._history.clear()
