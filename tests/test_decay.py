"""Tests for ConfidenceDecay -- MemCtrl confidence decay system.

Covers: decay rules per layer, explicit memory protection, floor clamping,
flagged memory detection, reinforcement, and edge cases.
"""

import os
import tempfile

import pytest

from memctrl.store import MemoryStore
from memctrl.decay import ConfidenceDecay, DECAY_RULES


@pytest.fixture
def store():
    """Create a temporary MemoryStore for each test."""
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    s = MemoryStore(db_path)
    yield s
    s.close()
    os.unlink(db_path)


@pytest.fixture
def decay(store):
    """Create a ConfidenceDecay instance wired to the temporary store."""
    return ConfidenceDecay(store)


# ---------------------------------------------------------------------------
# Decay rules constants
# ---------------------------------------------------------------------------


def test_decay_rules_structure():
    """DECAY_RULES must contain the three standard layers with rate and floor."""
    assert "project" in DECAY_RULES
    assert "session" in DECAY_RULES
    assert "user" in DECAY_RULES
    for layer, rule in DECAY_RULES.items():
        assert "rate" in rule
        assert "floor" in rule


def test_project_layer_never_decays():
    """Project layer has rate=0.0 and floor=1.0, meaning no decay ever."""
    assert DECAY_RULES["project"]["rate"] == 0.0
    assert DECAY_RULES["project"]["floor"] == 1.0


def test_session_layer_fast_decay():
    """Session layer has the highest decay rate."""
    assert DECAY_RULES["session"]["rate"] == 0.05
    assert DECAY_RULES["session"]["floor"] == 0.3


def test_user_layer_slow_decay():
    """User layer has a slower decay rate than session."""
    assert DECAY_RULES["user"]["rate"] == 0.01
    assert DECAY_RULES["user"]["floor"] == 0.5


# ---------------------------------------------------------------------------
# Decay application per layer
# ---------------------------------------------------------------------------


def test_explicit_memory_never_decays(decay, store):
    """Memories with confidence=1.0 (explicit) must never be affected by decay."""
    mid = store.insert_memory("session", "explicit fact", "test", confidence=1.0)
    affected = decay.decay_memories(days_elapsed=10)
    mem = store.get_memory(mid)
    assert mem.confidence == 1.0
    assert len(affected) == 0


def test_explicit_memory_in_project_never_decays(decay, store):
    """Project-layer explicit memories must be completely immune."""
    mid = store.insert_memory(
        "project", "ADR-001: we use FastAPI", "test", confidence=1.0
    )
    affected = decay.decay_memories(days_elapsed=100)
    mem = store.get_memory(mid)
    assert mem.confidence == 1.0
    assert len(affected) == 0


def test_inferred_session_memory_decays(decay, store):
    """Session-layer inferred (0.7) memories must decay at the session rate."""
    mid = store.insert_memory(
        "session", "inferred session fact", "test", confidence=0.7
    )
    affected = decay.decay_memories(days_elapsed=1)
    mem = store.get_memory(mid)
    expected = 0.7 * (1.0 - 0.05)  # 0.665
    assert abs(mem.confidence - expected) < 1e-9
    assert len(affected) == 1
    assert affected[0]["memory_id"] == mid
    assert affected[0]["old_confidence"] == 0.7
    assert abs(affected[0]["new_confidence"] - expected) < 1e-9


def test_inferred_user_memory_decays(decay, store):
    """User-layer inferred (0.7) memories must decay at the slower user rate."""
    mid = store.insert_memory(
        "user", "inferred user preference", "test", confidence=0.7
    )
    affected = decay.decay_memories(days_elapsed=1)
    mem = store.get_memory(mid)
    expected = 0.7 * (1.0 - 0.01)  # 0.693
    assert abs(mem.confidence - expected) < 1e-9
    assert len(affected) == 1


def test_mentioned_memory_decays(decay, store):
    """Mentioned (0.5) memories must also decay."""
    mid = store.insert_memory(
        "session", "maybe we should try X", "test", confidence=0.5
    )
    affected = decay.decay_memories(days_elapsed=1)
    mem = store.get_memory(mid)
    expected = 0.5 * (1.0 - 0.05)  # 0.475
    assert abs(mem.confidence - expected) < 1e-9
    assert len(affected) == 1


def test_project_inferred_memory_never_decays(decay, store):
    """Project layer has rate=0.0, so even inferred memories should not decay."""
    mid = store.insert_memory(
        "project", "inferred project fact", "test", confidence=0.7
    )
    affected = decay.decay_memories(days_elapsed=10)
    mem = store.get_memory(mid)
    assert mem.confidence == 0.7
    assert len(affected) == 0


