"""Tests for Phase 2: Minimal Knowledge Lifecycle, Relations, and Evidence References."""

import sqlite3
import pytest
from datetime import datetime, timedelta

from memctrl.store import (
    CLAIM_TYPES,
    LIFECYCLE_STATES,
    VERIFICATION_STATES,
    RELATION_TYPES,
    MemoryStore,
    MemoryRelation,
    MemoryEvidence,
)


@pytest.fixture
def store(tmp_path):
    db = tmp_path / "lifecycle.db"
    return MemoryStore(str(db))


def test_default_lifecycle_fields(store):
    """A standard inserted memory defaults to assertion, accepted, unverified."""
    mid = store.insert_memory("project", "Default lifecycle fact", source="manual")
    mem = store.get_memory(mid)
    assert mem is not None
    assert mem.claim_type == "assertion"
    assert mem.lifecycle_state == "accepted"
    assert mem.verification_state == "unverified"
    assert mem.observed_at is None
    assert mem.valid_from is None
    assert mem.valid_until is None


def test_independent_lifecycle_dimensions(store):
    """Claim type, lifecycle state, and verification state are independent dimensions."""
    now = datetime.now()
    mid = store.insert_memory(
        layer="project",
        content="Redis caused the latency spike",
        source="investigation",
        confidence=0.7,
        claim_type="hypothesis",
        lifecycle_state="accepted",
        verification_state="disputed",
        observed_at=now,
    )
    mem = store.get_memory(mid)
    assert mem is not None
    assert mem.claim_type == "hypothesis"
    assert mem.lifecycle_state == "accepted"
    assert mem.verification_state == "disputed"
    assert mem.observed_at is not None


def test_invalid_lifecycle_values_rejected(store):
    """Unknown enum values must not be accepted."""
    with pytest.raises(ValueError, match="Invalid claim_type"):
        store.insert_memory(
            "project", "Invalid fact", claim_type="magical_truth"
        )

    with pytest.raises(ValueError, match="Invalid lifecycle_state"):
        store.insert_memory(
            "project", "Invalid fact", lifecycle_state="super_accepted"
        )

    with pytest.raises(ValueError, match="Invalid verification_state"):
        store.insert_memory(
            "project", "Invalid fact", verification_state="absolute_certainty"
        )


def test_v2_to_v4_migration_preserves_semantics(tmp_path):
    """Legacy v2 memories must become accepted + unverified (never verified)."""
    db_file = tmp_path / "legacy_v2.db"
    with sqlite3.connect(str(db_file)) as conn:
        conn.executescript(
            """
            CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
            INSERT INTO schema_version (version) VALUES (2);

            CREATE TABLE memories (
                id          TEXT PRIMARY KEY,
                layer       TEXT NOT NULL,
                content     TEXT NOT NULL,
                source      TEXT,
                confidence  REAL DEFAULT 1.0,
                created_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                expires_at  TIMESTAMP,
                tags        TEXT
            );

            INSERT INTO memories (id, layer, content, source, confidence, created_at, expires_at, tags)
            VALUES ('leg-1', 'project', 'Legacy fact', 'manual', 1.0, '2025-01-01T00:00:00', NULL, '[]');
            """
        )
        conn.commit()

    store = MemoryStore(str(db_file))
    mem = store.get_memory("leg-1")
    assert mem is not None
    assert mem.confidence == 1.0
    # Crucial: confidence=1.0 does NOT mean verified!
    assert mem.lifecycle_state == "accepted"
    assert mem.verification_state == "unverified"
    assert mem.claim_type == "assertion"


def test_memory_relations_lineage(store):
    """Memory relations track lineage (e.g. derived_from, supports, contradicts)."""
    m1 = store.insert_memory("session", "Observed timeout on port 5432", claim_type="observation")
    m2 = store.insert_memory("project", "Postgres pool exhausted", claim_type="derived_lesson")

    rel_id = store.add_memory_relation(
        from_memory_id=m2,
        to_memory_id=m1,
        relation_type="derived_from",
        metadata={"reason": "session observation distilled into lesson"},
    )
    assert rel_id is not None

    relations_from = store.get_memory_relations(m2, direction="outgoing")
    assert len(relations_from) == 1
    assert relations_from[0].relation_type == "derived_from"
    assert relations_from[0].to_memory_id == m1

    relations_to = store.get_memory_relations(m1, direction="incoming")
    assert len(relations_to) == 1
    assert relations_to[0].from_memory_id == m2


