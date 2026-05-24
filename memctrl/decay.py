"""MemCtrl -- Confidence decay system.

Inferred facts (confidence < 1.0) decay over time if not reinforced.
Explicit facts (confidence = 1.0) never decay.

Decay rules by layer:
- project: never decays (rate=0.0, floor=1.0)
- session: fast decay (rate=0.05, floor=0.3)
- user: slow decay (rate=0.01, floor=0.5)

Decay runs on startup + periodically. Memories below floor are flagged
for review, not auto-deleted.

Why this matters: Without decay, inferred facts stay at 0.7 forever,
causing "confidence drift" where stale guesses pollute retrieval results.
Decay ensures only recently-reinforced inferred facts remain competitive
with explicit facts.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Dict, List, Optional

if TYPE_CHECKING:
    from memctrl.store import Memory, MemoryStore

# ---------------------------------------------------------------------------
# Default decay rules per layer
# ---------------------------------------------------------------------------

DECAY_RULES = {
    "project": {"rate": 0.0, "floor": 1.0},
    "session": {"rate": 0.05, "floor": 0.3},
    "user": {"rate": 0.01, "floor": 0.5},
}


# ---------------------------------------------------------------------------
# Confidence decay engine
# ---------------------------------------------------------------------------


class ConfidenceDecay:
    """Manages confidence decay for memories.

    Decay only affects confidence < 1.0 (inferred/mentioned facts).
    Explicit facts (confidence = 1.0) never decay. This ensures that
    user-confirmed memories remain authoritative while heuristic guesses
    naturally fade unless reinforced by successful retrieval.

    The decay formula is multiplicative:
        new_confidence = old_confidence * (1 - rate) ^ days_elapsed

    This produces smooth exponential decay that converges toward the
    layer floor rather than crossing it.
    """

    def __init__(self, store: "MemoryStore", rules: Optional[Dict[str, Dict]] = None):
        """Initialize with a MemoryStore instance and optional custom decay rules.

        Args:
            store: The MemoryStore to operate on.
            rules: Optional override for DECAY_RULES. Must map layer names to
                   dicts with "rate" (float) and "floor" (float) keys.
        """
        self.store = store
        self.rules = rules or DECAY_RULES

    def _get_rule(self, layer: str) -> Dict[str, float]:
        """Look up decay rule for a layer, falling back to no-decay if unknown.

        Unknown layers are treated as project-like (no decay) to avoid
        accidentally degrading confidence on data from future layer types.
        """
        return self.rules.get(layer, {"rate": 0.0, "floor": 1.0})

    def _compute_new_confidence(
        self, current: float, rate: float, floor: float, days: int
    ) -> float:
        """Apply exponential decay formula, clamped to floor.

        We use multiplicative exponential decay because:
        1. It naturally slows down as confidence approaches zero.
        2. It is time-invariant (decaying by 2 days then 3 days = decaying by 5 days).
        3. It keeps relative ordering stable (a 0.7 memory always decays
           slower in absolute terms than a 0.5 memory at the same rate).
        """
        if current >= 1.0 or rate <= 0.0 or days <= 0:
            return current
        decayed = current * ((1.0 - rate) ** days)
        # Clamp to floor so the memory remains retrievable for review.
        return max(decayed, floor)

    def decay_memories(self, days_elapsed: int = 1) -> List[Dict]:
        """Apply decay to all eligible memories.

        Only memories with confidence < 1.0 are decayed. Explicit facts
        (confidence = 1.0) are skipped entirely. After decay, any memory
        that falls below its layer's floor is flagged but NOT deleted --
        the caller decides whether to review, delete, or consolidate.

        Args:
            days_elapsed: Number of days to simulate decay for. Must be >= 0.

        Returns:
            List of dicts with memory_id, old_confidence, new_confidence,
            layer for each affected memory.
        """
        if days_elapsed <= 0:
            return []

        affected: List[Dict] = []
        memories = self.store.list_memories()

        for mem in memories:
            # Skip explicit facts -- they never decay.
            if mem.confidence >= 1.0:
                continue

            rule = self._get_rule(mem.layer)
            rate = rule["rate"]
            floor = rule["floor"]

            # If rate is zero or floor is at explicit level, skip.
            if rate <= 0.0 or floor >= 1.0:
                continue

            new_confidence = self._compute_new_confidence(
                mem.confidence, rate, floor, days_elapsed
            )

            if new_confidence != mem.confidence:
                self.store.update_memory_confidence(mem.id, new_confidence)
                affected.append(
                    {
                        "memory_id": mem.id,
                        "old_confidence": mem.confidence,
                        "new_confidence": new_confidence,
                        "layer": mem.layer,
                    }
                )

        return affected

    def get_flagged_memories(
        self, floor_override: Optional[float] = None
    ) -> List["Memory"]:
        """Get memories that have decayed below their layer's floor.

        These are candidates for review or deletion by a human operator.
        We do NOT auto-delete because a low-confidence memory may still
        contain valuable context that just needs re-verification.

        Args:
            floor_override: If provided, use this threshold instead of the
                            layer-specific floor. Useful for emergency review.

        Returns:
            List of Memory objects below their floor threshold.
        """
        flagged: List["Memory"] = []
        memories = self.store.list_memories()

        for mem in memories:
            threshold = (
                floor_override
                if floor_override is not None
                else self._get_rule(mem.layer)["floor"]
            )
            if mem.confidence < threshold:
                flagged.append(mem)

        return flagged

    def reinforce_memory(self, memory_id: str, amount: float = 0.1) -> bool:
        """Reinforce a memory by increasing its confidence.

        Called when a memory is successfully retrieved (access = reinforcement).
        This is the key feedback loop that prevents useful inferred facts from
        decaying away: every time a memory contributes to a good answer, it gets
        a small confidence boost.

        Cannot exceed 1.0. If the memory is already at 1.0, it stays there.

        Args:
            memory_id: The UUID of the memory to reinforce.
            amount: How much to increase confidence by (default 0.1).

        Returns:
            True if memory was found and updated, False otherwise.
        """
        mem = self.store.get_memory(memory_id)
        if mem is None:
            return False

        new_confidence = min(mem.confidence + amount, 1.0)
        updated = self.store.update_memory_confidence(memory_id, new_confidence)
        if updated:
            # Also update timestamp to mark this as recently reinforced.
            self.store.update_memory_timestamp(memory_id)
        return updated