def test_multi_day_decay(decay, store):
    """Decay over multiple days should be exponential, not linear."""
    mid = store.insert_memory("session", "multi-day fact", "test", confidence=0.7)
    decay.decay_memories(days_elapsed=5)
    mem = store.get_memory(mid)
    expected = 0.7 * ((1.0 - 0.05) ** 5)
    assert abs(mem.confidence - expected) < 1e-9


def test_multiple_memories_decay_together(decay, store):
    """decay_memories should process all eligible memories in one call."""
    m1 = store.insert_memory("session", "fact one", "test", confidence=0.7)
    m2 = store.insert_memory("session", "fact two", "test", confidence=0.6)
    m3 = store.insert_memory("user", "fact three", "test", confidence=0.7)
    # explicit -- should not decay
    m4 = store.insert_memory("session", "explicit fact", "test", confidence=1.0)

    affected = decay.decay_memories(days_elapsed=1)
    assert len(affected) == 3

    affected_ids = {a["memory_id"] for a in affected}
    assert m1 in affected_ids
    assert m2 in affected_ids
    assert m3 in affected_ids
    assert m4 not in affected_ids

    assert abs(store.get_memory(m1).confidence - 0.7 * 0.95) < 1e-9
    assert abs(store.get_memory(m2).confidence - 0.6 * 0.95) < 1e-9
    assert abs(store.get_memory(m3).confidence - 0.7 * 0.99) < 1e-9


# ---------------------------------------------------------------------------
# Floor clamping
# ---------------------------------------------------------------------------


def test_session_floor_is_respected(decay, store):
    """Session confidence must never drop below floor (0.3)."""
    store.insert_memory("session", "will decay a lot", "test", confidence=0.4)
    # After many days, confidence would be < 0.3 without floor
    decay.decay_memories(days_elapsed=100)
    mem = store.list_memories("session")[0]
    assert mem.confidence >= 0.3
    assert mem.confidence == pytest.approx(0.3, abs=1e-9)


def test_user_floor_is_respected(decay, store):
    """User confidence must never drop below floor (0.5)."""
    store.insert_memory("user", "will decay a lot", "test", confidence=0.6)
    decay.decay_memories(days_elapsed=100)
    mem = store.list_memories("user")[0]
    assert mem.confidence >= 0.5
    assert mem.confidence == pytest.approx(0.5, abs=1e-9)


def test_floor_exact_boundary(decay, store):
    """If decay would exactly hit the floor, it should stop there."""
    # 0.7 * (0.95 ^ n) -> find n where it crosses 0.3
    # This is a boundary test: start at 0.31, decay heavily
    store.insert_memory("session", "near floor", "test", confidence=0.31)
    decay.decay_memories(days_elapsed=1)
    mem = store.list_memories("session")[0]
    # Should be max(0.31 * 0.95, 0.3) = max(0.2945, 0.3) = 0.3
    assert mem.confidence == 0.3


# ---------------------------------------------------------------------------
# Flagged memories
# ---------------------------------------------------------------------------


def test_get_flagged_memories_empty_when_all_above_floor(decay, store):
    """No memories should be flagged when all are above their layer floor."""
    store.insert_memory("session", "above floor", "test", confidence=0.7)
    store.insert_memory("user", "above floor", "test", confidence=0.7)
    flagged = decay.get_flagged_memories()
    assert len(flagged) == 0


def test_get_flagged_memories_finds_below_floor(decay, store):
    """Memories below floor should be flagged for review."""
    # Insert a memory with confidence already below the session floor
    mid = store.insert_memory("session", "below floor", "test", confidence=0.25)
    flagged = decay.get_flagged_memories()
    assert len(flagged) == 1
    assert flagged[0].id == mid


def test_get_flagged_memories_at_floor_not_flagged(decay, store):
    """Memories exactly at the floor should NOT be flagged (only below)."""
    # Insert a memory with confidence exactly at the session floor
    store.insert_memory("session", "exactly at floor", "test", confidence=0.3)
    flagged = decay.get_flagged_memories()
    assert len(flagged) == 0


def test_get_flagged_memories_with_floor_override(decay, store):
    """floor_override lets us raise the threshold for emergency review."""
    store.insert_memory("session", "at 0.6", "test", confidence=0.6)
    store.insert_memory("session", "at 0.4", "test", confidence=0.4)
    # With floor_override=0.5, only the 0.4 one should be flagged
    flagged = decay.get_flagged_memories(floor_override=0.5)
    assert len(flagged) == 1
    assert flagged[0].confidence == 0.4


