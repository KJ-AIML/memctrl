"""MemCtrl — Query result cache with persistent storage and tree-version invalidation.

Caches retrieval results to avoid re-processing the same query.
Invalidates automatically when the memory tree changes (new memories
added/modified/deleted).

Design:
- In-memory dict cache for fast access (no external dependencies)
- Persistent SQLite cache for cross-process durability (CLI users)
- TTL support (default 5 minutes for query results)
- Tree version tracking: cache key includes tree_version
- When a memory is added/modified/deleted, tree_version increments
"""

from __future__ import annotations

import json
import sqlite3
import time
from dataclasses import dataclass
from pathlib import Path
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
        """Check if this cached result is invalid due to tree change or TTL expiry."""
        if self.tree_version != current_tree_version:
            return True
        age = time.monotonic() - self.cached_at
        return age > ttl_seconds


class QueryCache:
    """Query cache with in-memory fast path and persistent SQLite backend.

    Each time a memory is added, modified, or deleted, the tree_version
    increments. Any cache entries with a mismatched tree_version are
    considered stale and will be re-computed.

    WHY: Repeat queries are common in AI coding assistants. Without caching,
    every query rebuilds the tree and re-runs retrieval, which is wasteful.
    For CLI users, the cache persists across process invocations via SQLite.

    Usage:
        cache = QueryCache(db_path="~/.memctrl/cache.db")

        # On memory change:
        cache.invalidate()

        # On query:
        cached = cache.get(query_text)
        if cached:
            return cached
        result = await retriever.retrieve(...)
        cache.set(query_text, result)
    """

    def __init__(
        self,
        default_ttl_seconds: float = 300.0,
        db_path: Optional[str] = None,
    ):
        """Initialize cache with TTL and optional persistent storage.

        Args:
            default_ttl_seconds: TTL for cache entries. Default: 5 minutes.
            db_path: Path to SQLite cache DB. If None, uses in-memory only.
        """
        self._default_ttl_seconds = default_ttl_seconds
        self._tree_version: int = 0
        self._cache: Dict[str, CachedResult] = {}
        self._hits: int = 0
        self._misses: int = 0
        self._db_path: Optional[str] = db_path

        if db_path:
            self._init_persistent_cache()
            self._tree_version = self._load_tree_version()

    def _init_persistent_cache(self) -> None:
        """Initialize SQLite cache table."""
        if not self._db_path:
            return
        Path(self._db_path).parent.mkdir(parents=True, exist_ok=True)
        conn = sqlite3.connect(self._db_path, timeout=10.0)
        try:
            conn.execute(
                """
                CREATE TABLE IF NOT EXISTS query_cache (
                    query TEXT PRIMARY KEY,
                    result_json TEXT NOT NULL,
                    tree_version INTEGER NOT NULL,
                    cached_at REAL NOT NULL,
                    ttl_seconds REAL NOT NULL
                )
                """
            )
            conn.execute(
                """
                CREATE TABLE IF NOT EXISTS cache_meta (
                    key TEXT PRIMARY KEY,
                    value INTEGER
                )
                """
            )
            conn.commit()
        finally:
            conn.close()

    def _load_tree_version(self) -> int:
        """Load tree version from persistent store."""
        if not self._db_path:
            return 0
        try:
            conn = sqlite3.connect(self._db_path, timeout=10.0)
            row = conn.execute(
                "SELECT value FROM cache_meta WHERE key = 'tree_version'"
            ).fetchone()
            conn.close()
            return row[0] if row else 0
        except Exception:
            return 0

    def _save_tree_version(self) -> None:
        """Save tree version to persistent store."""
        if not self._db_path:
            return
        try:
            conn = sqlite3.connect(self._db_path, timeout=10.0)
            conn.execute(
                "INSERT OR REPLACE INTO cache_meta (key, value) VALUES (?, ?)",
                ("tree_version", self._tree_version),
            )
            conn.commit()
            conn.close()
        except Exception:
            pass

    def _result_to_json(self, result: RetrievalResult) -> str:
        """Serialize RetrievalResult to JSON."""
        data = {
            "facts": result.facts,
            "trace": result.trace,
            "confidence": result.confidence,
            "sources": result.sources,
        }
        if result.provenance is not None:
            data["provenance"] = result.provenance.to_dict()
        return json.dumps(data)

    def _result_from_json(self, json_str: str) -> RetrievalResult:
        """Deserialize RetrievalResult from JSON."""
        data = json.loads(json_str)
        return RetrievalResult(
            facts=data.get("facts", []),
            trace=data.get("trace", []),
            confidence=data.get("confidence", 0.0),
            sources=data.get("sources", []),
        )

    def _persist_entry(self, query: str, result: RetrievalResult) -> None:
        """Save cache entry to SQLite."""
        if not self._db_path:
            return
        try:
            conn = sqlite3.connect(self._db_path, timeout=10.0)
            conn.execute(
                """
                INSERT OR REPLACE INTO query_cache
                (query, result_json, tree_version, cached_at, ttl_seconds)
                VALUES (?, ?, ?, ?, ?)
                """,
                (
                    query,
                    self._result_to_json(result),
                    self._tree_version,
                    time.monotonic(),
                    self._default_ttl_seconds,
                ),
            )
            conn.commit()
            conn.close()
        except Exception:
            pass

    def _load_persistent_entry(self, query: str) -> Optional[CachedResult]:
        """Load cache entry from SQLite if valid."""
        if not self._db_path:
            return None
        try:
            conn = sqlite3.connect(self._db_path, timeout=10.0)
            row = conn.execute(
                "SELECT result_json, tree_version, cached_at, ttl_seconds "
                "FROM query_cache WHERE query = ?",
                (query,),
            ).fetchone()
            conn.close()

            if not row:
                return None

            result_json, tree_version, cached_at, ttl_seconds = row
            entry = CachedResult(
                result=self._result_from_json(result_json),
                tree_version=tree_version,
                cached_at=cached_at,
                query=query,
            )

            # Check staleness
            if entry.is_stale(self._tree_version, ttl_seconds):
                # Remove stale entry
                self._remove_persistent_entry(query)
                return None

            return entry
        except Exception:
            return None

    def _remove_persistent_entry(self, query: str) -> None:
        """Remove cache entry from SQLite."""
        if not self._db_path:
            return
        try:
            conn = sqlite3.connect(self._db_path, timeout=10.0)
            conn.execute("DELETE FROM query_cache WHERE query = ?", (query,))
            conn.commit()
            conn.close()
        except Exception:
            pass

    @property
    def tree_version(self) -> int:
        """Current tree version — increments on every invalidation."""
        return self._tree_version

    def get(self, query: str) -> Optional[RetrievalResult]:
        """Get cached result if valid (tree version matches and TTL not expired).

        WHY: This is the fast path — a cached result returns in <1ms
        vs. hundreds of ms for tree build + retrieval. We check both
        tree version AND TTL because either one means the result might
        not be correct anymore.

        Returns None if no valid cached result exists.
        """
        # Check in-memory first
        entry = self._cache.get(query)
        if entry is not None:
            if entry.is_stale(self._tree_version, self._default_ttl_seconds):
                del self._cache[query]
                self._misses += 1
                return None
            self._hits += 1
            return entry.result

        # Check persistent cache (for CLI cross-process durability)
        entry = self._load_persistent_entry(query)
        if entry is not None:
            # Promote to in-memory for fast future access
            self._cache[query] = entry
            self._hits += 1
            return entry.result

        self._misses += 1
        return None

    def set(self, query: str, result: RetrievalResult) -> None:
        """Cache a retrieval result with current tree version.

        WHY: We store the result alongside the current tree_version so
        that future get() calls can detect when the tree has changed
        and the result is no longer valid.
        """
        cached = CachedResult(
            result=result,
            tree_version=self._tree_version,
            cached_at=time.monotonic(),
            query=query,
        )
        self._cache[query] = cached
        self._persist_entry(query, result)

    def invalidate(self) -> int:
        """Increment tree version — call this when ANY memory changes.

        WHY: Rather than tracking individual memory IDs or diffs, we
        use a global version counter. This is simple and correct: any
        tree mutation bumps the version, which invalidates ALL cached
        results. The next query will recompute and re-cache.

        Returns the new tree version.
        """
        self._tree_version += 1
        self._save_tree_version()
        return self._tree_version

    def clear(self) -> None:
        """Clear all cached entries (memory and persistent)."""
        self._cache.clear()
        if self._db_path:
            try:
                conn = sqlite3.connect(self._db_path, timeout=10.0)
                conn.execute("DELETE FROM query_cache")
                conn.commit()
                conn.close()
            except Exception:
                pass

    def stats(self) -> dict:
        """Return cache statistics: hits, misses, size, current_version, hit_rate."""
        total = self._hits + self._misses
        return {
            "hits": self._hits,
            "misses": self._misses,
            "size": len(self._cache),
            "current_version": self._tree_version,
            "hit_rate": self._hits / total if total > 0 else 0.0,
            "persistent": self._db_path is not None,
        }