def test_supersedes_vs_refutes_distinction(store):
    """Temporal change (supersedes) and falsification (refutes) are distinct."""
    m_old = store.insert_memory("project", "Use Redis for caching", claim_type="decision")
    m_new = store.insert_memory("project", "Use SQLite WAL for caching", claim_type="decision")

    # 1. Supersede (temporal evolution: Redis replaced by SQLite)
    store.supersede_memory(old_memory_id=m_old, new_memory_id=m_new)
    old_reloaded = store.get_memory(m_old)
    assert old_reloaded.lifecycle_state == "superseded"
    assert old_reloaded.verification_state == "unverified"  # Redis wasn't false; it was superseded

    # 2. Refute (falsification: hypothesis proven wrong)
    m_wrong = store.insert_memory("project", "Network switch dropped packets", claim_type="hypothesis")
    store.refute_memory(m_wrong, reason="Packet capture proved switch healthy; server CPU was 100%")
    wrong_reloaded = store.get_memory(m_wrong)
    assert wrong_reloaded.verification_state == "refuted"
    assert wrong_reloaded.lifecycle_state == "rejected"


def test_external_evidence_references(store):
    """Memories can link to external evidence (e.g. Heli task records) without mutating source."""
    mid = store.insert_memory(
        "project",
        "Provider hydration delay causes false 401 loop",
        claim_type="derived_lesson",
    )

    ev_id = store.add_memory_evidence(
        memory_id=mid,
        source_system="heli",
        source_id="task-auth-fix-42",
        source_revision="9431c89",
        relation="derived_from",
        metadata={"measured_latency_ms": 320},
    )
    assert ev_id is not None

    evidence = store.get_memory_evidence(mid)
    assert len(evidence) == 1
    assert evidence[0].source_system == "heli"
    assert evidence[0].source_id == "task-auth-fix-42"
    assert evidence[0].source_revision == "9431c89"
    assert evidence[0].relation == "derived_from"
    assert evidence[0].metadata.get("measured_latency_ms") == 320


def test_self_referencing_guards(store):
    """A memory cannot supersede, refute, or link to itself."""
    m1 = store.insert_memory("project", "Self referential fact")

    with pytest.raises(ValueError, match="cannot supersede itself"):
        store.supersede_memory(m1, m1)

    with pytest.raises(ValueError, match="cannot refute itself"):
        store.refute_memory(m1, reason="I was wrong about myself", refuting_memory_id=m1)

    with pytest.raises(ValueError, match="self-referencing"):
        store.add_memory_relation(m1, m1, "derived_from")


@pytest.mark.asyncio
async def test_all_lifecycle_states_truth_table(store):
    """Verify default retrieval vs history retrieval across all lifecycle states."""
    from memctrl.retriever import MemoryRetriever

    # Create one memory for each lifecycle state
    m_accepted = store.insert_memory("project", "Accepted fact", lifecycle_state="accepted")
    m_candidate = store.insert_memory("project", "Candidate fact", lifecycle_state="candidate")
    m_superseded = store.insert_memory("project", "Superseded fact", lifecycle_state="superseded")
    m_rejected = store.insert_memory("project", "Rejected fact", lifecycle_state="rejected")
    m_archived = store.insert_memory("project", "Archived fact", lifecycle_state="archived")

    # And one refuted memory
    m_refuted = store.insert_memory(
        "project", "Refuted claim", lifecycle_state="accepted", verification_state="refuted"
    )

    # 1. Default retrieval: only accepted + non-refuted is eligible
    default_mems = store.list_memories()
    default_ids = [m.id for m in default_mems]
    assert default_ids == [m_accepted]

    # Retriever default
    lookup = {m.id: m.to_dict() for m in store.list_memories(include_history=True)}
    tree = {"id": "root", "title": "R", "layer": "root", "summary": "", "memory_ids": list(lookup.keys()), "children": []}
    retriever = MemoryRetriever()

    res_default = await retriever.retrieve("fact", tree, memory_lookup=lookup)
    assert len(res_default.facts) == 1
    assert "Accepted fact" in res_default.facts[0]

    # 2. History retrieval: surfaces all states with explicit annotations
    res_history = await retriever.retrieve("fact", tree, memory_lookup=lookup, history=True)
    assert len(res_history.facts) == 5
    annotated = " ".join(res_history.facts)
    assert "[CANDIDATE]" in annotated
    assert "[SUPERSEDED]" in annotated
    assert "[REJECTED]" in annotated
    assert "[ARCHIVED]" in annotated
    assert "Accepted fact" in annotated