def test_get_flagged_memories_with_high_override(decay, store):
    """A high floor_override should flag many memories."""
    store.insert_memory("session", "at 0.7", "test", confidence=0.7)
    store.insert_memory("user", "at 0.8", "test", confidence=0.8)
    # Floor override of 0.75 flags the 0.7 session memory
    flagged = decay.get_flagged_memories(floor_override=0.75)
    assert len(flagged) == 1
    assert flagged[0].confidence == 0.7


def test_get_flagged_memories_respects_layer_rules(decay, store):
    """Different layers have different floors; only the right ones flag."""
    # User floor is 0.5, session floor is 0.3
    store.insert_memory("user", "user at 0.45", "test", confidence=0.45)
    store.insert_memory("session", "session at 0.45", "test", confidence=0.45)
    flagged = decay.get_flagged_memories()
    # User memory at 0.45 < 0.5 floor -> flagged
    # Session memory at 0.45 > 0.3 floor -> NOT flagged
    assert len(flagged) == 1
    assert flagged[0].layer == "user"


# ---------------------------------------------------------------------------
# Reinforcement
# ---------------------------------------------------------------------------


def test_reinforce_memory_increases_confidence(decay, store):
    """Reinforcement should boost confidence by the default amount (0.1)."""
    mid = store.insert_memory("session", "inferred fact", "test", confidence=0.6)
    result = decay.reinforce_memory(mid)
    assert result is True
    mem = store.get_memory(mid)
    assert abs(mem.confidence - 0.7) < 1e-9


def test_reinforce_memory_capped_at_1_0(decay, store):
    """Reinforcement must never push confidence above 1.0."""
    mid = store.insert_memory("session", "already high", "test", confidence=0.96)
    result = decay.reinforce_memory(mid, amount=0.1)
    assert result is True
    mem = store.get_memory(mid)
    assert mem.confidence == 1.0


def test_reinforce_explicit_memory_stays_at_1_0(decay, store):
    """Reinforcing an already-explicit memory should be a no-op."""
    mid = store.insert_memory("project", "explicit fact", "test", confidence=1.0)
    result = decay.reinforce_memory(mid)
    assert result is True
    mem = store.get_memory(mid)
    assert mem.confidence == 1.0


def test_reinforce_memory_updates_timestamp(decay, store):
    """Reinforcement should update the memory's created_at timestamp."""
    mid = store.insert_memory("session", "old fact", "test", confidence=0.6)
    old_mem = store.get_memory(mid)
    old_ts = old_mem.created_at

    # Small sleep to ensure timestamp changes
    import time

    time.sleep(0.01)

    decay.reinforce_memory(mid)
    new_mem = store.get_memory(mid)
    assert new_mem.created_at > old_ts


def test_reinforce_missing_memory_returns_false(decay):
    """Reinforcing a nonexistent memory should return False."""
    result = decay.reinforce_memory("nonexistent-id")
    assert result is False


def test_reinforce_memory_custom_amount(decay, store):
    """Reinforcement amount should be configurable."""
    mid = store.insert_memory("session", "inferred fact", "test", confidence=0.5)
    decay.reinforce_memory(mid, amount=0.15)
    mem = store.get_memory(mid)
    assert abs(mem.confidence - 0.65) < 1e-9


# ---------------------------------------------------------------------------
# Edge cases
# ---------------------------------------------------------------------------


def test_decay_zero_days_does_nothing(decay, store):
    """With days_elapsed=0, no decay should be applied."""
    mid = store.insert_memory("session", "inferred fact", "test", confidence=0.7)
    affected = decay.decay_memories(days_elapsed=0)
    mem = store.get_memory(mid)
    assert mem.confidence == 0.7
    assert len(affected) == 0


def test_decay_negative_days_does_nothing(decay, store):
    """With negative days_elapsed, no decay should be applied."""
    mid = store.insert_memory("session", "inferred fact", "test", confidence=0.7)
    affected = decay.decay_memories(days_elapsed=-5)
    mem = store.get_memory(mid)
    assert mem.confidence == 0.7
    assert len(affected) == 0


def test_empty_store(decay):
    """Decay on an empty store should return an empty list."""
    affected = decay.decay_memories(days_elapsed=10)
    assert affected == []
    flagged = decay.get_flagged_memories()
    assert flagged == []


def test_only_explicit_memories_no_decay(decay, store):
    """A store with only explicit memories should have nothing to decay."""
    store.insert_memory("project", "explicit one", "test", confidence=1.0)
    store.insert_memory("session", "explicit two", "test", confidence=1.0)
    store.insert_memory("user", "explicit three", "test", confidence=1.0)
    affected = decay.decay_memories(days_elapsed=100)
    assert len(affected) == 0


