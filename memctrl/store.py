"""MemCtrl — SQLite data layer with retry logic, atomic tree rebuild, and secret redaction.

Implements the core storage for memories, tree nodes, and trigger logs.
Tree node format adapted from PageIndex (VectifyAI):
  {node_id, title, start_index, end_index, summary, sub_nodes[]}
We replace page references with memory metadata (layer, source, confidence).
"""

from __future__ import annotations

import json
import sqlite3
import time
import uuid
from contextlib import contextmanager
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import List, Optional

from memctrl.sanitize import sanitize_text

# ---------------------------------------------------------------------------
# Constants & Enums for Knowledge Lifecycle & Lineage
# ---------------------------------------------------------------------------

CLAIM_TYPES = {"observation", "assertion", "hypothesis", "decision", "derived_lesson"}
LIFECYCLE_STATES = {"candidate", "accepted", "superseded", "rejected", "archived"}
VERIFICATION_STATES = {"unverified", "supported", "disputed", "refuted"}
RELATION_TYPES = {"derived_from", "supports", "contradicts", "supersedes", "refutes"}


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
    updated_at: Optional[datetime] = None
    last_accessed_at: Optional[datetime] = None
    access_count: int = 0
    # Knowledge lifecycle & semantics
    claim_type: str = "assertion"
    lifecycle_state: str = "accepted"
    verification_state: str = "unverified"
    observed_at: Optional[datetime] = None
    valid_from: Optional[datetime] = None
    valid_until: Optional[datetime] = None

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
            "updated_at": self.updated_at.isoformat() if self.updated_at else None,
            "last_accessed_at": (
                self.last_accessed_at.isoformat() if self.last_accessed_at else None
            ),
            "access_count": self.access_count,
            "claim_type": self.claim_type,
            "lifecycle_state": self.lifecycle_state,
            "verification_state": self.verification_state,
            "observed_at": self.observed_at.isoformat() if self.observed_at else None,
            "valid_from": self.valid_from.isoformat() if self.valid_from else None,
            "valid_until": self.valid_until.isoformat() if self.valid_until else None,
        }

    @classmethod
    def from_row(cls, row: sqlite3.Row) -> "Memory":
        keys = row.keys() if hasattr(row, "keys") else []
        return cls(
            id=row["id"],
            layer=row["layer"],
            content=row["content"],
            source=row["source"],
            confidence=row["confidence"],
            created_at=_parse_dt(row["created_at"]),
            expires_at=_parse_dt(row["expires_at"]) if row["expires_at"] else None,
            tags=json.loads(row["tags"]) if row["tags"] else [],
            updated_at=(
                _parse_dt(row["updated_at"])
                if "updated_at" in keys and row["updated_at"]
                else None
            ),
            last_accessed_at=(
                _parse_dt(row["last_accessed_at"])
                if "last_accessed_at" in keys and row["last_accessed_at"]
                else None
            ),
            access_count=(
                row["access_count"]
                if "access_count" in keys and row["access_count"] is not None
                else 0
            ),
            claim_type=(
                row["claim_type"]
                if "claim_type" in keys and row["claim_type"]
                else "assertion"
            ),
            lifecycle_state=(
                row["lifecycle_state"]
                if "lifecycle_state" in keys and row["lifecycle_state"]
                else "accepted"
            ),
            verification_state=(
                row["verification_state"]
                if "verification_state" in keys and row["verification_state"]
                else "unverified"
            ),
            observed_at=(
                _parse_dt(row["observed_at"])
                if "observed_at" in keys and row["observed_at"]
                else None
            ),
            valid_from=(
                _parse_dt(row["valid_from"])
                if "valid_from" in keys and row["valid_from"]
                else None
            ),
            valid_until=(
                _parse_dt(row["valid_until"])
                if "valid_until" in keys and row["valid_until"]
                else None
            ),
        )


@dataclass
class MemoryRelation:
    """Directed relationship between memories representing derivation, support, or refutation."""

    id: str
    from_memory_id: str
    to_memory_id: str
    relation_type: str  # derived_from, supports, contradicts, supersedes, refutes
    created_at: datetime
    metadata: dict = field(default_factory=dict)

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "from_memory_id": self.from_memory_id,
            "to_memory_id": self.to_memory_id,
            "relation_type": self.relation_type,
            "created_at": self.created_at.isoformat() if self.created_at else None,
            "metadata": self.metadata,
        }

    @classmethod
    def from_row(cls, row: sqlite3.Row) -> "MemoryRelation":
        return cls(
            id=row["id"],
            from_memory_id=row["from_memory_id"],
            to_memory_id=row["to_memory_id"],
            relation_type=row["relation_type"],
            created_at=_parse_dt(row["created_at"]),
            metadata=json.loads(row["metadata_json"]) if row["metadata_json"] else {},
        )


