"""Tests for Retrieval Provenance System.

Covers: MemorySource, RetrievalProvenance, ProvenanceTracker.
Ensures every retrieval operation can be fully audited and traced.

Key properties tested:
- Recording creates correct provenance with all memory sources
- Aggregation properties (avg_confidence, layer_breakdown, source_type_breakdown)
- Anomaly detection (low confidence, source imbalance)
- History management with max_history limit
- Empty retrieval edge cases
- Serialization to_dict
"""

from __future__ import annotations

import pytest

from memctrl.provenance import MemorySource, ProvenanceTracker, RetrievalProvenance


# ---------------------------------------------------------------------------
# MemorySource
# ---------------------------------------------------------------------------


def test_memory_source_fields():
    """MemorySource should hold all provenance fields for a single memory."""
    ms = MemorySource(
        memory_id="m1",
        content="we use FastAPI",
        layer="project",
        source_type="explicit",
        confidence=1.0,
        match_reason="keyword match: FastAPI",
        trace_path=["root", "project", "tech_stack"],
    )
    assert ms.memory_id == "m1"
    assert ms.content == "we use FastAPI"
    assert ms.layer == "project"
    assert ms.source_type == "explicit"
    assert ms.confidence == 1.0
    assert ms.match_reason == "keyword match: FastAPI"
    assert ms.trace_path == ["root", "project", "tech_stack"]


def test_memory_source_defaults():
    """MemorySource trace_path should default to empty list."""
    ms = MemorySource(
        memory_id="m1",
        content="test",
        layer="session",
        source_type="inferred",
        confidence=0.7,
        match_reason="default",
    )
    assert ms.trace_path == []


def test_memory_source_to_dict():
    """MemorySource.to_dict should serialize all fields."""
    ms = MemorySource(
        memory_id="m1",
        content="short content",
        layer="project",
        source_type="explicit",
        confidence=1.0,
        match_reason="test",
        trace_path=["root"],
    )
    d = ms.to_dict()
    assert d["memory_id"] == "m1"
    assert d["content"] == "short content"
    assert d["layer"] == "project"
    assert d["source_type"] == "explicit"
    assert d["confidence"] == 1.0
    assert d["match_reason"] == "test"
    assert d["trace_path"] == ["root"]


def test_memory_source_to_dict_truncates_long_content():
    """MemorySource.to_dict should truncate content > 200 chars."""
    long_content = "x" * 250
    ms = MemorySource(
        memory_id="m1",
        content=long_content,
        layer="project",
        source_type="explicit",
        confidence=1.0,
        match_reason="test",
    )
    d = ms.to_dict()
    assert d["content"].endswith("...")
    assert len(d["content"]) == 203  # 200 + "..."


# ---------------------------------------------------------------------------
# RetrievalProvenance
# ---------------------------------------------------------------------------


def test_retrieval_provenance_creation():
    """RetrievalProvenance should initialize with all fields."""
    from datetime import datetime

    rp = RetrievalProvenance(
        query="what is our stack?",
        timestamp=datetime.now(),
        sources=[],
        total_memories_searched=10,
        retrieval_method="keyword",
        tree_version=1,
    )
    assert rp.query == "what is our stack?"
    assert rp.total_memories_searched == 10
    assert rp.retrieval_method == "keyword"
    assert rp.tree_version == 1


def test_avg_confidence_with_sources():
    """avg_confidence should compute the mean of all source confidences."""
    from datetime import datetime

    rp = RetrievalProvenance(
        query="test",
        timestamp=datetime.now(),
        sources=[
            MemorySource(
                memory_id="m1",
                content="c1",
                layer="project",
                source_type="explicit",
                confidence=1.0,
                match_reason="test",
            ),
            MemorySource(
                memory_id="m2",
                content="c2",
                layer="session",
                source_type="inferred",
                confidence=0.5,
                match_reason="test",
            ),
        ],
    )
    assert rp.avg_confidence == 0.75


def test_avg_confidence_empty():
    """avg_confidence should be 0.0 when no sources exist."""
    from datetime import datetime

    rp = RetrievalProvenance(query="test", timestamp=datetime.now(), sources=[])
    assert rp.avg_confidence == 0.0