def test_custom_rules_override(decay, store):
    """Custom rules passed to __init__ should be respected."""
    custom_rules = {
        "session": {"rate": 0.1, "floor": 0.2},
    }
    custom_decay = ConfidenceDecay(store, rules=custom_rules)
    mid = store.insert_memory("session", "custom decay", "test", confidence=0.7)
    custom_decay.decay_memories(days_elapsed=1)
    mem = store.get_memory(mid)
    expected = 0.7 * (1.0 - 0.1)  # 0.63
    assert abs(mem.confidence - expected) < 1e-9


def test_unknown_layer_uses_safe_defaults(decay, store):
    """Layers not in DECAY_RULES should default to no decay (safe fallback)."""
    mid = store.insert_memory("unknown_layer", "orphan fact", "test", confidence=0.7)
    affected = decay.decay_memories(days_elapsed=10)
    mem = store.get_memory(mid)
    assert mem.confidence == 0.7
    assert len(affected) == 0


def test_decay_preserves_memory_content(decay, store):
    """Decay should only change confidence, never content or other fields."""
    mid = store.insert_memory(
        "session",
        "important content",
        "test_source",
        confidence=0.7,
        tags=["important"],
    )
    decay.decay_memories(days_elapsed=1)
    mem = store.get_memory(mid)
    assert mem.content == "important content"
    assert mem.source == "test_source"
    assert mem.tags == ["important"]
    assert mem.layer == "session"


def test_store_methods_integration(store):
    """The store methods update_memory_confidence, get_memories_below_confidence,
    and update_memory_timestamp should work correctly."""
    mid = store.insert_memory("session", "test fact", "test", confidence=0.7)

    # update_memory_confidence
    result = store.update_memory_confidence(mid, 0.55)
    assert result is True
    mem = store.get_memory(mid)
    assert mem.confidence == 0.55

    # get_memories_below_confidence
    below = store.get_memories_below_confidence(0.6)
    assert len(below) == 1
    assert below[0].id == mid

    below_none = store.get_memories_below_confidence(0.5)
    assert len(below_none) == 0

    # update_memory_timestamp
    old_ts = mem.created_at
    import time

    time.sleep(0.01)
    result = store.update_memory_timestamp(mid)
    assert result is True
    mem = store.get_memory(mid)
    assert mem.created_at > old_ts


def test_store_update_confidence_missing(store):
    """update_memory_confidence should return False for missing memory."""
    assert store.update_memory_confidence("nonexistent-id", 0.5) is False


def test_store_get_below_confidence_by_layer(store):
    """get_memories_below_confidence with layer filter should work."""
    m1 = store.insert_memory("session", "session low", "test", confidence=0.4)
    m2 = store.insert_memory("user", "user low", "test", confidence=0.4)

    session_below = store.get_memories_below_confidence(0.5, layer="session")
    assert len(session_below) == 1
    assert session_below[0].id == m1

    user_below = store.get_memories_below_confidence(0.5, layer="user")
    assert len(user_below) == 1
    assert user_below[0].id == m2


def test_store_update_timestamp_missing(store):
    """update_memory_timestamp should return False for missing memory."""
    assert store.update_memory_timestamp("nonexistent-id") is False


def test_repeated_decay_accumulates(decay, store):
    """Calling decay multiple times should accumulate correctly."""
    mid = store.insert_memory("session", "decay me", "test", confidence=0.7)
    decay.decay_memories(days_elapsed=1)
    decay.decay_memories(days_elapsed=1)
    mem = store.get_memory(mid)
    expected = 0.7 * ((1.0 - 0.05) ** 2)
    assert abs(mem.confidence - expected) < 1e-9


def test_reinforce_then_decay(decay, store):
    """Reinforcement should protect against subsequent decay."""
    mid = store.insert_memory("session", "inferred", "test", confidence=0.6)
    decay.reinforce_memory(mid, amount=0.1)
    mem_after_reinforce = store.get_memory(mid)
    assert abs(mem_after_reinforce.confidence - 0.7) < 1e-9

    decay.decay_memories(days_elapsed=1)
    mem_after_decay = store.get_memory(mid)
    expected = 0.7 * (1.0 - 0.05)
    assert abs(mem_after_decay.confidence - expected) < 1e-9


def test_decay_affected_report_format(decay, store):
    """Each entry in the affected list must have the expected keys."""
    mid = store.insert_memory("session", "test", "test", confidence=0.7)
    affected = decay.decay_memories(days_elapsed=1)
    assert len(affected) == 1
    entry = affected[0]
    assert "memory_id" in entry
    assert "old_confidence" in entry
    assert "new_confidence" in entry
    assert "layer" in entry
    assert entry["memory_id"] == mid
    assert entry["layer"] == "session"