@dataclass
class MemoryEvidence:
    """External evidence pointer linking a memory to an authoritative source record."""

    id: str
    memory_id: str
    source_system: str  # e.g. "heli"
    source_id: str  # e.g. "task-3017"
    source_revision: Optional[str]  # e.g. "git-sha"
    relation: str  # derived_from, supports, contradicts, etc.
    created_at: datetime
    metadata: dict = field(default_factory=dict)

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "memory_id": self.memory_id,
            "source_system": self.source_system,
            "source_id": self.source_id,
            "source_revision": self.source_revision,
            "relation": self.relation,
            "created_at": self.created_at.isoformat() if self.created_at else None,
            "metadata": self.metadata,
        }

    @classmethod
    def from_row(cls, row: sqlite3.Row) -> "MemoryEvidence":
        return cls(
            id=row["id"],
            memory_id=row["memory_id"],
            source_system=row["source_system"],
            source_id=row["source_id"],
            source_revision=row["source_revision"],
            relation=row["relation"],
            created_at=_parse_dt(row["created_at"]),
            metadata=json.loads(row["metadata_json"]) if row["metadata_json"] else {},
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
# Retry decorator for SQLite operations
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Store
# ---------------------------------------------------------------------------


class MemoryStore:
    """SQLite-backed store for memories, tree nodes, and trigger logs."""

    def __init__(self, db_path: Optional[str] = None):
        self.db_path = db_path or _default_db_path()
        self._init_db()
        self._last_decay_at: Optional[datetime] = None

    def close(self) -> None:
        """No-op for API compatibility.

        Connections are opened and closed per operation. In the future,
        this may close a pooled connection.
        """
        pass

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

    def _retry_write(self, write_fn):
        """Execute a write function with exponential backoff on database lock.

        WHY: WAL mode + busy_timeout=30s helps, but rapid concurrent writes
        from CLI + MCP server can still collide. Retrying with backoff
        covers the common case where one writer finishes within milliseconds.
        """
        last_exc = None
        for delay in (0.05, 0.2, 0.5):
            try:
                with self._connect() as conn:
                    return write_fn(conn)
            except sqlite3.OperationalError as exc:
                if "database is locked" in str(exc).lower():
                    last_exc = exc
                    time.sleep(delay)
                    continue
                raise
        raise last_exc

    # --- Schema ---

    def _init_db(self) -> None:
        def _write(conn):
            conn.executescript(
                """
                CREATE TABLE IF NOT EXISTS schema_version (
                    version INTEGER PRIMARY KEY
                );

                CREATE TABLE IF NOT EXISTS memories (
                    id          TEXT PRIMARY KEY,
                    layer       TEXT NOT NULL,
                    content     TEXT NOT NULL,
                    source      TEXT,
                    confidence  REAL DEFAULT 1.0,
                    created_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    expires_at  TIMESTAMP,
                    tags        TEXT,
                    updated_at  TIMESTAMP,
                    last_accessed_at TIMESTAMP,
                    access_count INTEGER DEFAULT 0,
                    claim_type  TEXT DEFAULT 'assertion',
                    lifecycle_state TEXT DEFAULT 'accepted',
                    verification_state TEXT DEFAULT 'unverified',
                    observed_at TIMESTAMP,
                    valid_from  TIMESTAMP,
                    valid_until TIMESTAMP
                );

                CREATE TABLE IF NOT EXISTS maintenance_state (
                    key         TEXT PRIMARY KEY,
                    last_run_at TIMESTAMP,
                    metadata_json TEXT
                );

                CREATE TABLE IF NOT EXISTS memory_relations (
                    id          TEXT PRIMARY KEY,
                    from_memory_id TEXT NOT NULL,
                    to_memory_id TEXT NOT NULL,
                    relation_type TEXT NOT NULL,
                    created_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    metadata_json TEXT
                );

                CREATE TABLE IF NOT EXISTS memory_evidence (
                    id          TEXT PRIMARY KEY,
                    memory_id   TEXT NOT NULL,
                    source_system TEXT NOT NULL,
                    source_id   TEXT NOT NULL,
                    source_revision TEXT,
                    relation    TEXT NOT NULL,
                    created_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    metadata_json TEXT
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
                """
            )
            # Run schema migration before creating indexes on migrated columns
            self._migrate_db(conn)

            conn.executescript(
                """
                CREATE INDEX IF NOT EXISTS idx_memories_layer ON memories(layer);
                CREATE INDEX IF NOT EXISTS idx_memories_expires ON memories(expires_at);
                CREATE INDEX IF NOT EXISTS idx_memories_lifecycle ON memories(lifecycle_state);
                CREATE INDEX IF NOT EXISTS idx_memories_verification ON memories(verification_state);
                CREATE INDEX IF NOT EXISTS idx_relations_from ON memory_relations(from_memory_id);
                CREATE INDEX IF NOT EXISTS idx_relations_to ON memory_relations(to_memory_id);
                CREATE INDEX IF NOT EXISTS idx_evidence_mem ON memory_evidence(memory_id);
                CREATE INDEX IF NOT EXISTS idx_evidence_src ON memory_evidence(source_system, source_id);
                CREATE INDEX IF NOT EXISTS idx_tree_parent ON tree_nodes(parent_id);
                CREATE INDEX IF NOT EXISTS idx_tree_layer ON tree_nodes(layer);
                CREATE INDEX IF NOT EXISTS idx_triggers_ts ON triggers_log(timestamp);
                CREATE INDEX IF NOT EXISTS idx_provenance_ts ON provenance(timestamp);
                CREATE INDEX IF NOT EXISTS idx_otel_spans_trace ON otel_spans(trace_id);
                CREATE INDEX IF NOT EXISTS idx_otel_spans_op ON otel_spans(operation);
                """
            )
            conn.commit()

        self._retry_write(_write)

    def _migrate_db(self, conn: sqlite3.Connection) -> None:
        """Migrate database schema up to version 4."""
        row = conn.execute("SELECT MAX(version) FROM schema_version").fetchone()
        current_version = row[0] if row and row[0] is not None else 2

        if current_version < 3:
            col_info = [r[1] for r in conn.execute("PRAGMA table_info(memories)").fetchall()]
            if "updated_at" not in col_info:
                conn.execute("ALTER TABLE memories ADD COLUMN updated_at TIMESTAMP")
            if "last_accessed_at" not in col_info:
                conn.execute("ALTER TABLE memories ADD COLUMN last_accessed_at TIMESTAMP")
            if "access_count" not in col_info:
                conn.execute("ALTER TABLE memories ADD COLUMN access_count INTEGER DEFAULT 0")

            conn.execute(
                """CREATE TABLE IF NOT EXISTS maintenance_state (
                    key TEXT PRIMARY KEY,
                    last_run_at TIMESTAMP,
                    metadata_json TEXT
                )"""
            )
            conn.execute("INSERT OR REPLACE INTO schema_version (version) VALUES (3)")
            current_version = 3

        if current_version < 4:
            col_info = [r[1] for r in conn.execute("PRAGMA table_info(memories)").fetchall()]
            if "claim_type" not in col_info:
                conn.execute("ALTER TABLE memories ADD COLUMN claim_type TEXT DEFAULT 'assertion'")
            if "lifecycle_state" not in col_info:
                conn.execute("ALTER TABLE memories ADD COLUMN lifecycle_state TEXT DEFAULT 'accepted'")
            if "verification_state" not in col_info:
                conn.execute("ALTER TABLE memories ADD COLUMN verification_state TEXT DEFAULT 'unverified'")
            if "observed_at" not in col_info:
                conn.execute("ALTER TABLE memories ADD COLUMN observed_at TIMESTAMP")
            if "valid_from" not in col_info:
                conn.execute("ALTER TABLE memories ADD COLUMN valid_from TIMESTAMP")
            if "valid_until" not in col_info:
                conn.execute("ALTER TABLE memories ADD COLUMN valid_until TIMESTAMP")

            conn.execute(
                """CREATE TABLE IF NOT EXISTS memory_relations (
                    id              TEXT PRIMARY KEY,
                    from_memory_id  TEXT NOT NULL,
                    to_memory_id    TEXT NOT NULL,
                    relation_type   TEXT NOT NULL,
                    created_at      TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    metadata_json   TEXT
                )"""
            )
            conn.execute("CREATE INDEX IF NOT EXISTS idx_relations_from ON memory_relations(from_memory_id)")
            conn.execute("CREATE INDEX IF NOT EXISTS idx_relations_to ON memory_relations(to_memory_id)")

            conn.execute(
                """CREATE TABLE IF NOT EXISTS memory_evidence (
                    id              TEXT PRIMARY KEY,
                    memory_id       TEXT NOT NULL,
                    source_system   TEXT NOT NULL,
                    source_id       TEXT NOT NULL,
                    source_revision TEXT,
                    relation        TEXT NOT NULL,
                    created_at      TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    metadata_json   TEXT
                )"""
            )
            conn.execute("CREATE INDEX IF NOT EXISTS idx_evidence_mem ON memory_evidence(memory_id)")
            conn.execute("CREATE INDEX IF NOT EXISTS idx_evidence_src ON memory_evidence(source_system, source_id)")

            conn.execute("INSERT OR REPLACE INTO schema_version (version) VALUES (4)")
        elif current_version == 4:
            conn.execute("INSERT OR IGNORE INTO schema_version (version) VALUES (4)")

    # --- Memory CRUD ---

    def _insert_memory_tx(
        self,
        conn: sqlite3.Connection,
        layer: str,
        content: str,
        source: str = "manual",
        confidence: float = 1.0,
        tags: Optional[List[str]] = None,
        expires_at: Optional[datetime] = None,
        memory_id: Optional[str] = None,
        created_at: Optional[datetime] = None,
        updated_at: Optional[datetime] = None,
        last_accessed_at: Optional[datetime] = None,
        access_count: int = 0,
        claim_type: str = "assertion",
        lifecycle_state: str = "accepted",
        verification_state: str = "unverified",
        observed_at: Optional[datetime] = None,
        valid_from: Optional[datetime] = None,
        valid_until: Optional[datetime] = None,
    ) -> str:
        """Single internal persistence boundary for all memory inserts.

        Validates, sanitizes secrets/PII, normalizes, and persists to SQLite.
        """
        if not content:
            raise ValueError("Memory content cannot be empty")
        if not layer:
            raise ValueError("Memory layer cannot be empty")
        if claim_type not in CLAIM_TYPES:
            raise ValueError(
                f"Invalid claim_type '{claim_type}'. Must be one of {sorted(CLAIM_TYPES)}"
            )
        if lifecycle_state not in LIFECYCLE_STATES:
            raise ValueError(
                f"Invalid lifecycle_state '{lifecycle_state}'. Must be one of {sorted(LIFECYCLE_STATES)}"
            )
        if verification_state not in VERIFICATION_STATES:
            raise ValueError(
                f"Invalid verification_state '{verification_state}'. Must be one of {sorted(VERIFICATION_STATES)}"
            )

        sanitized_content = sanitize_text(content)
        mid = memory_id or str(uuid.uuid4())
        now_iso = _now_iso()
        c_at = created_at.isoformat() if created_at else now_iso
        u_at = updated_at.isoformat() if updated_at else c_at
        e_at = expires_at.isoformat() if expires_at else None
        l_at = last_accessed_at.isoformat() if last_accessed_at else None
        obs_at = observed_at.isoformat() if observed_at else None
        v_from = valid_from.isoformat() if valid_from else None
        v_until = valid_until.isoformat() if valid_until else None

        cols = [r[1] for r in conn.execute("PRAGMA table_info(memories)").fetchall()]
        if "claim_type" in cols:
            conn.execute(
                """INSERT INTO memories (id, layer, content, source, confidence,
                                         created_at, expires_at, tags,
                                         updated_at, last_accessed_at, access_count,
                                         claim_type, lifecycle_state, verification_state,
                                         observed_at, valid_from, valid_until)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
                (
                    mid,
                    layer,
                    sanitized_content,
                    source,
                    confidence,
                    c_at,
                    e_at,
                    json.dumps(tags or []),
                    u_at,
                    l_at,
                    access_count,
                    claim_type,
                    lifecycle_state,
                    verification_state,
                    obs_at,
                    v_from,
                    v_until,
                ),
            )
        elif "updated_at" in cols:
            conn.execute(
                """INSERT INTO memories (id, layer, content, source, confidence,
                                         created_at, expires_at, tags,
                                         updated_at, last_accessed_at, access_count)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
                (
                    mid,
                    layer,
                    sanitized_content,
                    source,
                    confidence,
                    c_at,
                    e_at,
                    json.dumps(tags or []),
                    u_at,
                    l_at,
                    access_count,
                ),
            )
        else:
            conn.execute(
                """INSERT INTO memories (id, layer, content, source, confidence,
                                         created_at, expires_at, tags)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?)""",
                (
                    mid,
                    layer,
                    sanitized_content,
                    source,
                    confidence,
                    c_at,
                    e_at,
                    json.dumps(tags or []),
                ),
            )
        return mid

    def insert_memory(
        self,
        layer: str,
        content: str,
        source: str = "manual",
        confidence: float = 1.0,
        tags: Optional[List[str]] = None,
        expires_at: Optional[datetime] = None,
        claim_type: str = "assertion",
        lifecycle_state: str = "accepted",
        verification_state: str = "unverified",
        observed_at: Optional[datetime] = None,
        valid_from: Optional[datetime] = None,
        valid_until: Optional[datetime] = None,
    ) -> str:
        def _write(conn):
            mid = self._insert_memory_tx(
                conn=conn,
                layer=layer,
                content=content,
                source=source,
                confidence=confidence,
                tags=tags,
                expires_at=expires_at,
                claim_type=claim_type,
                lifecycle_state=lifecycle_state,
                verification_state=verification_state,
                observed_at=observed_at,
                valid_from=valid_from,
                valid_until=valid_until,
            )
            conn.commit()
            return mid

        return self._retry_write(_write)

    def get_memory(self, id: str, include_expired: bool = False) -> Optional[Memory]:
        with self._connect() as conn:
            row = conn.execute("SELECT * FROM memories WHERE id = ?", (id,)).fetchone()
            if not row:
                return None
            mem = Memory.from_row(row)
            if not include_expired and mem.expires_at and mem.expires_at < datetime.now():
                return None
            return mem

    def list_memories(
        self,
        layer: Optional[str] = None,
        include_expired: bool = False,
        include_candidates: bool = False,
        include_history: bool = False,
        lifecycle_state: Optional[str] = None,
        verification_state: Optional[str] = None,
        claim_type: Optional[str] = None,
    ) -> List[Memory]:
        now_iso = _now_iso()
        with self._connect() as conn:
            query = "SELECT * FROM memories"
            clauses = []
            params = []
            if layer:
                clauses.append("layer = ?")
                params.append(layer)
            if not include_expired:
                clauses.append("(expires_at IS NULL OR expires_at >= ?)")
                params.append(now_iso)

            cols = [r[1] for r in conn.execute("PRAGMA table_info(memories)").fetchall()]
            if "lifecycle_state" in cols:
                if lifecycle_state is not None:
                    clauses.append("lifecycle_state = ?")
                    params.append(lifecycle_state)
                elif not include_history:
                    if include_candidates:
                        clauses.append("lifecycle_state IN ('accepted', 'candidate')")
                    else:
                        clauses.append("lifecycle_state = 'accepted'")

                if verification_state is not None:
                    clauses.append("verification_state = ?")
                    params.append(verification_state)
                elif not include_history:
                    clauses.append("verification_state != 'refuted'")

                if claim_type is not None:
                    clauses.append("claim_type = ?")
                    params.append(claim_type)

            if clauses:
                query += " WHERE " + " AND ".join(clauses)
            query += " ORDER BY created_at DESC"
            rows = conn.execute(query, params).fetchall()
            return [Memory.from_row(r) for r in rows]

    def delete_memory(self, id: str) -> bool:
        def _write(conn):
            cur = conn.execute("DELETE FROM memories WHERE id = ?", (id,))
            conn.commit()
            return cur.rowcount > 0

        return self._retry_write(_write)

    def update_memory_layer(self, id: str, new_layer: str) -> bool:
        def _write(conn):
            cur = conn.execute(
                "UPDATE memories SET layer = ? WHERE id = ?",
                (new_layer, id),
            )
            conn.commit()
            return cur.rowcount > 0

        return self._retry_write(_write)

    def update_memory_confidence(self, id: str, new_confidence: float) -> bool:
        """Update the confidence score of a memory. Returns True if found."""

        def _write(conn):
            cur = conn.execute(
                "UPDATE memories SET confidence = ? WHERE id = ?",
                (new_confidence, id),
            )
            conn.commit()
            return cur.rowcount > 0

        return self._retry_write(_write)

    def get_memories_below_confidence(
        self, threshold: float, layer: Optional[str] = None, include_expired: bool = False
    ) -> List[Memory]:
        """Get all memories with confidence < threshold, optionally filtered by layer."""
        now_iso = _now_iso()
        with self._connect() as conn:
            clauses = ["confidence < ?"]
            params: list = [threshold]
            if layer:
                clauses.append("layer = ?")
                params.append(layer)
            if not include_expired:
                clauses.append("(expires_at IS NULL OR expires_at >= ?)")
                params.append(now_iso)

            query = f"SELECT * FROM memories WHERE {' AND '.join(clauses)}"
            rows = conn.execute(query, params).fetchall()
            return [Memory.from_row(r) for r in rows]

    def update_memory_timestamp(self, id: str) -> bool:
        """Update updated_at to now (created_at is immutable!)."""

        def _write(conn):
            cols = [r[1] for r in conn.execute("PRAGMA table_info(memories)").fetchall()]
            if "updated_at" in cols:
                cur = conn.execute(
                    "UPDATE memories SET updated_at = ? WHERE id = ?",
                    (_now_iso(), id),
                )
            else:
                cur = conn.execute("SELECT id FROM memories WHERE id = ?", (id,))
            conn.commit()
            return cur.rowcount > 0

        return self._retry_write(_write)

    def record_memory_access(self, memory_id: str) -> bool:
        """Record memory access: increments access_count and updates last_accessed_at.

        Does NOT modify created_at, confidence, or verification.
        """

        def _write(conn):
            cols = [r[1] for r in conn.execute("PRAGMA table_info(memories)").fetchall()]
            if "last_accessed_at" in cols and "access_count" in cols:
                cur = conn.execute(
                    """UPDATE memories
                       SET access_count = COALESCE(access_count, 0) + 1,
                           last_accessed_at = ?
                       WHERE id = ?""",
                    (_now_iso(), memory_id),
                )
            else:
                cur = conn.execute("SELECT id FROM memories WHERE id = ?", (memory_id,))
            conn.commit()
            return cur.rowcount > 0

        return self._retry_write(_write)

    # --- Knowledge Lifecycle & Verification State Operations ---

    def update_memory_lifecycle(self, id: str, state: str) -> bool:
        """Update lifecycle state of a memory (candidate, accepted, superseded, rejected, archived)."""
        if state not in LIFECYCLE_STATES:
            raise ValueError(
                f"Invalid lifecycle_state '{state}'. Must be one of {sorted(LIFECYCLE_STATES)}"
            )

        def _write(conn):
            cur = conn.execute(
                "UPDATE memories SET lifecycle_state = ?, updated_at = ? WHERE id = ?",
                (state, _now_iso(), id),
            )
            conn.commit()
            return cur.rowcount > 0

        return self._retry_write(_write)

    def update_memory_verification(self, id: str, state: str) -> bool:
        """Update verification state of a memory (unverified, supported, disputed, refuted)."""
        if state not in VERIFICATION_STATES:
            raise ValueError(
                f"Invalid verification_state '{state}'. Must be one of {sorted(VERIFICATION_STATES)}"
            )

        def _write(conn):
            cur = conn.execute(
                "UPDATE memories SET verification_state = ?, updated_at = ? WHERE id = ?",
                (state, _now_iso(), id),
            )
            conn.commit()
            return cur.rowcount > 0

        return self._retry_write(_write)

    def supersede_memory(
        self, old_memory_id: str, new_memory_id: str, metadata: Optional[dict] = None
    ) -> bool:
        """Mark old_memory as superseded by new_memory and record lineage relation."""

        def _write(conn):
            cur = conn.execute(
                "UPDATE memories SET lifecycle_state = 'superseded', updated_at = ? WHERE id = ?",
                (_now_iso(), old_memory_id),
            )
            if cur.rowcount == 0:
                conn.commit()
                return False
            rel_id = str(uuid.uuid4())
            conn.execute(
                """INSERT INTO memory_relations (id, from_memory_id, to_memory_id, relation_type, created_at, metadata_json)
                   VALUES (?, ?, ?, 'supersedes', ?, ?)""",
                (rel_id, new_memory_id, old_memory_id, _now_iso(), json.dumps(metadata or {})),
            )
            conn.commit()
            return True

        return self._retry_write(_write)

    def refute_memory(
        self, memory_id: str, reason: str, refuting_memory_id: Optional[str] = None
    ) -> bool:
        """Mark memory as refuted (verification_state='refuted', lifecycle_state='rejected')."""

        def _write(conn):
            cur = conn.execute(
                """UPDATE memories
                   SET verification_state = 'refuted',
                       lifecycle_state = 'rejected',
                       updated_at = ?
                   WHERE id = ?""",
                (_now_iso(), memory_id),
            )
            if cur.rowcount == 0:
                conn.commit()
                return False
            if refuting_memory_id:
                rel_id = str(uuid.uuid4())
                conn.execute(
                    """INSERT INTO memory_relations (id, from_memory_id, to_memory_id, relation_type, created_at, metadata_json)
                       VALUES (?, ?, ?, 'refutes', ?, ?)""",
                    (rel_id, refuting_memory_id, memory_id, _now_iso(), json.dumps({"reason": reason})),
                )
            conn.commit()
            return True

        return self._retry_write(_write)

    # --- Memory Relations ---

    def add_memory_relation(
        self,
        from_memory_id: str,
        to_memory_id: str,
        relation_type: str,
        metadata: Optional[dict] = None,
    ) -> str:
        """Record a directed relation between two memories."""
        if relation_type not in RELATION_TYPES:
            raise ValueError(
                f"Invalid relation_type '{relation_type}'. Must be one of {sorted(RELATION_TYPES)}"
            )
        rel_id = str(uuid.uuid4())

        def _write(conn):
            conn.execute(
                """INSERT INTO memory_relations (id, from_memory_id, to_memory_id, relation_type, created_at, metadata_json)
                   VALUES (?, ?, ?, ?, ?, ?)""",
                (rel_id, from_memory_id, to_memory_id, relation_type, _now_iso(), json.dumps(metadata or {})),
            )
            conn.commit()
            return rel_id

        return self._retry_write(_write)

    def get_memory_relations(
        self, memory_id: str, direction: str = "both"
    ) -> List[MemoryRelation]:
        """Get relations connected to a memory (outgoing, incoming, or both)."""
        with self._connect() as conn:
            if direction == "outgoing":
                rows = conn.execute(
                    "SELECT * FROM memory_relations WHERE from_memory_id = ? ORDER BY created_at ASC",
                    (memory_id,),
                ).fetchall()
            elif direction == "incoming":
                rows = conn.execute(
                    "SELECT * FROM memory_relations WHERE to_memory_id = ? ORDER BY created_at ASC",
                    (memory_id,),
                ).fetchall()
            else:
                rows = conn.execute(
                    "SELECT * FROM memory_relations WHERE from_memory_id = ? OR to_memory_id = ? ORDER BY created_at ASC",
                    (memory_id, memory_id),
                ).fetchall()
            return [MemoryRelation.from_row(r) for r in rows]

    # --- Memory External Evidence References ---

    def add_memory_evidence(
        self,
        memory_id: str,
        source_system: str,
        source_id: str,
        relation: str = "derived_from",
        source_revision: Optional[str] = None,
        metadata: Optional[dict] = None,
    ) -> str:
        """Record an external evidence reference linking memory to an external source record."""
        ev_id = str(uuid.uuid4())

        def _write(conn):
            conn.execute(
                """INSERT INTO memory_evidence (id, memory_id, source_system, source_id, source_revision, relation, created_at, metadata_json)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?)""",
                (
                    ev_id,
                    memory_id,
                    source_system,
                    source_id,
                    source_revision,
                    relation,
                    _now_iso(),
                    json.dumps(metadata or {}),
                ),
            )
            conn.commit()
            return ev_id

        return self._retry_write(_write)

    def get_memory_evidence(self, memory_id: str) -> List[MemoryEvidence]:
        """Get external evidence references for a memory."""
        with self._connect() as conn:
            rows = conn.execute(
                "SELECT * FROM memory_evidence WHERE memory_id = ? ORDER BY created_at ASC",
                (memory_id,),
            ).fetchall()
            return [MemoryEvidence.from_row(r) for r in rows]

    # --- Maintenance State (Persistent Decay & Tasks) ---

    def record_maintenance(
        self,
        key: str,
        run_at: Optional[datetime] = None,
        metadata: Optional[dict] = None,
    ) -> None:
        """Record maintenance execution in SQLite maintenance_state table."""
        ts = run_at.isoformat() if run_at else _now_iso()
        meta_json = json.dumps(metadata) if metadata else None

        def _write(conn):
            conn.execute(
                """INSERT OR REPLACE INTO maintenance_state (key, last_run_at, metadata_json)
                   VALUES (?, ?, ?)""",
                (key, ts, meta_json),
            )
            conn.commit()

        self._retry_write(_write)

    def get_last_maintenance(self, key: str) -> Optional[datetime]:
        """Get the timestamp of the last maintenance run for a key."""
        with self._connect() as conn:
            row = conn.execute(
                "SELECT last_run_at FROM maintenance_state WHERE key = ?", (key,)
            ).fetchone()
            if row and row["last_run_at"]:
                return _parse_dt(row["last_run_at"])
            return None

    # --- Expiration ---

    def expire_old_memories(self) -> int:
        """Delete memories where expires_at < now(). Returns count."""

        def _write(conn):
            cur = conn.execute(
                "DELETE FROM memories WHERE expires_at IS NOT NULL AND expires_at < ?",
                (_now_iso(),),
            )
            conn.commit()
            return cur.rowcount

        return self._retry_write(_write)

    # --- Decay (auto-triggered) ---

    def run_decay_if_needed(self, decay_engine, min_hours: float = 24.0) -> bool:
        """Run confidence decay if enough time has passed since last run.

        Uses persistent maintenance_state in SQLite so maintenance is not forgotten
        when a new process starts.
        """
        now = datetime.now()
        last_decay = self.get_last_maintenance("confidence_decay")
        if last_decay is None:
            # Baseline: record now so decay is evaluated on future runs after min_hours
            self.record_maintenance("confidence_decay", now, {"init": True})
            return False

        if (now - last_decay).total_seconds() < min_hours * 3600:
            return False

        decayed = decay_engine.decay_memories()
        self.record_maintenance(
            "confidence_decay", now, {"decayed_count": len(decayed)}
        )
        return len(decayed) > 0

    # --- Consolidation ---

    def consolidate(self, from_layer: str, to_layer: str) -> List[str]:
        """Move all memories from from_layer to to_layer. Returns moved IDs."""

        def _write(conn):
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

        return self._retry_write(_write)

    def consolidate_and_log(
        self,
        from_layer: str,
        to_layer: str,
        event: str,
        action: str,
    ) -> List[str]:
        """Atomically consolidate memories and log trigger."""

        def _write(conn):
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

        return self._retry_write(_write)

    def consolidate_with_audit(
        self,
        from_layer: str,
        to_layer: str,
        reflection_content: str,
        reflection_source: str,
        event: str,
        action: str,
        move_memories: bool = True,
        claim_type: str = "derived_lesson",
        lifecycle_state: str = "candidate",
        verification_state: str = "unverified",
    ) -> tuple[List[str], Optional[str]]:
        """Atomically consolidate memories, create reflection candidate, and log trigger."""

        def _write(conn):
            rows = conn.execute(
                "SELECT id FROM memories WHERE layer = ?", (from_layer,)
            ).fetchall()
            ids = [r["id"] for r in rows]

            if not ids:
                conn.commit()
                return [], None

            # 1. Move memories if requested (legacy behavior); otherwise preserve in source layer
            if move_memories:
                placeholders = ",".join("?" * len(ids))
                conn.execute(
                    f"UPDATE memories SET layer = ? WHERE id IN ({placeholders})",
                    (to_layer, *ids),
                )

            # 2. Create reflection memory via centralized persistence boundary (sanitizes content)
            rid = self._insert_memory_tx(
                conn=conn,
                layer=to_layer,
                content=reflection_content,
                source=reflection_source,
                confidence=0.7,
                tags=["reflection", event, "auto-consolidated"],
                claim_type=claim_type,
                lifecycle_state=lifecycle_state,
                verification_state=verification_state,
            )

            # 3. Add derived_from relations linking candidate to each source memory
            cols = [r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()]
            if "memory_relations" in cols:
                for mid in ids:
                    rel_id = str(uuid.uuid4())
                    conn.execute(
                        """INSERT INTO memory_relations (id, from_memory_id, to_memory_id, relation_type, created_at, metadata_json)
                           VALUES (?, ?, ?, 'derived_from', ?, ?)""",
                        (rel_id, rid, mid, _now_iso(), json.dumps({"event": event, "action": action})),
                    )

            # 4. Log trigger
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

        return self._retry_write(_write)

    # --- Tree nodes (ATOMIC rebuild) ---

    def rebuild_tree_atomic(self, nodes: List[TreeNode]) -> None:
        """Atomically replace all tree nodes.

        This is the CRITICAL fix for CR-1: previously, clear_tree_nodes()
        and insert_tree_node() were separate transactions. A crash between
        them left an empty tree. Now the entire rebuild is a single transaction.
        """

        def _write(conn):
            conn.execute("DELETE FROM tree_nodes")
            self._insert_nodes_recursive(conn, nodes, parent_id=None)
            conn.commit()

        self._retry_write(_write)

    def _insert_nodes_recursive(
        self, conn, nodes: List[TreeNode], parent_id: Optional[str] = None
    ) -> None:
        for node in nodes:
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
            if node.children:
                self._insert_nodes_recursive(conn, node.children, parent_id=node.id)

    def clear_tree_nodes(self) -> None:
        def _write(conn):
            conn.execute("DELETE FROM tree_nodes")
            conn.commit()

        self._retry_write(_write)

    def insert_tree_node(self, node: TreeNode, parent_id: Optional[str] = None) -> str:
        def _write(conn):
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

        return self._retry_write(_write)

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

        def _write(conn):
            conn.execute(
                """INSERT INTO triggers_log (id, event, action,
                                              memories_affected, timestamp)
                   VALUES (?, ?, ?, ?, ?)""",
                (tid, event, action, json.dumps(memory_ids), _now_iso()),
            )
            conn.commit()
            return tid

        return self._retry_write(_write)

    def get_trigger_log(self, limit: int = 50) -> List[TriggerLog]:
        with self._connect() as conn:
            rows = conn.execute(
                "SELECT * FROM triggers_log ORDER BY timestamp DESC LIMIT ?",
                (limit,),
            ).fetchall()
            return [TriggerLog.from_row(r) for r in rows]

    # --- Provenance ---

    def save_provenance(self, provenance: dict) -> str:
        pid = str(uuid.uuid4())

        def _write(conn):
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

        return self._retry_write(_write)

    def get_provenance(self, limit: int = 100, offset: int = 0) -> List[dict]:
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
                    "sources": json.loads(r["sources_json"])
                    if r["sources_json"]
                    else [],
                }
                for r in rows
            ]

    def clear_provenance(self) -> None:
        def _write(conn):
            conn.execute("DELETE FROM provenance")
            conn.commit()

        self._retry_write(_write)

    # --- OTel spans ---

    def save_otel_span(self, span: dict) -> str:
        sid = str(uuid.uuid4())

        def _write(conn):
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

        return self._retry_write(_write)

    def get_otel_spans(
        self, limit: int = 1000, offset: int = 0, trace_id: Optional[str] = None
    ) -> List[dict]:
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
        def _write(conn):
            conn.execute("DELETE FROM otel_spans")
            conn.commit()

        self._retry_write(_write)

    def prune_otel_spans(self, max_rows: int = 10000) -> int:
        def _write(conn):
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

        return self._retry_write(_write)

    def wal_checkpoint(self) -> None:
        """Run WAL checkpoint to prevent unbounded .db-wal growth.

        Raises sqlite3.OperationalError if checkpoint is blocked by active readers.
        """
        with self._connect() as conn:
            row = conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchone()
            if row and row[0] != 0:
                raise sqlite3.OperationalError(
                    f"WAL checkpoint blocked: busy={row[0]}"
                )

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
            span_count = conn.execute("SELECT COUNT(*) FROM otel_spans").fetchone()[0]
            return {
                "memories": mem_count,
                "tree_nodes": node_count,
                "triggers": trigger_count,
                "provenance_records": provenance_count,
                "otel_spans": span_count,
            }