def test_layer_breakdown():
    """layer_breakdown should count sources per layer."""
    from datetime import datetime

    rp = RetrievalProvenance(
        query="test",
        timestamp=datetime.now(),
        sources=[
            MemorySource(
                memory_id="m1",
                content="c1",
                layer="project",
                source_type="explicit",
                confidence=1.0,
                match_reason="test",
            ),
            MemorySource(
                memory_id="m2",
                content="c2",
                layer="session",
                source_type="inferred",
                confidence=0.5,
                match_reason="test",
            ),
            MemorySource(
                memory_id="m3",
                content="c3",
                layer="project",
                source_type="explicit",
                confidence=0.9,
                match_reason="test",
            ),
        ],
    )
    assert rp.layer_breakdown == {"project": 2, "session": 1}


def test_source_type_breakdown():
    """source_type_breakdown should count sources by type."""
    from datetime import datetime

    rp = RetrievalProvenance(
        query="test",
        timestamp=datetime.now(),
        sources=[
            MemorySource(
                memory_id="m1",
                content="c1",
                layer="project",
                source_type="explicit",
                confidence=1.0,
                match_reason="test",
            ),
            MemorySource(
                memory_id="m2",
                content="c2",
                layer="project",
                source_type="inferred",
                confidence=0.7,
                match_reason="test",
            ),
            MemorySource(
                memory_id="m3",
                content="c3",
                layer="session",
                source_type="explicit",
                confidence=0.9,
                match_reason="test",
            ),
        ],
    )
    assert rp.source_type_breakdown == {"explicit": 2, "inferred": 1}


def test_retrieval_provenance_to_dict():
    """to_dict should serialize the complete provenance record."""
    from datetime import datetime

    rp = RetrievalProvenance(
        query="what is our stack?",
        timestamp=datetime(2024, 1, 1, 12, 0, 0),
        sources=[
            MemorySource(
                memory_id="m1",
                content="we use FastAPI",
                layer="project",
                source_type="explicit",
                confidence=1.0,
                match_reason="keyword match",
                trace_path=["root", "project"],
            )
        ],
        total_memories_searched=5,
        retrieval_method="keyword",
        tree_version=2,
    )
    d = rp.to_dict()
    assert d["query"] == "what is our stack?"
    assert d["timestamp"] == "2024-01-01T12:00:00"
    assert d["total_memories_searched"] == 5
    assert d["retrieval_method"] == "keyword"
    assert d["tree_version"] == 2
    assert d["avg_confidence"] == 1.0
    assert d["layer_breakdown"] == {"project": 1}
    assert d["source_type_breakdown"] == {"explicit": 1}
    assert len(d["sources"]) == 1
    assert d["sources"][0]["memory_id"] == "m1"


# ---------------------------------------------------------------------------
# ProvenanceTracker
# ---------------------------------------------------------------------------


def test_record_retrieval():
    """record_retrieval should create and store a RetrievalProvenance record."""
    tracker = ProvenanceTracker()
    results = [
        {
            "id": "m1",
            "content": "we use FastAPI",
            "layer": "project",
            "source": "explicit",
            "confidence": 1.0,
        },
        {
            "id": "m2",
            "content": "we use PostgreSQL",
            "layer": "project",
            "source": "explicit",
            "confidence": 0.9,
        },
    ]
    prov = tracker.record_retrieval(
        query="what is our stack?",
        results=results,
        method="keyword",
        tree_version=1,
        total_memories_searched=10,
    )
    assert prov.query == "what is our stack?"
    assert prov.retrieval_method == "keyword"
    assert prov.tree_version == 1
    assert prov.total_memories_searched == 10
    assert len(prov.sources) == 2
    assert prov.sources[0].memory_id == "m1"
    assert prov.sources[1].memory_id == "m2"
    assert prov.avg_confidence == 0.95


def test_record_retrieval_default_match_reason():
    """When no match_reasons dict is provided, a default reason should be used."""
    tracker = ProvenanceTracker()
    results = [
        {
            "id": "m1",
            "content": "test content",
            "layer": "session",
            "source": "inferred",
            "confidence": 0.7,
        },
    ]
    prov = tracker.record_retrieval(
        query="test",
        results=results,
        method="llm",
        tree_version=1,
    )
    assert prov.sources[0].match_reason == "matched via llm retrieval"


def test_record_retrieval_with_custom_trace_paths_and_reasons():
    """Custom trace_paths and match_reasons should be applied per memory."""
    tracker = ProvenanceTracker()
    results = [
        {
            "id": "m1",
            "content": "test",
            "layer": "project",
            "source": "explicit",
            "confidence": 1.0,
        },
    ]
    prov = tracker.record_retrieval(
        query="test",
        results=results,
        method="keyword",
        tree_version=1,
        trace_paths={"m1": ["root", "project", "tech_stack"]},
        match_reasons={"m1": "exact keyword match on 'test'"},
    )
    assert prov.sources[0].trace_path == ["root", "project", "tech_stack"]
    assert prov.sources[0].match_reason == "exact keyword match on 'test'"


