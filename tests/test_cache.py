"""Tests for QueryCache — query result cache with tree-version invalidation.

Covers: cache miss/hit, tree-version invalidation, TTL expiry, stats tracking,
multiple independent queries, clear.
"""

import time


from memctrl.cache import CachedResult, QueryCache
from memctrl.retriever import RetrievalResult


# ---------------------------------------------------------------------------
# CachedResult.is_stale
# ---------------------------------------------------------------------------


def test_cached_result_is_stale_different_version():
    """A result is stale if the tree version has changed."""
    result = RetrievalResult(facts=["f1"], trace=["root"], confidence=0.9)
    cached = CachedResult(
        result=result, tree_version=1, cached_at=time.monotonic(), query="q"
    )
    assert cached.is_stale(current_tree_version=2, ttl_seconds=300.0) is True


def test_cached_result_is_stale_same_version():
    """A result is NOT stale if the tree version matches and TTL hasn't expired."""
    result = RetrievalResult(facts=["f1"], trace=["root"], confidence=0.9)
    cached = CachedResult(
        result=result, tree_version=1, cached_at=time.monotonic(), query="q"
    )
    assert cached.is_stale(current_tree_version=1, ttl_seconds=300.0) is False


def test_cached_result_is_stale_ttl_expired():
    """A result is stale if TTL has expired, even if version matches."""
    result = RetrievalResult(facts=["f1"], trace=["root"], confidence=0.9)
    cached = CachedResult(
        result=result, tree_version=1, cached_at=time.monotonic() - 10.0, query="q"
    )
    assert cached.is_stale(current_tree_version=1, ttl_seconds=5.0) is True


def test_cached_result_is_not_stale_within_ttl():
    """A result is NOT stale if within TTL and version matches."""
    result = RetrievalResult(facts=["f1"], trace=["root"], confidence=0.9)
    cached = CachedResult(
        result=result, tree_version=1, cached_at=time.monotonic(), query="q"
    )
    assert cached.is_stale(current_tree_version=1, ttl_seconds=10.0) is False


# ---------------------------------------------------------------------------
# QueryCache — basic get/set
# ---------------------------------------------------------------------------


def test_cache_miss_returns_none():
    """Getting an uncached query returns None."""
    cache = QueryCache()
    assert cache.get("nonexistent query") is None


def test_cache_hit_returns_result():
    """After setting a result, getting the same query returns it."""
    cache = QueryCache()
    result = RetrievalResult(facts=["we use FastAPI"], trace=["root"], confidence=0.9)
    cache.set("what framework?", result)

    cached = cache.get("what framework?")
    assert cached is not None
    assert cached.facts == ["we use FastAPI"]
    assert cached.confidence == 0.9


def test_cache_different_queries_independent():
    """Multiple queries are cached independently — getting one doesn't affect others."""
    cache = QueryCache()
    result_a = RetrievalResult(facts=["fact A"], trace=["root"], confidence=0.8)
    result_b = RetrievalResult(facts=["fact B"], trace=["root"], confidence=0.7)

    cache.set("query A", result_a)
    cache.set("query B", result_b)

    assert cache.get("query A").facts == ["fact A"]
    assert cache.get("query B").facts == ["fact B"]
    assert cache.get("query C") is None


# ---------------------------------------------------------------------------
# QueryCache — tree version invalidation
# ---------------------------------------------------------------------------


def test_cache_invalidation_after_tree_change():
    """After invalidate(), previously cached results are considered stale."""
    cache = QueryCache()
    result = RetrievalResult(facts=["fact"], trace=["root"], confidence=0.9)
    cache.set("query", result)

    # Before invalidation — cache hit
    assert cache.get("query") is not None

    # Invalidate (simulates memory add/modify/delete)
    new_version = cache.invalidate()
    assert new_version == 1
    assert cache.tree_version == 1

    # After invalidation — cache miss (stale entry removed)
    assert cache.get("query") is None


def test_invalidation_multiple_times():
    """Each invalidation bumps the version."""
    cache = QueryCache()
    assert cache.tree_version == 0

    cache.invalidate()
    assert cache.tree_version == 1

    cache.invalidate()
    assert cache.tree_version == 2

    cache.invalidate()
    assert cache.tree_version == 3


def test_cache_still_works_after_invalidation():
    """After invalidation, new queries can be cached and retrieved."""
    cache = QueryCache()
    old_result = RetrievalResult(facts=["old"], trace=["root"], confidence=0.5)
    cache.set("query", old_result)
    cache.invalidate()

    # Old result is gone
    assert cache.get("query") is None

    # New result can be cached
    new_result = RetrievalResult(facts=["new"], trace=["root"], confidence=0.9)
    cache.set("query", new_result)
    assert cache.get("query").facts == ["new"]


# ---------------------------------------------------------------------------
# QueryCache — TTL expiry
# ---------------------------------------------------------------------------


def test_cache_ttl_expiry():
    """Results expire after TTL."""
    cache = QueryCache(default_ttl_seconds=0.1)  # 100ms TTL
    result = RetrievalResult(facts=["fact"], trace=["root"], confidence=0.9)
    cache.set("query", result)

    # Immediately — cache hit
    assert cache.get("query") is not None

    # Wait for TTL to expire
    time.sleep(0.15)

    # After TTL — cache miss (stale entry removed)
    assert cache.get("query") is None


