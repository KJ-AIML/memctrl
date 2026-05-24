"""MemCtrl — SQLite data layer.

Implements the core storage for memories, tree nodes, and trigger logs.
Tree node format adapted from PageIndex (VectifyAI):
  {node_id, title, start_index, end_index, summary, sub_nodes[]}
We replace page references with memory metadata (layer, source, confidence).
"""

from __future__ import annotations

import json
import sqlite3
import uuid
from contextlib import contextmanager
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import List, Optional

# ---------------------------------------------------------------------------
# Data models
# ---------------------------------------------------------------------------


@dataclass
class Memory:
    """A single memory fact stored in the system."""

    id: str
    layer: str  # 'project' | 'session' | 'user'
    content: str  # the memory fact
    source: str  # where it came from
    confidence: float  # 1.0=explicit, 0.7=inferred, 0.5=mentioned
    created_at: datetime
    expires_at: Optional[datetime]
    tags: List[str] = field(default_factory=list)

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "layer": self.layer,
            "content": self.content,
            "source": self.source,
            "confidence": self.confidence,
            "created_at": self.created_at.isoformat() if self.created_at else None,
            "expires_at": self.expires_at.isoformat() if self.expires_at else None,
            "tags": self.tags,
        }

    @classmethod
    def from_row(cls, row: sqlite3.Row) -> "Memory":
        return cls(
            id=row["id"],
            layer=row["layer"],
            content=row["content"],
            source=row["source"],
            confidence=row["confidence"],
            created_at=_parse_dt(row["created_at"]),
            expires_at=_parse_dt(row["expires_at"]) if row["expires_at"] else None,
            tags=json.loads(row["tags"]) if row["tags"] else [],
        )


@dataclass
class TreeNode:
    """Hierarchical tree node — PageIndex-adapted for memory.

    PageIndex node format (VectifyAI):
        {node_id, title, start_index, end_index, summary, sub_nodes[]}
    Adaptation: replace page refs with (layer, memory_ids, confidence).
    """

    id: str
    title: str  # e.g. "tech_stack"
    layer: str  # project / session / user
    summary: str  # LLM-generated summary of this branch
    memory_ids: List[str] = field(default_factory=list)
    children: List["TreeNode"] = field(default_factory=list)
    confidence: float = 1.0
    last_updated: datetime = field(default_factory=datetime.now)

    def is_leaf(self) -> bool:
        return len(self.children) == 0

    def all_memory_ids(self) -> List[str]:
        """Collect all memory IDs in this subtree."""
        result = list(self.memory_ids)
        for child in self.children:
            result.extend(child.all_memory_ids())
        return result

    def find_node(self, node_id: str) -> Optional["TreeNode"]:
        if self.id == node_id:
            return self
        for child in self.children:
            found = child.find_node(node_id)
            if found:
                return found
        return None

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "title": self.title,
            "layer": self.layer,
            "summary": self.summary,
            "memory_ids": self.memory_ids,
            "children": [c.to_dict() for c in self.children],
            "confidence": self.confidence,
            "last_updated": self.last_updated.isoformat(),
        }

    @classmethod
    def from_dict(cls, data: dict) -> "TreeNode":
        return cls(
            id=data["id"],
            title=data["title"],
            layer=data["layer"],
            summary=data.get("summary", ""),
            memory_ids=data.get("memory_ids", []),
            children=[cls.from_dict(c) for c in data.get("children", [])],
            confidence=data.get("confidence", 1.0),
            last_updated=_parse_dt(data.get("last_updated")),
        )


@dataclass
class TriggerLog:
    """Audit trail entry for trigger executions."""

    id: str
    event: str
    action: str
    memories_affected: List[str]
    timestamp: datetime

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "event": self.event,
            "action": self.action,
            "memories_affected": self.memories_affected,
            "timestamp": self.timestamp.isoformat() if self.timestamp else None,
        }

    @classmethod
    def from_row(cls, row: sqlite3.Row) -> "TriggerLog":
        return cls(
            id=row["id"],
            event=row["event"],
            action=row["action"],
            memories_affected=json.loads(row["memories_affected"])
            if row["memories_affected"]
            else [],
            timestamp=_parse_dt(row["timestamp"]),
        )


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _default_db_path() -> str:
    """Default SQLite DB path: ~/.memctrl/memories.db"""
    p = Path.home() / ".memctrl" / "memories.db"
    p.parent.mkdir(parents=True, exist_ok=True)
    return str(p)