def test_get_history():
    """get_history should return all recorded provenance records."""
    tracker = ProvenanceTracker()
    tracker.record_retrieval(query="q1", results=[], method="keyword", tree_version=1)
    tracker.record_retrieval(query="q2", results=[], method="llm", tree_version=1)
    history = tracker.get_history()
    assert len(history) == 2
    assert history[0].query == "q1"
    assert history[1].query == "q2"


def test_get_provenance_report_all():
    """get_provenance_report with no query should aggregate across all records."""
    tracker = ProvenanceTracker()
    tracker.record_retrieval(
        query="q1",
        results=[
            {
                "id": "m1",
                "content": "c1",
                "layer": "project",
                "source": "explicit",
                "confidence": 1.0,
            }
        ],
        method="keyword",
        tree_version=1,
    )
    tracker.record_retrieval(
        query="q2",
        results=[
            {
                "id": "m2",
                "content": "c2",
                "layer": "session",
                "source": "inferred",
                "confidence": 0.5,
            }
        ],
        method="llm",
        tree_version=1,
    )
    report = tracker.get_provenance_report()
    assert report["retrieval_count"] == 2
    assert report["avg_confidence"] == 0.75
    assert report["layer_breakdown"] == {"project": 1, "session": 1}
    assert report["source_type_breakdown"] == {"explicit": 1, "inferred": 1}
    assert report["method_breakdown"] == {"keyword": 1, "llm": 1}


def test_get_provenance_report_specific_query():
    """get_provenance_report with a query should filter to that query."""
    tracker = ProvenanceTracker()
    tracker.record_retrieval(
        query="q1",
        results=[
            {
                "id": "m1",
                "content": "c1",
                "layer": "project",
                "source": "explicit",
                "confidence": 1.0,
            }
        ],
        method="keyword",
        tree_version=1,
    )
    tracker.record_retrieval(
        query="q2",
        results=[
            {
                "id": "m2",
                "content": "c2",
                "layer": "session",
                "source": "inferred",
                "confidence": 0.5,
            }
        ],
        method="llm",
        tree_version=1,
    )
    report = tracker.get_provenance_report(query="q1")
    assert report["retrieval_count"] == 1
    assert report["avg_confidence"] == 1.0
    assert report["layer_breakdown"] == {"project": 1}


def test_get_provenance_report_no_records():
    """get_provenance_report should return empty stats when no records exist."""
    tracker = ProvenanceTracker()
    report = tracker.get_provenance_report()
    assert report["retrieval_count"] == 0
    assert report["avg_confidence"] == 0.0
    assert report["sources"] == []


# ---------------------------------------------------------------------------
# Anomaly detection
# ---------------------------------------------------------------------------


def test_detect_low_confidence_retrievals():
    """Detect retrievals where average confidence is below threshold."""
    tracker = ProvenanceTracker()
    tracker.record_retrieval(
        query="q1",
        results=[
            {
                "id": "m1",
                "content": "c1",
                "layer": "session",
                "source": "inferred",
                "confidence": 0.3,
            },
            {
                "id": "m2",
                "content": "c2",
                "layer": "session",
                "source": "inferred",
                "confidence": 0.4,
            },
        ],
        method="keyword",
        tree_version=1,
    )
    tracker.record_retrieval(
        query="q2",
        results=[
            {
                "id": "m3",
                "content": "c3",
                "layer": "project",
                "source": "explicit",
                "confidence": 1.0,
            }
        ],
        method="keyword",
        tree_version=1,
    )
    low = tracker.detect_low_confidence_retrievals(threshold=0.5)
    assert len(low) == 1
    assert low[0].query == "q1"


def test_detect_low_confidence_retrievals_empty():
    """When no low-confidence retrievals exist, return empty list."""
    tracker = ProvenanceTracker()
    tracker.record_retrieval(
        query="q1",
        results=[
            {
                "id": "m1",
                "content": "c1",
                "layer": "project",
                "source": "explicit",
                "confidence": 1.0,
            }
        ],
        method="keyword",
        tree_version=1,
    )
    low = tracker.detect_low_confidence_retrievals(threshold=0.5)
    assert low == []