def test_cache_ttl_does_not_expire_early():
    """Results within TTL are still valid."""
    cache = QueryCache(default_ttl_seconds=10.0)
    result = RetrievalResult(facts=["fact"], trace=["root"], confidence=0.9)
    cache.set("query", result)

    # Should still be valid
    assert cache.get("query") is not None


# ---------------------------------------------------------------------------
# QueryCache — stats
# ---------------------------------------------------------------------------


def test_cache_stats_empty():
    """Stats on a fresh cache show all zeros."""
    cache = QueryCache()
    stats = cache.stats()
    assert stats["hits"] == 0
    assert stats["misses"] == 0
    assert stats["size"] == 0
    assert stats["current_version"] == 0
    assert stats["hit_rate"] == 0.0


def test_cache_stats_after_hit():
    """Stats track cache hits correctly."""
    cache = QueryCache()
    result = RetrievalResult(facts=["fact"], trace=["root"], confidence=0.9)
    cache.set("query", result)
    cache.get("query")  # hit

    stats = cache.stats()
    assert stats["hits"] == 1
    assert stats["misses"] == 0
    assert stats["size"] == 1
    assert stats["hit_rate"] == 1.0


def test_cache_stats_after_miss():
    """Stats track cache misses correctly."""
    cache = QueryCache()
    cache.get("query")  # miss

    stats = cache.stats()
    assert stats["hits"] == 0
    assert stats["misses"] == 1
    assert stats["size"] == 0
    assert stats["hit_rate"] == 0.0


def test_cache_stats_mixed():
    """Stats track a mix of hits and misses."""
    cache = QueryCache()
    result = RetrievalResult(facts=["fact"], trace=["root"], confidence=0.9)
    cache.set("cached_query", result)

    cache.get("cached_query")  # hit
    cache.get("cached_query")  # hit
    cache.get("missing_query")  # miss

    stats = cache.stats()
    assert stats["hits"] == 2
    assert stats["misses"] == 1
    assert stats["size"] == 1
    assert stats["hit_rate"] == 2 / 3


def test_cache_stats_after_invalidation():
    """Stats are preserved after invalidation (only cache entries are affected)."""
    cache = QueryCache()
    result = RetrievalResult(facts=["fact"], trace=["root"], confidence=0.9)
    cache.set("query", result)
    cache.get("query")  # hit
    cache.invalidate()

    # Entry is still in the dict but stale; accessing it removes it.
    assert cache.get("query") is None  # stale entry removed on access

    stats = cache.stats()
    assert stats["hits"] == 1
    assert stats["misses"] == 1  # the stale access counts as a miss
    assert stats["size"] == 0
    assert stats["current_version"] == 1


# ---------------------------------------------------------------------------
# QueryCache — clear
# ---------------------------------------------------------------------------


def test_cache_clear_removes_all_entries():
    """clear() removes all cached entries."""
    cache = QueryCache()
    cache.set("q1", RetrievalResult(facts=["f1"], trace=["root"], confidence=0.9))
    cache.set("q2", RetrievalResult(facts=["f2"], trace=["root"], confidence=0.8))

    assert cache.get("q1") is not None
    assert cache.get("q2") is not None

    cache.clear()

    assert cache.get("q1") is None
    assert cache.get("q2") is None
    assert cache.stats()["size"] == 0


def test_cache_clear_preserves_stats():
    """clear() removes entries but preserves hit/miss stats."""
    cache = QueryCache()
    cache.set("q1", RetrievalResult(facts=["f1"], trace=["root"], confidence=0.9))
    cache.get("q1")  # hit
    cache.clear()

    stats = cache.stats()
    assert stats["hits"] == 1
    assert stats["size"] == 0


def test_cache_clear_preserves_tree_version():
    """clear() removes entries but doesn't change tree_version."""
    cache = QueryCache()
    cache.invalidate()
    cache.invalidate()
    assert cache.tree_version == 2

    cache.clear()
    assert cache.tree_version == 2


# ---------------------------------------------------------------------------
# QueryCache — stale entry cleanup on get
# ---------------------------------------------------------------------------


def test_cache_stale_entry_removed_on_get():
    """When a stale entry is detected during get(), it's removed from the cache."""
    cache = QueryCache()
    result = RetrievalResult(facts=["fact"], trace=["root"], confidence=0.9)
    cache.set("query", result)
    cache.invalidate()  # bump version

    # Entry should be gone after version bump
    assert cache.get("query") is None
    assert cache.stats()["size"] == 0


# ---------------------------------------------------------------------------
# QueryCache — default TTL
# ---------------------------------------------------------------------------


def test_cache_default_ttl():
    """Default TTL is 300 seconds (5 minutes)."""
    cache = QueryCache()
    assert cache._default_ttl_seconds == 300.0


def test_cache_custom_ttl():
    """Custom TTL can be set at initialization."""
    cache = QueryCache(default_ttl_seconds=60.0)
    assert cache._default_ttl_seconds == 60.0


# ---------------------------------------------------------------------------
# QueryCache — RetrievalResult integrity
# ---------------------------------------------------------------------------


def test_cache_preserves_full_result():
    """The cache preserves all fields of RetrievalResult."""
    cache = QueryCache()
    result = RetrievalResult(
        facts=["fact 1", "fact 2"],
        trace=["root", "node A", "leaf"],
        confidence=0.85,
        sources=["manual", "extract"],
    )
    cache.set("query", result)

    cached = cache.get("query")
    assert cached.facts == ["fact 1", "fact 2"]
    assert cached.trace == ["root", "node A", "leaf"]
    assert cached.confidence == 0.85
    assert cached.sources == ["manual", "extract"]
