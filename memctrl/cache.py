"""MemCtrl — Query result cache with tree-version invalidation.

Caches retrieval results to avoid re-processing the same query.
Invalidates automatically when the memory tree changes (new memories
added/modified/deleted).

Design:
- Simple in-memory dict cache (no external dependencies)
- TTL support (default 5 minutes for query results)
- Tree version tracking: cache key includes tree_version
- When a memory is added/modified/deleted, tree_version increments
"""

from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Dict, Optional

from memctrl.retriever import RetrievalResult


@dataclass
class CachedResult:
    """A cached retrieval result with metadata.

    WHY: We need to track both WHEN the result was cached (TTL check)
    and WHAT version of the tree it was computed against (version check).
    Without tree_version, a cache hit could return stale data after
    memories were added/modified/deleted.
    """

    result: RetrievalResult
    tree_version: int
    cached_at: float
    query: str

    def is_stale(self, current_tree_version: int, ttl_seconds: float) -> bool:
        """Check if this cached result is invalid due to tree change or TTL expiry.

        WHY: Two independent invalidation mechanisms:
        1. Tree version mismatch — the underlying data changed, so the
           cached result may no longer be correct.
        2. TTL expiry — even if the tree hasn't changed, we recompute
           after TTL to pick up any non-tree changes (e.g., retriever
           logic updates, confidence threshold changes).
        """
        if self.tree_version != current_tree_version:
            return True
        age = time.monotonic() - self.cached_at
        return age > ttl_seconds


class QueryCache:
    """In-memory query cache with tree-version invalidation.

    Each time a memory is added, modified, or deleted, the tree_version
    increments. Any cache entries with a mismatched tree_version are
    considered stale and will be re-computed.

    WHY: Repeat queries are common in AI coding assistants (e.g., the
    user asks follow-up questions or the system re-queries for context).
    Without caching, every query rebuilds the tree and re-runs retrieval,
    which is wasteful especially for large memory stores.

    Usage:
        cache = QueryCache()

        # On memory change:
        cache.invalidate()

        # On query:
        cached = cache.get(query_text)
        if cached:
            return cached
        result = await retriever.retrieve(...)
        cache.set(query_text, result)
    """

    def __init__(self, default_ttl_seconds: float = 300.0):
        """Initialize cache with TTL. Default: 5 minutes.

        WHY: 5 minutes is a sweet spot — long enough to benefit from
        caching during an active coding session, short enough that
        stale results don't linger if the retriever logic changes.
        """
        self._default_ttl_seconds = default_ttl_seconds
        self._tree_version: int = 0
        self._cache: Dict[str, CachedResult] = {}
        self._hits: int = 0
        self._misses: int = 0

    @property
    def tree_version(self) -> int:
        """Current tree version — increments on every invalidation.

        WHY: Exposed as a property so callers can inspect the current
        version without being able to set it directly (only invalidate
        should bump the version).
        """
        return self._tree_version

    def get(self, query: str) -> Optional[RetrievalResult]:
        """Get cached result if valid (tree version matches and TTL not expired).

        WHY: This is the fast path — a cached result returns in <1ms
        vs. hundreds of ms for tree build + retrieval. We check both
        tree version AND TTL because either one means the result might
        not be correct anymore.

        Returns None if no valid cached result exists.
        """
        entry = self._cache.get(query)
        if entry is None:
            self._misses += 1
            return None

        if entry.is_stale(self._tree_version, self._default_ttl_seconds):
            # Stale entry — remove it to keep cache clean
            del self._cache[query]
            self._misses += 1
            return None

        self._hits += 1
        return entry.result

    def set(self, query: str, result: RetrievalResult) -> None:
        """Cache a retrieval result with current tree version.

        WHY: We store the result alongside the current tree_version so
        that future get() calls can detect when the tree has changed
        and the result is no longer valid.
        """
        self._cache[query] = CachedResult(
            result=result,
            tree_version=self._tree_version,
            cached_at=time.monotonic(),
            query=query,
        )

    def invalidate(self) -> int:
        """Increment tree version — call this when ANY memory changes.

        WHY: Rather than tracking individual memory IDs or diffs, we
        use a global version counter. This is simple and correct: any
        tree mutation bumps the version, which invalidates ALL cached
        results. The next query will recompute and re-cache.

        Returns the new tree version.
        """
        self._tree_version += 1
        return self._tree_version

    def clear(self) -> None:
        """Clear all cached entries.

        WHY: Useful for testing and for explicit cache resets (e.g.,
        after a major schema change or when debugging).
        """
        self._cache.clear()

    def stats(self) -> dict:
        """Return cache statistics: hits, misses, size, current_version.

        WHY: Exposing stats makes the cache observable — callers can
        monitor hit rates and cache size to tune TTL or debug issues.
        """
        total = self._hits + self._misses
        return {
            "hits": self._hits,
            "misses": self._misses,
            "size": len(self._cache),
            "current_version": self._tree_version,
            "hit_rate": self._hits / total if total > 0 else 0.0,
        }