def test_detect_source_type_imbalance():
    """Detect when one source type dominates retrieval results."""
    tracker = ProvenanceTracker()
    # Create 9 inferred out of 10 total sources (90% > 0.9 threshold would trigger,
    # but default threshold is 0.9, so we need > 0.9)
    for i in range(9):
        tracker.record_retrieval(
            query=f"q{i}",
            results=[
                {
                    "id": f"m{i}",
                    "content": f"c{i}",
                    "layer": "session",
                    "source": "inferred",
                    "confidence": 0.7,
                }
            ],
            method="keyword",
            tree_version=1,
        )
    # Add 1 explicit to make ratio 9/10 = 0.9 — this does NOT exceed 0.9 threshold
    tracker.record_retrieval(
        query="q_explicit",
        results=[
            {
                "id": "m_explicit",
                "content": "explicit content",
                "layer": "project",
                "source": "explicit",
                "confidence": 1.0,
            }
        ],
        method="keyword",
        tree_version=1,
    )
    # 9/10 = 0.9, not > 0.9, so no imbalance
    imbalance = tracker.detect_source_type_imbalance(ratio_threshold=0.9)
    assert imbalance is None

    # Now add one more inferred to make it 10/11 = 0.909 > 0.9
    tracker.record_retrieval(
        query="q_extra",
        results=[
            {
                "id": "m_extra",
                "content": "extra inferred",
                "layer": "session",
                "source": "inferred",
                "confidence": 0.7,
            }
        ],
        method="keyword",
        tree_version=1,
    )
    imbalance = tracker.detect_source_type_imbalance(ratio_threshold=0.9)
    assert imbalance is not None
    assert imbalance["source_type"] == "inferred"
    assert imbalance["ratio"] > 0.9


def test_detect_source_type_imbalance_none():
    """When history is empty, detect_source_type_imbalance should return None."""
    tracker = ProvenanceTracker()
    assert tracker.detect_source_type_imbalance() is None


def test_detect_source_type_imbalance_balanced():
    """When sources are well-balanced, no imbalance should be detected."""
    tracker = ProvenanceTracker()
    for _ in range(5):
        tracker.record_retrieval(
            query="q1",
            results=[
                {
                    "id": "m1",
                    "content": "c1",
                    "layer": "project",
                    "source": "explicit",
                    "confidence": 1.0,
                },
                {
                    "id": "m2",
                    "content": "c2",
                    "layer": "session",
                    "source": "inferred",
                    "confidence": 0.7,
                },
            ],
            method="keyword",
            tree_version=1,
        )
    # 50/50 split — no imbalance
    imbalance = tracker.detect_source_type_imbalance(ratio_threshold=0.9)
    assert imbalance is None


# ---------------------------------------------------------------------------
# History management
# ---------------------------------------------------------------------------


def test_max_history_limit():
    """ProvenanceTracker should discard old records when max_history is exceeded."""
    tracker = ProvenanceTracker(max_history=3)
    for i in range(5):
        tracker.record_retrieval(
            query=f"q{i}",
            results=[],
            method="keyword",
            tree_version=1,
        )
    history = tracker.get_history()
    assert len(history) == 3
    # Should keep the most recent 3
    assert history[0].query == "q2"
    assert history[1].query == "q3"
    assert history[2].query == "q4"


def test_max_history_exact():
    """When history equals max_history, all records should be kept."""
    tracker = ProvenanceTracker(max_history=3)
    for i in range(3):
        tracker.record_retrieval(
            query=f"q{i}",
            results=[],
            method="keyword",
            tree_version=1,
        )
    history = tracker.get_history()
    assert len(history) == 3
    assert history[0].query == "q0"
    assert history[1].query == "q1"
    assert history[2].query == "q2"


# ---------------------------------------------------------------------------
# Empty retrieval handling
# ---------------------------------------------------------------------------


def test_empty_results():
    """Recording an empty result list should create provenance with no sources."""
    tracker = ProvenanceTracker()
    prov = tracker.record_retrieval(
        query="test",
        results=[],
        method="keyword",
        tree_version=1,
        total_memories_searched=100,
    )
    assert prov.sources == []
    assert prov.avg_confidence == 0.0
    assert prov.layer_breakdown == {}
    assert prov.source_type_breakdown == {}
    assert prov.total_memories_searched == 100


def test_empty_retrieval_low_confidence():
    """Empty retrievals have avg_confidence 0.0 and should be flagged as low."""
    tracker = ProvenanceTracker()
    tracker.record_retrieval(query="test", results=[], method="keyword", tree_version=1)
    low = tracker.detect_low_confidence_retrievals(threshold=0.1)
    assert len(low) == 1


