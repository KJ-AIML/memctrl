//! SQLite storage preserving Python v4 data and transactional lineage.
use crate::{model::*, sanitize::*};
use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use uuid::Uuid;

pub struct MemoryStore {
    pub conn: Connection,
}
// Savepoints compose with a caller's transaction; Drop guarantees rollback on errors/panics.
pub(crate) struct AtomicWrite<'a> {
    conn: &'a Connection,
    name: String,
    active: bool,
}
impl<'a> AtomicWrite<'a> {
    pub(crate) fn new(conn: &'a Connection) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!("memctrl_{}", NEXT.fetch_add(1, Ordering::Relaxed));
        conn.execute_batch(&format!("SAVEPOINT {name}"))?;
        Ok(Self {
            conn,
            name,
            active: true,
        })
    }
    pub(crate) fn commit(mut self) -> Result<()> {
        self.conn.execute_batch(&format!("RELEASE {}", self.name))?;
        self.active = false;
        Ok(())
    }
}
impl Drop for AtomicWrite<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ = self
                .conn
                .execute_batch(&format!("ROLLBACK TO {}; RELEASE {}", self.name, self.name));
        }
    }
}
fn sql_error(error: anyhow::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, error.into())
}
fn semantic<T: std::str::FromStr<Err = anyhow::Error> + Default>(
    row: &Row<'_>,
    column: &str,
) -> rusqlite::Result<T> {
    match row.get::<_, Option<String>>(column)? {
        Some(s) if !s.trim().is_empty() => s.parse().map_err(sql_error),
        _ => Ok(T::default()),
    }
}
fn memory_row(row: &Row<'_>) -> rusqlite::Result<Memory> {
    let tags: Option<String> = row.get("tags")?;
    Ok(Memory {
        id: row.get("id")?,
        layer: sanitize_text(&row.get::<_, String>("layer")?),
        content: sanitize_text(&row.get::<_, String>("content")?),
        source: sanitize_text(&row.get::<_, Option<String>>("source")?.unwrap_or_default()),
        confidence: row.get::<_, Option<f64>>("confidence")?.unwrap_or(1.0),
        created_at: row
            .get::<_, Option<String>>("created_at")?
            .unwrap_or_else(now),
        updated_at: row.get("updated_at")?,
        last_accessed_at: row.get("last_accessed_at")?,
        access_count: row
            .get::<_, Option<i64>>("access_count")?
            .unwrap_or(0)
            .max(0) as u64,
        tags: tags
            .filter(|s| !s.is_empty())
            .map(|s| serde_json::from_str::<Vec<String>>(&s))
            .transpose()
            .map_err(|e| sql_error(e.into()))?
            .unwrap_or_default()
            .iter()
            .map(|s| sanitize_text(s))
            .collect(),
        expires_at: row.get("expires_at")?,
        observed_at: row.get("observed_at")?,
        valid_from: row.get("valid_from")?,
        valid_until: row.get("valid_until")?,
        claim_type: semantic(row, "claim_type")?,
        lifecycle_state: semantic(row, "lifecycle_state")?,
        verification_state: semantic(row, "verification_state")?,
    })
}
fn json_column(row: &Row<'_>, name: &str) -> rusqlite::Result<Value> {
    row.get::<_, Option<String>>(name)?
        .filter(|s| !s.is_empty())
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|e| sql_error(e.into()))
        .map(|v| sanitize_value(&v.unwrap_or_else(|| json!({}))))
}
impl MemoryStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path != Path::new(":memory:") {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        let version_table:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_version')",[],|r|r.get(0))?;
        if version_table {
            let version: Option<i64> =
                conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))?;
            if version.is_some_and(|v| v > 4) {
                bail!("unsupported future schema version")
            }
        }
        conn.busy_timeout(Duration::from_secs(30))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        let store = Self { conn };
        store.initialize()?;
        Ok(store)
    }
    fn atomic<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        let guard = AtomicWrite::new(&self.conn)?;
        let result = operation()?;
        guard.commit()?;
        Ok(result)
    }
    fn initialize(&self) -> Result<()> {
        self.atomic(||{
        self.conn.execute_batch(include_str!("schema.sql"))?;
        let mut statement=self.conn.prepare("PRAGMA table_info(memories)")?;
        let columns:HashSet<String>=statement.query_map([],|row|row.get(1))?.collect::<rusqlite::Result<_>>()?;drop(statement);
        for (name,definition) in [("updated_at","TEXT"),("last_accessed_at","TEXT"),("access_count","INTEGER DEFAULT 0"),("claim_type","TEXT DEFAULT 'assertion'"),("lifecycle_state","TEXT DEFAULT 'accepted'"),("verification_state","TEXT DEFAULT 'unverified'"),("observed_at","TEXT"),("valid_from","TEXT"),("valid_until","TEXT")] {
            if !columns.contains(name){self.conn.execute_batch(&format!("ALTER TABLE memories ADD COLUMN {name} {definition}"))?;}
        }
        self.conn.execute_batch("UPDATE memories SET claim_type=COALESCE(NULLIF(trim(claim_type),''),'assertion'), lifecycle_state=COALESCE(NULLIF(trim(lifecycle_state),''),'accepted'), verification_state=COALESCE(NULLIF(trim(verification_state),''),'unverified') WHERE claim_type IS NULL OR trim(claim_type)='' OR lifecycle_state IS NULL OR trim(lifecycle_state)='' OR verification_state IS NULL OR trim(verification_state)=''; INSERT OR IGNORE INTO schema_version VALUES(4);
CREATE INDEX IF NOT EXISTS idx_memories_layer ON memories(layer);
CREATE INDEX IF NOT EXISTS idx_memories_expires ON memories(expires_at);
CREATE INDEX IF NOT EXISTS idx_memories_lifecycle ON memories(lifecycle_state);
CREATE INDEX IF NOT EXISTS idx_memories_verification ON memories(verification_state);
CREATE INDEX IF NOT EXISTS idx_relations_from ON memory_relations(from_memory_id);
CREATE INDEX IF NOT EXISTS idx_relations_to ON memory_relations(to_memory_id);
CREATE INDEX IF NOT EXISTS idx_evidence_mem ON memory_evidence(memory_id);
CREATE INDEX IF NOT EXISTS idx_evidence_src ON memory_evidence(source_system,source_id);
CREATE INDEX IF NOT EXISTS idx_tree_parent ON tree_nodes(parent_id);
CREATE INDEX IF NOT EXISTS idx_provenance_ts ON provenance(timestamp);
CREATE INDEX IF NOT EXISTS idx_otel_spans_trace ON otel_spans(trace_id);")?;
        for table in ["memories","tree_nodes","memory_relations","memory_evidence"]{
            for event in ["INSERT","DELETE","UPDATE"]{
                let clause=if table=="memories" && event=="UPDATE" {"UPDATE OF layer,content,source,confidence,created_at,expires_at,tags,claim_type,lifecycle_state,verification_state,observed_at,valid_from,valid_until"}else{event};
                self.conn.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS rust_revision_{table}_{} AFTER {clause} ON {table} BEGIN UPDATE rust_revision SET revision=revision+1 WHERE id=1; DELETE FROM rust_query_cache; DELETE FROM query_cache; END;",event.to_lowercase()))?;
            }
        }
        Ok(())
    })
    }
    pub fn insert_memory(&self, mut memory: NewMemory) -> Result<String> {
        memory.validate()?;
        memory.content = sanitize_text(&memory.content);
        memory.source = sanitize_text(&memory.source);
        memory.tags = memory.tags.iter().map(|s| sanitize_text(s)).collect();
        for timestamp in [
            &mut memory.expires_at,
            &mut memory.observed_at,
            &mut memory.valid_from,
            &mut memory.valid_until,
        ]
        .into_iter()
        .flatten()
        {
            *timestamp = parse_time(timestamp)?.to_rfc3339();
        }
        let id = Uuid::new_v4().to_string();
        let timestamp = now();
        self.conn.execute("INSERT INTO memories(id,layer,content,source,confidence,created_at,expires_at,tags,updated_at,access_count,claim_type,lifecycle_state,verification_state,observed_at,valid_from,valid_until) VALUES(?,?,?,?,?,?,?,?,?,0,?,?,?,?,?,?)",params![id,memory.layer,memory.content,memory.source,memory.confidence,timestamp,memory.expires_at,serde_json::to_string(&memory.tags)?,timestamp,memory.claim_type.to_string(),memory.lifecycle_state.to_string(),memory.verification_state.to_string(),memory.observed_at,memory.valid_from,memory.valid_until])?;
        Ok(id)
    }
    pub fn get_memory(&self, id: &str, include_history: bool) -> Result<Option<Memory>> {
        let memory = self
            .conn
            .query_row("SELECT * FROM memories WHERE id=?", [id], memory_row)
            .optional()?;
        Ok(memory.filter(|m| include_history || m.eligible()))
    }
    pub fn list_memories(&self, layer: Option<&str>, include_history: bool) -> Result<Vec<Memory>> {
        if let Some(layer) = layer {
            validate_identifier(layer, "layer")?;
        }
        let mut statement = self.conn.prepare(
            "SELECT * FROM memories WHERE (? IS NULL OR layer=?) ORDER BY created_at DESC,id",
        )?;
        let memories = statement
            .query_map(params![layer, layer], memory_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(memories
            .into_iter()
            .filter(|m| include_history || m.eligible())
            .collect())
    }
    fn require_memory(&self, id: &str) -> Result<()> {
        if self.get_memory(id, true)?.is_none() {
            bail!("memory does not exist")
        }
        Ok(())
    }
    pub fn delete_memory(&self, id: &str) -> Result<bool> {
        self.atomic(|| {
            let count = self.conn.execute("DELETE FROM memories WHERE id=?", [id])?;
            if count > 0 {
                self.conn.execute(
                    "DELETE FROM memory_relations WHERE from_memory_id=? OR to_memory_id=?",
                    params![id, id],
                )?;
                self.conn
                    .execute("DELETE FROM memory_evidence WHERE memory_id=?", [id])?;
                self.conn.execute("DELETE FROM tree_nodes", [])?;
            }
            Ok(count > 0)
        })
    }
    pub fn update_layer(&self, id: &str, layer: &str) -> Result<bool> {
        validate_identifier(layer, "layer")?;
        Ok(self.conn.execute(
            "UPDATE memories SET layer=?,updated_at=? WHERE id=?",
            params![layer, now(), id],
        )? > 0)
    }
    pub fn update_confidence(&self, id: &str, confidence: f64) -> Result<bool> {
        validate_confidence(confidence)?;
        Ok(self.conn.execute(
            "UPDATE memories SET confidence=?,updated_at=? WHERE id=?",
            params![confidence, now(), id],
        )? > 0)
    }
    pub fn record_access(&self, id: &str) -> Result<bool> {
        Ok(self.conn.execute("UPDATE memories SET access_count=COALESCE(access_count,0)+1,last_accessed_at=? WHERE id=?",params![now(),id])?>0)
    }
    pub fn update_lifecycle(&self, id: &str, state: LifecycleState) -> Result<bool> {
        self.atomic(|| {
            if state == LifecycleState::Accepted {
                if let Some(m) = self.get_memory(id, true)? {
                    if m.verification_state == VerificationState::Refuted {
                        bail!("refuted memory cannot be accepted")
                    }
                }
            }
            Ok(self.conn.execute(
                "UPDATE memories SET lifecycle_state=?,updated_at=? WHERE id=?",
                params![state.to_string(), now(), id],
            )? > 0)
        })
    }
    pub fn update_verification(&self, id: &str, state: VerificationState) -> Result<bool> {
        Ok(self.conn.execute("UPDATE memories SET verification_state=?,lifecycle_state=CASE WHEN ?='refuted' THEN 'rejected' ELSE lifecycle_state END,updated_at=? WHERE id=?",params![state.to_string(),state.to_string(),now(),id])?>0)
    }
    pub fn supersede_memory(&self, old: &str, new: &str, metadata: Value) -> Result<()> {
        if old == new {
            bail!("memory cannot supersede itself")
        }
        self.atomic(|| {
            self.require_memory(old)?;
            self.require_memory(new)?;
            self.update_lifecycle(old, LifecycleState::Superseded)?;
            self.add_relation(new, old, RelationType::Supersedes, metadata)?;
            Ok(())
        })
    }
    pub fn refute_memory(&self, id: &str, reason: &str, refuter: Option<&str>) -> Result<()> {
        if refuter == Some(id) {
            bail!("memory cannot refute itself")
        }
        self.atomic(|| {
            self.require_memory(id)?;
            if let Some(refuter) = refuter {
                self.require_memory(refuter)?;
            }
            self.update_verification(id, VerificationState::Refuted)?;
            if let Some(refuter) = refuter {
                self.add_relation(
                    refuter,
                    id,
                    RelationType::Refutes,
                    json!({"reason":sanitize_text(reason)}),
                )?;
            }
            Ok(())
        })
    }
    pub fn add_relation(
        &self,
        from: &str,
        to: &str,
        kind: RelationType,
        metadata: Value,
    ) -> Result<String> {
        if from == to {
            bail!("self referencing relation is invalid")
        }
        self.atomic(||{self.require_memory(from)?;self.require_memory(to)?;let id=Uuid::new_v4().to_string();self.conn.execute("INSERT INTO memory_relations(id,from_memory_id,to_memory_id,relation_type,created_at,metadata_json) VALUES(?,?,?,?,?,?)",params![id,from,to,kind.to_string(),now(),sanitize_value(&metadata).to_string()])?;Ok(id)})
    }
    pub fn relations(&self, id: &str) -> Result<Vec<Value>> {
        let mut stmt=self.conn.prepare("SELECT * FROM memory_relations WHERE from_memory_id=? OR to_memory_id=? ORDER BY created_at,id")?;
        let values=stmt.query_map(params![id,id],|r|Ok(json!({"id":r.get::<_,String>("id")?,"from_memory_id":r.get::<_,String>("from_memory_id")?,"to_memory_id":r.get::<_,String>("to_memory_id")?,"relation_type":r.get::<_,String>("relation_type")?,"created_at":r.get::<_,String>("created_at")?,"metadata":json_column(r,"metadata_json")?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(values)
    }
    pub fn add_evidence(
        &self,
        mid: &str,
        system: &str,
        source_id: &str,
        revision: Option<&str>,
        kind: RelationType,
        metadata: Value,
    ) -> Result<String> {
        validate_identifier(system, "source system")?;
        validate_identifier(source_id, "source id")?;
        self.atomic(||{self.require_memory(mid)?;let id=Uuid::new_v4().to_string();self.conn.execute("INSERT INTO memory_evidence(id,memory_id,source_system,source_id,source_revision,relation,created_at,metadata_json) VALUES(?,?,?,?,?,?,?,?)",params![id,mid,system,source_id,revision.map(sanitize_revision),kind.to_string(),now(),sanitize_value(&metadata).to_string()])?;Ok(id)})
    }
    pub fn evidence(&self, id: &str) -> Result<Vec<Value>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM memory_evidence WHERE memory_id=? ORDER BY created_at,id")?;
        let values=stmt.query_map([id],|r|Ok(json!({"id":r.get::<_,String>("id")?,"memory_id":r.get::<_,String>("memory_id")?,"source_system":sanitize_text(&r.get::<_,String>("source_system")?),"source_id":sanitize_text(&r.get::<_,String>("source_id")?),"source_revision":r.get::<_,Option<String>>("source_revision")?.map(|v|sanitize_revision(&v)),"relation":r.get::<_,String>("relation")?,"created_at":r.get::<_,String>("created_at")?,"metadata":json_column(r,"metadata_json")?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(values)
    }
    pub fn consolidate_and_log(
        &self,
        from: &str,
        to: &str,
        event: &str,
        action: &str,
    ) -> Result<Vec<String>> {
        validate_identifier(from, "layer")?;
        validate_identifier(to, "layer")?;
        validate_identifier(event, "event")?;
        self.atomic(||{let ids=self.list_memories(Some(from),false)?.into_iter().map(|m|m.id).collect::<Vec<_>>();for id in &ids{self.update_layer(id,to)?;}if !ids.is_empty(){self.conn.execute("INSERT INTO triggers_log(id,event,action,memories_affected,timestamp) VALUES(?,?,?,?,?)",params![Uuid::new_v4().to_string(),event,sanitize_text(action),serde_json::to_string(&ids)?,now()])?;}Ok(ids)})
    }
    pub fn rebuild_tree(&self, tree: &TreeNode) -> Result<()> {
        fn insert(
            conn: &Connection,
            node: &TreeNode,
            parent: Option<&str>,
            depth: usize,
        ) -> Result<()> {
            if depth > 256 {
                bail!("tree exceeds maximum depth")
            }
            validate_identifier(&node.id, "tree id")?;
            if let Some(layer) = &node.layer {
                validate_identifier(layer, "layer")?;
            }
            conn.execute("INSERT INTO tree_nodes(id,parent_id,layer,title,summary,memory_ids,updated_at) VALUES(?,?,?,?,?,?,?)",params![node.id,parent,node.layer.as_deref().unwrap_or(""),sanitize_text(&node.title),sanitize_text(&node.summary),serde_json::to_string(&node.memory_ids)?,now()])?;
            for child in &node.children {
                insert(conn, child, Some(&node.id), depth + 1)?;
            }
            Ok(())
        }
        self.atomic(|| {
            self.conn.execute("DELETE FROM tree_nodes", [])?;
            insert(&self.conn, tree, None, 0)
        })
    }
    pub fn load_tree(&self) -> Result<Option<TreeNode>> {
        let mut statement = self.conn.prepare(
            "SELECT id,parent_id,layer,title,summary,memory_ids FROM tree_nodes ORDER BY rowid",
        )?;
        let rows = statement
            .query_map([], |r| {
                Ok((
                    TreeNode {
                        id: r.get(0)?,
                        layer: r.get::<_, Option<String>>(2)?.filter(|s| !s.is_empty()),
                        title: sanitize_text(&r.get::<_, String>(3)?),
                        summary: sanitize_text(&r.get::<_, Option<String>>(4)?.unwrap_or_default()),
                        memory_ids: r
                            .get::<_, Option<String>>(5)?
                            .filter(|s| !s.is_empty())
                            .map(|s| serde_json::from_str(&s))
                            .transpose()
                            .map_err(|e| sql_error(e.into()))?
                            .unwrap_or_default(),
                        children: vec![],
                    },
                    r.get::<_, Option<String>>(1)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.is_empty() {
            return Ok(None);
        }
        let mut nodes = HashMap::new();
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        let mut roots = vec![];
        for (node, parent) in rows {
            if let Some(parent) = parent.filter(|s| !s.is_empty()) {
                children.entry(parent).or_default().push(node.id.clone());
            } else {
                roots.push(node.id.clone());
            }
            nodes.insert(node.id.clone(), node);
        }
        fn build(
            id: &str,
            nodes: &HashMap<String, TreeNode>,
            children: &HashMap<String, Vec<String>>,
            seen: &mut HashSet<String>,
            depth: usize,
        ) -> Result<TreeNode> {
            if depth > 256 || !seen.insert(id.into()) {
                bail!("invalid stored tree cycle or excessive depth")
            }
            let mut node = nodes
                .get(id)
                .cloned()
                .ok_or_else(|| anyhow!("invalid stored tree parent"))?;
            if let Some(ids) = children.get(id) {
                for child in ids {
                    node.children
                        .push(build(child, nodes, children, seen, depth + 1)?);
                }
            }
            Ok(node)
        }
        let mut seen = HashSet::new();
        let mut trees = vec![];
        for root in roots {
            trees.push(build(&root, &nodes, &children, &mut seen, 0)?);
        }
        if seen.len() != nodes.len() {
            bail!("stored tree has disconnected or cyclic nodes")
        }
        if trees.len() == 1 {
            Ok(trees.pop())
        } else {
            Ok(Some(TreeNode {
                id: "root".into(),
                title: "Memory Tree".into(),
                summary: "Root of all memory layers".into(),
                children: trees,
                ..Default::default()
            }))
        }
    }
    pub fn audit(&self, limit: usize) -> Result<Vec<Value>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM triggers_log ORDER BY timestamp DESC,rowid DESC LIMIT ?")?;
        let values=stmt.query_map([limit as i64],|r|Ok(sanitize_value(&json!({"id":r.get::<_,String>("id")?,"event":r.get::<_,String>("event")?,"action":r.get::<_,String>("action")?,"memories_affected":json_column(r,"memories_affected")?,"timestamp":r.get::<_,String>("timestamp")?}))))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(values)
    }
    pub fn provenance(&self, limit: usize) -> Result<Vec<Value>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM provenance ORDER BY timestamp DESC,rowid DESC LIMIT ?")?;
        let values=stmt.query_map([limit as i64],|r|Ok(sanitize_value(&json!({"id":r.get::<_,String>("id")?,"query":r.get::<_,String>("query")?,"timestamp":r.get::<_,String>("timestamp")?,"method":r.get::<_,String>("method")?,"tree_version":r.get::<_,i64>("tree_version")?,"total_memories_searched":r.get::<_,i64>("total_memories_searched")?,"avg_confidence":r.get::<_,f64>("avg_confidence")?,"sources":json_column(r,"sources_json")?}))))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(values)
    }
    pub fn save_provenance(
        &self,
        query: &str,
        result: &RetrievalResult,
        searched: usize,
    ) -> Result<()> {
        let mut sources = Vec::new();
        for (index, id) in result.memory_ids.iter().enumerate() {
            if let Some(memory) = self.get_memory(id, true)? {
                sources.push(json!({"memory_id":id,"source":result.sources.get(index).unwrap_or(&memory.source),"content":memory.content,"layer":memory.layer,"confidence":memory.confidence,"match_reason":"keyword_tree","trace_path":result.trace}));
            }
        }
        self.conn.execute("INSERT INTO provenance(id,query,timestamp,method,tree_version,total_memories_searched,avg_confidence,sources_json) VALUES(?,?,?,?,?,?,?,?)",params![Uuid::new_v4().to_string(),sanitize_text(query),now(),"keyword_tree",self.revision()?,searched as i64,result.confidence,sanitize_value(&json!(sources)).to_string()])?;
        Ok(())
    }
    pub fn maintenance(&self, key: &str, details: Value) -> Result<()> {
        validate_identifier(key, "maintenance key")?;
        self.conn.execute("INSERT INTO maintenance_state(key,last_run_at,metadata_json) VALUES(?,?,?) ON CONFLICT(key) DO UPDATE SET last_run_at=excluded.last_run_at,metadata_json=excluded.metadata_json",params![key,now(),sanitize_value(&details).to_string()])?;
        Ok(())
    }
    pub fn last_maintenance(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT last_run_at FROM maintenance_state WHERE key=?",
                [key],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
    }
    pub fn revision(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT revision FROM rust_revision WHERE id=1", [], |r| {
                r.get(0)
            })?)
    }
    pub fn stats(&self) -> Result<Value> {
        let count = |table: &str| -> Result<i64> {
            Ok(self
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
        };
        let mut layers = serde_json::Map::new();
        let mut stmt = self
            .conn
            .prepare("SELECT layer,COUNT(*) FROM memories GROUP BY layer")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (layer, count) = row?;
            layers.insert(sanitize_text(&layer), json!(count));
        }
        Ok(
            json!({"memories":count("memories")?,"eligible_memories":self.list_memories(None,false)?.len(),"tree_nodes":count("tree_nodes")?,"triggers":count("triggers_log")?,"provenance_records":count("provenance")?,"otel_spans":count("otel_spans")?,"memory_relations":count("memory_relations")?,"memory_evidence":count("memory_evidence")?,"revision":self.revision()?,"by_layer":layers}),
        )
    }
    pub fn clear(&self) -> Result<usize> {
        self.atomic(|| {
            let count = self
                .conn
                .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get::<_, i64>(0))?;
            for table in [
                "memories",
                "tree_nodes",
                "memory_relations",
                "memory_evidence",
                "triggers_log",
                "provenance",
                "otel_spans",
                "maintenance_state",
                "agent_checkpoints",
                "rust_query_cache",
                "query_cache",
            ] {
                self.conn.execute(&format!("DELETE FROM {table}"), [])?;
            }
            Ok(count as usize)
        })
    }
    pub fn checkpoint(&self) -> Result<()> {
        let busy = self
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                r.get::<_, i64>(0)
            })
            .context("checkpoint WAL")?;
        if busy != 0 {
            bail!("WAL checkpoint blocked by active readers")
        }
        Ok(())
    }
}