def _parse_dt(value) -> datetime:
    """Parse datetime from ISO string or return now."""
    if value is None:
        return datetime.now()
    if isinstance(value, datetime):
        return value
    if isinstance(value, str):
        # Try various ISO formats
        for fmt in ("%Y-%m-%dT%H:%M:%S.%f", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"):
            try:
                return datetime.strptime(value.split("+")[0].split("Z")[0], fmt)
            except ValueError:
                continue
    return datetime.now()


def _now_iso() -> str:
    return datetime.now().isoformat()


# ---------------------------------------------------------------------------
# Store
# ---------------------------------------------------------------------------


class MemoryStore:
    """SQLite-backed store for memories, tree nodes, and trigger logs."""

    def __init__(self, db_path: Optional[str] = None):
        self.db_path = db_path or _default_db_path()
        self._init_db()

    # --- Connection management ---

    @contextmanager
    def _connect(self):
        conn = sqlite3.connect(self.db_path, timeout=30.0)
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA synchronous=NORMAL")
        conn.row_factory = sqlite3.Row
        try:
            yield conn
        finally:
            conn.close()

    # --- Schema ---

    def _init_db(self) -> None:
        with self._connect() as conn:
            conn.executescript(
                """
                CREATE TABLE IF NOT EXISTS memories (
                    id          TEXT PRIMARY KEY,
                    layer       TEXT NOT NULL,
                    content     TEXT NOT NULL,
                    source      TEXT,
                    confidence  REAL DEFAULT 1.0,
                    created_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    expires_at  TIMESTAMP,
                    tags        TEXT
                );

                CREATE TABLE IF NOT EXISTS tree_nodes (
                    id          TEXT PRIMARY KEY,
                    parent_id   TEXT REFERENCES tree_nodes(id),
                    layer       TEXT NOT NULL,
                    title       TEXT NOT NULL,
                    summary     TEXT,
                    memory_ids  TEXT,
                    updated_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                );

                CREATE TABLE IF NOT EXISTS triggers_log (
                    id          TEXT PRIMARY KEY,
                    event       TEXT NOT NULL,
                    action      TEXT NOT NULL,
                    memories_affected TEXT,
                    timestamp   TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                );

                -- Provenance: audit trail for every retrieval operation
                CREATE TABLE IF NOT EXISTS provenance (
                    id          TEXT PRIMARY KEY,
                    query       TEXT NOT NULL,
                    timestamp   TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    method      TEXT NOT NULL,
                    tree_version INTEGER DEFAULT 0,
                    total_memories_searched INTEGER DEFAULT 0,
                    avg_confidence REAL DEFAULT 0.0,
                    sources_json TEXT NOT NULL
                );

                -- OTel spans: persistent OpenTelemetry span storage
                CREATE TABLE IF NOT EXISTS otel_spans (
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
                );

                CREATE INDEX IF NOT EXISTS idx_memories_layer ON memories(layer);
                CREATE INDEX IF NOT EXISTS idx_memories_expires ON memories(expires_at);
                CREATE INDEX IF NOT EXISTS idx_tree_parent ON tree_nodes(parent_id);
                CREATE INDEX IF NOT EXISTS idx_tree_layer ON tree_nodes(layer);
                CREATE INDEX IF NOT EXISTS idx_triggers_ts ON triggers_log(timestamp);
                CREATE INDEX IF NOT EXISTS idx_provenance_ts ON provenance(timestamp);
                CREATE INDEX IF NOT EXISTS idx_otel_spans_trace ON otel_spans(trace_id);
                CREATE INDEX IF NOT EXISTS idx_otel_spans_op ON otel_spans(operation);
                """
            )
            conn.commit()

    # --- Memory CRUD ---

    def insert_memory(
        self,
        layer: str,
        content: str,
        source: str = "manual",
        confidence: float = 1.0,
        tags: Optional[List[str]] = None,
        expires_at: Optional[datetime] = None,
    ) -> str:
        mid = str(uuid.uuid4())
        with self._connect() as conn:
            conn.execute(
                """INSERT INTO memories (id, layer, content, source, confidence,
                                         created_at, expires_at, tags)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?)""",
                (
                    mid,
                    layer,
                    content,
                    source,
                    confidence,
                    _now_iso(),
                    expires_at.isoformat() if expires_at else None,
                    json.dumps(tags or []),
                ),
            )
            conn.commit()
        return mid

    def get_memory(self, id: str) -> Optional[Memory]:
        with self._connect() as conn:
            row = conn.execute("SELECT * FROM memories WHERE id = ?", (id,)).fetchone()
            return Memory.from_row(row) if row else None

    def list_memories(self, layer: Optional[str] = None) -> List[Memory]:
        with self._connect() as conn:
            if layer:
                rows = conn.execute(
                    "SELECT * FROM memories WHERE layer = ? ORDER BY created_at DESC",
                    (layer,),
                ).fetchall()
            else:
                rows = conn.execute(
                    "SELECT * FROM memories ORDER BY created_at DESC"
                ).fetchall()
            return [Memory.from_row(r) for r in rows]

    def delete_memory(self, id: str) -> bool:
        with self._connect() as conn:
            cur = conn.execute("DELETE FROM memories WHERE id = ?", (id,))
            conn.commit()
            return cur.rowcount > 0

    def update_memory_layer(self, id: str, new_layer: str) -> bool:
        with self._connect() as conn:
            cur = conn.execute(
                "UPDATE memories SET layer = ? WHERE id = ?",
                (new_layer, id),
            )
            conn.commit()
            return cur.rowcount > 0

    def update_memory_confidence(self, id: str, new_confidence: float) -> bool:
        """Update the confidence score of a memory. Returns True if found."""
        with self._connect() as conn:
            cur = conn.execute(
                "UPDATE memories SET confidence = ? WHERE id = ?",
                (new_confidence, id),
            )
            conn.commit()
            return cur.rowcount > 0

    def get_memories_below_confidence(
        self, threshold: float, layer: Optional[str] = None
    ) -> List[Memory]:
        """Get all memories with confidence < threshold, optionally filtered by layer."""
        with self._connect() as conn:
            if layer:
                rows = conn.execute(
                    "SELECT * FROM memories WHERE confidence < ? AND layer = ?",
                    (threshold, layer),
                ).fetchall()
            else:
                rows = conn.execute(
                    "SELECT * FROM memories WHERE confidence < ?",
                    (threshold,),
                ).fetchall()
            return [Memory.from_row(r) for r in rows]

    def update_memory_timestamp(self, id: str) -> bool:
        """Update created_at to now (used when a memory is reinforced)."""
        with self._connect() as conn:
            cur = conn.execute(
                "UPDATE memories SET created_at = ? WHERE id = ?",
                (_now_iso(), id),
            )
            conn.commit()
            return cur.rowcount > 0

    # --- Expiration ---

    def expire_old_memories(self) -> int:
        """Delete memories where expires_at < now(). Returns count."""
        with self._connect() as conn:
            cur = conn.execute(
                "DELETE FROM memories WHERE expires_at IS NOT NULL AND expires_at < ?",
                (_now_iso(),),
            )
            conn.commit()
            return cur.rowcount

    # --- Consolidation ---

    def consolidate(self, from_layer: str, to_layer: str) -> List[str]:
        """Move all memories from from_layer to to_layer. Returns moved IDs."""
        with self._connect() as conn:
            rows = conn.execute(
                "SELECT id FROM memories WHERE layer = ?", (from_layer,)
            ).fetchall()
            ids = [r["id"] for r in rows]
            if ids:
                placeholders = ",".join("?" * len(ids))
                conn.execute(
                    f"UPDATE memories SET layer = ? WHERE id IN ({placeholders})",
                    (to_layer, *ids),
                )
                conn.commit()
            return ids

    def consolidate_and_log(
        self,
        from_layer: str,
        to_layer: str,
        event: str,
        action: str,
    ) -> List[str]:
        """Atomically consolidate memories and log trigger.

        Unlike ``consolidate_with_audit()``, this does NOT create a
        reflection memory. It is intended for rule-governed transitions
        where the trigger log is the only audit artifact needed.

        Returns:
            List of consolidated memory IDs.
        """
        with self._connect() as conn:
            rows = conn.execute(
                "SELECT id FROM memories WHERE layer = ?", (from_layer,)
            ).fetchall()
            ids = [r["id"] for r in rows]

            if not ids:
                conn.commit()
                return []

            placeholders = ",".join("?" * len(ids))
            conn.execute(
                f"UPDATE memories SET layer = ? WHERE id IN ({placeholders})",
                (to_layer, *ids),
            )

            lid = str(uuid.uuid4())
            conn.execute(
                """INSERT INTO triggers_log (id, event, action, memories_affected, timestamp)
                   VALUES (?, ?, ?, ?, ?)""",
                (
                    lid,
                    event,
                    action,
                    json.dumps(ids),
                    _now_iso(),
                ),
            )

            conn.commit()
            return ids

    def consolidate_with_audit(
        self,
        from_layer: str,
        to_layer: str,
        reflection_content: str,
        reflection_source: str,
        event: str,
        action: str,
    ) -> tuple[List[str], Optional[str]]:
        """Atomically consolidate memories, create reflection, and log trigger.

        All operations happen in a single SQLite transaction. If any step
        fails, the entire transaction rolls back — no partial state.

        Returns:
            (consolidated_ids, reflection_memory_id)
        """
        with self._connect() as conn:
            rows = conn.execute(
                "SELECT id FROM memories WHERE layer = ?", (from_layer,)
            ).fetchall()
            ids = [r["id"] for r in rows]

            if not ids:
                conn.commit()
                return [], None

            # 1. Move memories
            placeholders = ",".join("?" * len(ids))
            conn.execute(
                f"UPDATE memories SET layer = ? WHERE id IN ({placeholders})",
                (to_layer, *ids),
            )

            # 2. Create reflection memory
            rid = str(uuid.uuid4())
            conn.execute(
                """INSERT INTO memories (id, layer, content, source, confidence,
                                         created_at, expires_at, tags)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?)""",
                (
                    rid,
                    to_layer,
                    reflection_content,
                    reflection_source,
                    0.9,
                    _now_iso(),
                    None,
                    json.dumps(["reflection", event, "auto-consolidated"]),
                ),
            )

            # 3. Log trigger
            lid = str(uuid.uuid4())
            conn.execute(
                """INSERT INTO triggers_log (id, event, action, memories_affected, timestamp)
                   VALUES (?, ?, ?, ?, ?)""",
                (
                    lid,
                    event,
                    action,
                    json.dumps(ids + [rid]),
                    _now_iso(),
                ),
            )

            conn.commit()
            return ids, rid

    # --- Tree nodes ---

    def clear_tree_nodes(self) -> None:
        with self._connect() as conn:
            conn.execute("DELETE FROM tree_nodes")
            conn.commit()

    def insert_tree_node(self, node: TreeNode, parent_id: Optional[str] = None) -> str:
        with self._connect() as conn:
            conn.execute(
                """INSERT INTO tree_nodes (id, parent_id, layer, title, summary,
                                            memory_ids, updated_at)
                   VALUES (?, ?, ?, ?, ?, ?, ?)""",
                (
                    node.id,
                    parent_id,
                    node.layer,
                    node.title,
                    node.summary,
                    json.dumps(node.memory_ids),
                    _now_iso(),
                ),
            )
            conn.commit()
            return node.id

    def get_tree_nodes(self, layer: Optional[str] = None) -> List[dict]:
        with self._connect() as conn:
            if layer:
                rows = conn.execute(
                    "SELECT * FROM tree_nodes WHERE layer = ?", (layer,)
                ).fetchall()
            else:
                rows = conn.execute("SELECT * FROM tree_nodes").fetchall()
            return [
                {
                    "id": r["id"],
                    "parent_id": r["parent_id"],
                    "layer": r["layer"],
                    "title": r["title"],
                    "summary": r["summary"],
                    "memory_ids": json.loads(r["memory_ids"])
                    if r["memory_ids"]
                    else [],
                }
                for r in rows
            ]

    def build_tree_from_nodes(self) -> Optional[TreeNode]:
        """Rebuild TreeNode hierarchy from flat DB rows."""
        nodes = self.get_tree_nodes()
        if not nodes:
            return None
        children = {}
        root_candidates = []
        for n in nodes:
            pid = n.get("parent_id")
            if pid:
                children.setdefault(pid, []).append(n)
            else:
                root_candidates.append(n)

        def build(n: dict) -> TreeNode:
            node = TreeNode(
                id=n["id"],
                title=n["title"],
                layer=n["layer"],
                summary=n.get("summary", ""),
                memory_ids=n.get("memory_ids", []),
                children=[build(c) for c in children.get(n["id"], [])],
            )
            return node

        if not root_candidates:
            return None
        # Use first root as main root, wrap others under it
        if len(root_candidates) == 1:
            return build(root_candidates[0])
        root = TreeNode(
            id="root",
            title="Memory Tree",
            layer="root",
            summary="Root of all memory layers",
            children=[build(r) for r in root_candidates],
        )
        return root

    # --- Trigger log ---

    def log_trigger(self, event: str, action: str, memory_ids: List[str]) -> str:
        tid = str(uuid.uuid4())
        with self._connect() as conn:
            conn.execute(
                """INSERT INTO triggers_log (id, event, action,
                                              memories_affected, timestamp)
                   VALUES (?, ?, ?, ?, ?)""",
                (tid, event, action, json.dumps(memory_ids), _now_iso()),
            )
            conn.commit()
        return tid

    def get_trigger_log(self, limit: int = 50) -> List[TriggerLog]:
        with self._connect() as conn:
            rows = conn.execute(
                "SELECT * FROM triggers_log ORDER BY timestamp DESC LIMIT ?",
                (limit,),
            ).fetchall()
            return [TriggerLog.from_row(r) for r in rows]

    # --- Provenance ---

    def save_provenance(self, provenance: dict) -> str:
        """Persist a provenance record to SQLite.

        Args:
            provenance: Dict with keys: query, timestamp, method, tree_version,
                total_memories_searched, avg_confidence, sources (list of dicts).

        Returns:
            The generated provenance record ID.
        """
        pid = str(uuid.uuid4())
        with self._connect() as conn:
            conn.execute(
                """INSERT INTO provenance (id, query, timestamp, method,
                                            tree_version, total_memories_searched,
                                            avg_confidence, sources_json)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?)""",
                (
                    pid,
                    provenance.get("query", ""),
                    provenance.get("timestamp", _now_iso()),
                    provenance.get("method", provenance.get("retrieval_method", "")),
                    provenance.get("tree_version", 0),
                    provenance.get("total_memories_searched", 0),
                    provenance.get("avg_confidence", 0.0),
                    json.dumps(provenance.get("sources", [])),
                ),
            )
            conn.commit()
        return pid

    def get_provenance(self, limit: int = 100, offset: int = 0) -> List[dict]:
        """Retrieve provenance records ordered by timestamp descending."""
        with self._connect() as conn:
            rows = conn.execute(
                "SELECT * FROM provenance ORDER BY timestamp DESC LIMIT ? OFFSET ?",
                (limit, offset),
            ).fetchall()
            return [
                {
                    "id": r["id"],
                    "query": r["query"],
                    "timestamp": r["timestamp"],
                    "method": r["method"],
                    "tree_version": r["tree_version"],
                    "total_memories_searched": r["total_memories_searched"],
                    "avg_confidence": r["avg_confidence"],
                    "sources": json.loads(r["sources_json"]) if r["sources_json"] else [],
                }
                for r in rows
            ]

    def clear_provenance(self) -> None:
        """Clear all provenance records."""
        with self._connect() as conn:
            conn.execute("DELETE FROM provenance")
            conn.commit()

    # --- OTel spans ---

    def save_otel_span(self, span: dict) -> str:
        """Persist an OTel span dict to SQLite.

        Args:
            span: Dict with all MemorySpan fields.

        Returns:
            The generated span record ID.
        """
        sid = str(uuid.uuid4())
        with self._connect() as conn:
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
                    span.get("trace_id", ""),
                    span.get("span_id", ""),
                    span.get("operation", ""),
                    span.get("timestamp", 0.0),
                    span.get("duration_ms", 0.0),
                    span.get("memory_id"),
                    span.get("layer"),
                    span.get("memory_type"),
                    span.get("confidence"),
                    span.get("query"),
                    span.get("top_k"),
                    span.get("results_count"),
                    span.get("status", "ok"),
                    span.get("error_message"),
                    json.dumps(span.get("attributes", {})),
                    span.get("service_name", "memctrl"),
                ),
            )
            conn.commit()
        return sid

    def get_otel_spans(
        self, limit: int = 1000, offset: int = 0, trace_id: Optional[str] = None
    ) -> List[dict]:
        """Retrieve OTel span records."""
        with self._connect() as conn:
            if trace_id:
                rows = conn.execute(
                    """SELECT * FROM otel_spans
                       WHERE trace_id = ?
                       ORDER BY timestamp DESC LIMIT ? OFFSET ?""",
                    (trace_id, limit, offset),
                ).fetchall()
            else:
                rows = conn.execute(
                    """SELECT * FROM otel_spans
                       ORDER BY timestamp DESC LIMIT ? OFFSET ?""",
                    (limit, offset),
                ).fetchall()
            return [
                {
                    "id": r["id"],
                    "trace_id": r["trace_id"],
                    "span_id": r["span_id"],
                    "operation": r["operation"],
                    "timestamp": r["timestamp"],
                    "duration_ms": r["duration_ms"],
                    "memory_id": r["memory_id"],
                    "layer": r["layer"],
                    "memory_type": r["memory_type"],
                    "confidence": r["confidence"],
                    "query": r["query"],
                    "top_k": r["top_k"],
                    "results_count": r["results_count"],
                    "status": r["status"],
                    "error_message": r["error_message"],
                    "attributes": json.loads(r["attributes_json"])
                    if r["attributes_json"]
                    else {},
                    "service_name": r["service_name"],
                }
                for r in rows
            ]

    def clear_otel_spans(self) -> None:
        """Clear all OTel span records."""
        with self._connect() as conn:
            conn.execute("DELETE FROM otel_spans")
            conn.commit()

    def prune_otel_spans(self, max_rows: int = 10000) -> int:
        """Prune oldest OTel spans to keep table bounded.

        Args:
            max_rows: Maximum rows to keep. Oldest are deleted first.

        Returns:
            Number of rows deleted.
        """
        with self._connect() as conn:
            cur = conn.execute(
                """DELETE FROM otel_spans
                   WHERE id NOT IN (
                       SELECT id FROM otel_spans
                       ORDER BY timestamp DESC LIMIT ?
                   )""",
                (max_rows,),
            )
            conn.commit()
            return cur.rowcount

    # --- Stats ---

    def stats(self) -> dict:
        with self._connect() as conn:
            mem_count = conn.execute("SELECT COUNT(*) FROM memories").fetchone()[0]
            node_count = conn.execute("SELECT COUNT(*) FROM tree_nodes").fetchone()[0]
            trigger_count = conn.execute(
                "SELECT COUNT(*) FROM triggers_log"
            ).fetchone()[0]
            provenance_count = conn.execute(
                "SELECT COUNT(*) FROM provenance"
            ).fetchone()[0]
            span_count = conn.execute(
                "SELECT COUNT(*) FROM otel_spans"
            ).fetchone()[0]
            return {
                "memories": mem_count,
                "tree_nodes": node_count,
                "triggers": trigger_count,
                "provenance_records": provenance_count,
                "otel_spans": span_count,
            }
