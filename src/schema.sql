-- Additive Python-v4-compatible tables. Indexes/triggers are installed after migration.
CREATE TABLE IF NOT EXISTS schema_version(version INTEGER PRIMARY KEY);
CREATE TABLE IF NOT EXISTS memories (
 id TEXT PRIMARY KEY,layer TEXT NOT NULL,content TEXT NOT NULL,source TEXT,
 confidence REAL DEFAULT 1.0,created_at TEXT DEFAULT CURRENT_TIMESTAMP,expires_at TEXT,tags TEXT,
 updated_at TEXT,last_accessed_at TEXT,access_count INTEGER DEFAULT 0,
 claim_type TEXT DEFAULT 'assertion',lifecycle_state TEXT DEFAULT 'accepted',verification_state TEXT DEFAULT 'unverified',
 observed_at TEXT,valid_from TEXT,valid_until TEXT
);
CREATE TABLE IF NOT EXISTS maintenance_state(key TEXT PRIMARY KEY,last_run_at TEXT,metadata_json TEXT);
CREATE TABLE IF NOT EXISTS memory_relations(id TEXT PRIMARY KEY,from_memory_id TEXT NOT NULL,to_memory_id TEXT NOT NULL,relation_type TEXT NOT NULL,created_at TEXT DEFAULT CURRENT_TIMESTAMP,metadata_json TEXT);
CREATE TABLE IF NOT EXISTS memory_evidence(id TEXT PRIMARY KEY,memory_id TEXT NOT NULL,source_system TEXT NOT NULL,source_id TEXT NOT NULL,source_revision TEXT,relation TEXT NOT NULL,created_at TEXT DEFAULT CURRENT_TIMESTAMP,metadata_json TEXT);
CREATE TABLE IF NOT EXISTS tree_nodes(id TEXT PRIMARY KEY,parent_id TEXT REFERENCES tree_nodes(id),layer TEXT NOT NULL,title TEXT NOT NULL,summary TEXT,memory_ids TEXT,updated_at TEXT DEFAULT CURRENT_TIMESTAMP);
CREATE TABLE IF NOT EXISTS triggers_log(id TEXT PRIMARY KEY,event TEXT NOT NULL,action TEXT NOT NULL,memories_affected TEXT,timestamp TEXT DEFAULT CURRENT_TIMESTAMP);
CREATE TABLE IF NOT EXISTS provenance(id TEXT PRIMARY KEY,query TEXT NOT NULL,timestamp TEXT DEFAULT CURRENT_TIMESTAMP,method TEXT NOT NULL,tree_version INTEGER DEFAULT 0,total_memories_searched INTEGER DEFAULT 0,avg_confidence REAL DEFAULT 0.0,sources_json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS otel_spans(id TEXT PRIMARY KEY,trace_id TEXT NOT NULL,span_id TEXT NOT NULL,operation TEXT NOT NULL,timestamp REAL NOT NULL,duration_ms REAL NOT NULL,memory_id TEXT,layer TEXT,memory_type TEXT,confidence REAL,query TEXT,top_k INTEGER,results_count INTEGER,status TEXT NOT NULL,error_message TEXT,attributes_json TEXT,service_name TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS rust_revision(id INTEGER PRIMARY KEY CHECK(id=1),revision INTEGER NOT NULL);
INSERT OR IGNORE INTO rust_revision VALUES(1,0);
CREATE TABLE IF NOT EXISTS rust_query_cache(key TEXT PRIMARY KEY,result_json TEXT,revision INTEGER,cached_at REAL);
CREATE TABLE IF NOT EXISTS query_cache(query TEXT PRIMARY KEY,result_json TEXT NOT NULL,tree_version INTEGER NOT NULL,cached_at REAL NOT NULL,ttl_seconds REAL NOT NULL);
CREATE TABLE IF NOT EXISTS cache_meta(key TEXT PRIMARY KEY,value INTEGER);

CREATE TABLE IF NOT EXISTS agent_checkpoints(thread_id TEXT PRIMARY KEY,state_json TEXT NOT NULL,updated_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS memory_citations(id TEXT PRIMARY KEY,memory_id TEXT NOT NULL,path TEXT NOT NULL,line_start INTEGER,line_end INTEGER,file_hash TEXT NOT NULL,span_hash TEXT,head_hash TEXT,commit_sha TEXT,created_at TEXT NOT NULL,refreshed_at TEXT);
