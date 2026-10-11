BEGIN TRANSACTION;
CREATE TABLE maintenance_state (
                    key         TEXT PRIMARY KEY,
                    last_run_at TIMESTAMP,
                    metadata_json TEXT
                );
CREATE TABLE memories (
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
INSERT INTO "memories" VALUES('0254b5b0-41ea-4364-9575-b4f576e4ddba','project','We use Rust for the backend API','fixture',1.0,'2026-10-07T21:27:33.652087',NULL,'[]','2026-10-07T21:27:33.652087',NULL,0,'assertion','accepted','unverified',NULL,NULL,NULL);
INSERT INTO "memories" VALUES('dc78a624-c191-4f36-8805-6aa775af14b3','project','PostgreSQL is our database','fixture',1.0,'2026-10-07T21:27:33.652598',NULL,'[]','2026-10-07T21:27:33.652598',NULL,0,'assertion','accepted','unverified',NULL,NULL,NULL);
INSERT INTO "memories" VALUES('52983596-9ac2-4805-9c32-49b0e692000c','project','Authentication uses JWT middleware','fixture',1.0,'2026-10-07T21:27:33.652939',NULL,'[]','2026-10-07T21:27:33.652939',NULL,0,'assertion','accepted','unverified',NULL,NULL,NULL);
INSERT INTO "memories" VALUES('dbf86686-8c3d-494f-8707-da0d143fb69f','session','Investigating deployment errors in Kubernetes','fixture',0.7,'2026-10-07T21:27:33.653368',NULL,'[]','2026-10-07T21:27:33.653368',NULL,0,'assertion','accepted','unverified',NULL,NULL,NULL);
INSERT INTO "memories" VALUES('cbd8be1e-82fc-416c-9c5e-5f95c2236aab','user','I prefer concise explanations','fixture',1.0,'2026-10-07T21:27:33.653751',NULL,'[]','2026-10-07T21:27:33.653751',NULL,0,'assertion','accepted','unverified',NULL,NULL,NULL);
INSERT INTO "memories" VALUES('fe06008e-15cc-4cf4-9c67-ffe2e8425d12','project','Redis cache improves API latency','fixture',0.7,'2026-10-07T21:27:33.654067',NULL,'[]','2026-10-07T21:27:33.654067',NULL,0,'assertion','accepted','unverified',NULL,NULL,NULL);
INSERT INTO "memories" VALUES('d9ddcb51-fed6-4e1e-b33e-b1df15c097f8','session','Maybe use Python for a prototype','fixture',0.5,'2026-10-07T21:27:33.654559',NULL,'[]','2026-10-07T21:27:33.654559',NULL,0,'assertion','accepted','unverified',NULL,NULL,NULL);
CREATE TABLE memory_evidence (
                    id          TEXT PRIMARY KEY,
                    memory_id   TEXT NOT NULL,
                    source_system TEXT NOT NULL,
                    source_id   TEXT NOT NULL,
                    source_revision TEXT,
                    relation    TEXT NOT NULL,
                    created_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    metadata_json TEXT
                );
CREATE TABLE memory_relations (
                    id          TEXT PRIMARY KEY,
                    from_memory_id TEXT NOT NULL,
                    to_memory_id TEXT NOT NULL,
                    relation_type TEXT NOT NULL,
                    created_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    metadata_json TEXT
                );
CREATE TABLE otel_spans (
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
CREATE TABLE provenance (
                    id          TEXT PRIMARY KEY,
                    query       TEXT NOT NULL,
                    timestamp   TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    method      TEXT NOT NULL,
                    tree_version INTEGER DEFAULT 0,
                    total_memories_searched INTEGER DEFAULT 0,
                    avg_confidence REAL DEFAULT 0.0,
                    sources_json TEXT NOT NULL
                );
CREATE TABLE schema_version (
                    version INTEGER PRIMARY KEY
                );
INSERT INTO "schema_version" VALUES(3);
INSERT INTO "schema_version" VALUES(4);
CREATE TABLE tree_nodes (
                    id          TEXT PRIMARY KEY,
                    parent_id   TEXT REFERENCES tree_nodes(id),
                    layer       TEXT NOT NULL,
                    title       TEXT NOT NULL,
                    summary     TEXT,
                    memory_ids  TEXT,
                    updated_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                );
CREATE TABLE triggers_log (
                    id          TEXT PRIMARY KEY,
                    event       TEXT NOT NULL,
                    action      TEXT NOT NULL,
                    memories_affected TEXT,
                    timestamp   TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                );
CREATE INDEX idx_relations_from ON memory_relations(from_memory_id);
CREATE INDEX idx_relations_to ON memory_relations(to_memory_id);
CREATE INDEX idx_evidence_mem ON memory_evidence(memory_id);
CREATE INDEX idx_evidence_src ON memory_evidence(source_system, source_id);
CREATE INDEX idx_memories_layer ON memories(layer);
CREATE INDEX idx_memories_expires ON memories(expires_at);
CREATE INDEX idx_memories_lifecycle ON memories(lifecycle_state);
CREATE INDEX idx_memories_verification ON memories(verification_state);
CREATE INDEX idx_tree_parent ON tree_nodes(parent_id);
CREATE INDEX idx_tree_layer ON tree_nodes(layer);
CREATE INDEX idx_triggers_ts ON triggers_log(timestamp);
CREATE INDEX idx_provenance_ts ON provenance(timestamp);
CREATE INDEX idx_otel_spans_trace ON otel_spans(trace_id);
CREATE INDEX idx_otel_spans_op ON otel_spans(operation);
COMMIT;