# ---------------------------------------------------------------------------
# Clear
# ---------------------------------------------------------------------------


def test_clear_history():
    """clear should remove all history records."""
    tracker = ProvenanceTracker()
    tracker.record_retrieval(query="q1", results=[], method="keyword", tree_version=1)
    assert len(tracker.get_history()) == 1
    tracker.clear()
    assert tracker.get_history() == []


# ---------------------------------------------------------------------------
# RetrievalProvenance edge cases
# ---------------------------------------------------------------------------


def test_single_source_avg_confidence():
    """avg_confidence with a single source should equal that source's confidence."""
    from datetime import datetime

    rp = RetrievalProvenance(
        query="test",
        timestamp=datetime.now(),
        sources=[
            MemorySource(
                memory_id="m1",
                content="c1",
                layer="project",
                source_type="explicit",
                confidence=0.42,
                match_reason="test",
            )
        ],
    )
    assert rp.avg_confidence == 0.42


def test_layer_breakdown_empty():
    """layer_breakdown should be empty when no sources exist."""
    from datetime import datetime

    rp = RetrievalProvenance(query="test", timestamp=datetime.now(), sources=[])
    assert rp.layer_breakdown == {}


def test_source_type_breakdown_empty():
    """source_type_breakdown should be empty when no sources exist."""
    from datetime import datetime

    rp = RetrievalProvenance(query="test", timestamp=datetime.now(), sources=[])
    assert rp.source_type_breakdown == {}


# ---------------------------------------------------------------------------
# to_dict round-trip integrity
# ---------------------------------------------------------------------------


def test_to_dict_contains_all_sources():
    """to_dict should include all sources, not just the first."""
    from datetime import datetime

    rp = RetrievalProvenance(
        query="test",
        timestamp=datetime(2024, 6, 15, 10, 30, 0),
        sources=[
            MemorySource(
                memory_id="m1",
                content="c1",
                layer="project",
                source_type="explicit",
                confidence=1.0,
                match_reason="r1",
            ),
            MemorySource(
                memory_id="m2",
                content="c2",
                layer="session",
                source_type="inferred",
                confidence=0.6,
                match_reason="r2",
            ),
            MemorySource(
                memory_id="m3",
                content="c3",
                layer="user",
                source_type="mentioned",
                confidence=0.5,
                match_reason="r3",
            ),
        ],
        total_memories_searched=20,
        retrieval_method="hybrid",
        tree_version=3,
    )
    d = rp.to_dict()
    assert len(d["sources"]) == 3
    assert d["avg_confidence"] == pytest.approx(0.7)
    assert d["layer_breakdown"] == {"project": 1, "session": 1, "user": 1}
    assert d["source_type_breakdown"] == {"explicit": 1, "inferred": 1, "mentioned": 1}
    assert d["total_memories_searched"] == 20
    assert d["retrieval_method"] == "hybrid"
    assert d["tree_version"] == 3


# ---------------------------------------------------------------------------
# SQLite persistence
# ---------------------------------------------------------------------------


def test_provenance_tracker_persists_to_store():
    import os
    import tempfile

    from memctrl.store import MemoryStore

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    store = MemoryStore(db_path)
    tracker = ProvenanceTracker(store=store, persist=True, max_history=10)

    tracker.record_retrieval(
        query="what is our stack?",
        results=[
            {
                "id": "m1",
                "content": "we use FastAPI",
                "layer": "project",
                "source": "explicit",
                "confidence": 1.0,
            }
        ],
        method="keyword",
        tree_version=2,
    )

    # In-memory history
    assert len(tracker.get_history()) == 1

    # SQLite persistence
    records = store.get_provenance(limit=10)
    assert len(records) == 1
    assert records[0]["query"] == "what is our stack?"

    os.unlink(db_path)


def test_provenance_tracker_persist_false_skips_db():
    import os
    import tempfile

    from memctrl.store import MemoryStore

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    store = MemoryStore(db_path)
    tracker = ProvenanceTracker(store=store, persist=False, max_history=10)

    tracker.record_retrieval(
        query="q",
        results=[
            {
                "id": "m1",
                "content": "c",
                "layer": "project",
                "source": "explicit",
                "confidence": 1.0,
            }
        ],
        method="keyword",
        tree_version=1,
    )

    records = store.get_provenance(limit=10)
    assert len(records) == 0

    os.unlink(db_path)
