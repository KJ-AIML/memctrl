//! MCP stdio server using the official SDK; schemas are descriptive, so dispatch validates inputs.
use crate::{
    model::{LifecycleState, NewMemory, VerificationState},
    rules::Rules,
    sanitize::{sanitize_text, sanitize_value, validate_identifier},
    store::MemoryStore,
};
use anyhow::{bail, ensure, Result};
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
    ErrorData, RoleServer, ServerHandler, ServiceExt,
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Mutex};

fn string<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("{key} must be a string"))
}
fn optional_string<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match args.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_str()
            .map(Some)
            .ok_or_else(|| anyhow::anyhow!("{key} must be a string")),
    }
}
fn limit(args: &Value) -> Result<usize> {
    match args.get("limit") {
        None => Ok(50),
        Some(v) => {
            let n = v
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("limit must be a nonnegative integer"))?;
            ensure!(n <= 10000, "limit must be at most 10000");
            Ok(n as usize)
        }
    }
}
/// Validate all inputs before doing anything that can change memories, access counts, caches or audit.
pub fn dispatch(store: &mut MemoryStore, rules: &Rules, name: &str, args: &Value) -> Result<Value> {
    ensure!(args.is_object(), "arguments must be an object");
    for key in ["source", "layer", "reason"] {
        optional_string(args, key)?;
    }
    if let Some(v) = args.get("context") {
        ensure!(v.is_object(), "context must be an object");
    }
    let output = match name {
        "memctrl_add" => {
            let content = string(args, "content")?;
            let layer = string(args, "layer")?;
            let source = optional_string(args, "source")?.unwrap_or("mcp");
            validate_identifier(layer, "layer")?;
            validate_identifier(source, "source")?;
            let memory = NewMemory {
                content: content.into(),
                layer: layer.into(),
                source: source.into(),
                expires_at: rules.expiry(layer),
                ..Default::default()
            };
            memory.validate()?;
            json!({"id": store.insert_memory(memory)?, "status": "stored"})
        }
        "memctrl_query" => {
            let query = string(args, "query")?;
            let layer = optional_string(args, "layer")?;
            if let Some(l) = layer {
                validate_identifier(l, "layer")?;
            }
            serde_json::to_value(crate::retriever::retrieve(
                store,
                query,
                layer,
                limit(args)?,
            )?)?
        }
        "memctrl_trigger" => {
            let event = string(args, "event")?;
            validate_identifier(event, "event")?;
            let context = args.get("context").cloned().unwrap_or_else(|| json!({}));
            let ids = rules.fire_trigger(event, &context, store)?;
            json!({"status":"fired", "event":event, "affected":ids.len(), "memory_ids":ids})
        }
        "memctrl_tree" => {
            let memories = store.list_memories(None, false)?;
            let tree = crate::tree::build_tree(&memories);
            store.rebuild_tree(&tree)?;
            serde_json::to_value(tree)?
        }
        "memctrl_audit" => json!({"logs":store.audit(limit(args)?)?}),
        "memctrl_candidates" => {
            let layer = optional_string(args, "layer")?;
            if let Some(l) = layer {
                validate_identifier(l, "layer")?;
            }
            let count = limit(args)?;
            let candidates: Vec<_> = store
                .list_memories(layer, true)?
                .into_iter()
                .filter(|m| m.lifecycle_state == LifecycleState::Candidate)
                .take(count)
                .collect();
            json!({"candidates": candidates})
        }
        "memctrl_review" => {
            let id = string(args, "id")?;
            validate_identifier(id, "id")?;
            let action = string(args, "action")?;
            let reason = optional_string(args, "reason")?.unwrap_or("");
            ensure!(
                ["accept", "reject", "dispute", "refute"].contains(&action),
                "unknown review action"
            );
            let memory = store
                .get_memory(id, true)?
                .ok_or_else(|| anyhow::anyhow!("memory not found"))?;
            match action {
                "accept" => {
                    ensure!(
                        memory.verification_state != VerificationState::Refuted,
                        "cannot accept refuted memory"
                    );
                    store.update_lifecycle(id, LifecycleState::Accepted)?;
                }
                "reject" => {
                    store.update_lifecycle(id, LifecycleState::Rejected)?;
                }
                "dispute" => {
                    store.update_verification(id, VerificationState::Disputed)?;
                }
                "refute" => {
                    store.refute_memory(id, reason, None)?;
                }
                _ => unreachable!(),
            }
            json!({"id":id,"action":action,"status":"updated"})
        }
        _ => bail!("unknown tool"),
    };
    Ok(sanitize_value(&output))
}
fn tools() -> Vec<Tool> {
    let entries = [
        (
            "memctrl_query",
            "Retrieve relevant memories with reasoning trace",
            json!({"query":{"type":"string"},"layer":{"type":"string"},"limit":{"type":"integer","minimum":0,"maximum":10000}}),
            vec!["query"],
        ),
        (
            "memctrl_add",
            "Store a memory",
            json!({"content":{"type":"string"},"layer":{"type":"string"},"source":{"type":"string","default":"mcp"}}),
            vec!["content", "layer"],
        ),
        (
            "memctrl_trigger",
            "Fire a configured trigger",
            json!({"event":{"type":"string"},"context":{"type":"object"}}),
            vec!["event"],
        ),
        (
            "memctrl_tree",
            "Get the full memory tree",
            json!({}),
            vec![],
        ),
        (
            "memctrl_audit",
            "Get trigger audit logs",
            json!({"limit":{"type":"integer","minimum":0,"maximum":10000}}),
            vec![],
        ),
        (
            "memctrl_candidates",
            "List memories awaiting review",
            json!({"layer":{"type":"string"},"limit":{"type":"integer","minimum":0,"maximum":10000}}),
            vec![],
        ),
        (
            "memctrl_review",
            "Accept, reject, dispute or refute a memory",
            json!({"id":{"type":"string"},"action":{"type":"string","enum":["accept","reject","dispute","refute"]},"reason":{"type":"string"}}),
            vec!["id", "action"],
        ),
    ];
    entries
        .into_iter()
        .map(|(name, description, properties, required)| {
            let schema = json!({"type":"object","properties":properties,"required":required});
            Tool::new(
                name,
                description,
                schema.as_object().cloned().unwrap_or_default(),
            )
        })
        .collect()
}
struct MemoryServer {
    store: Mutex<MemoryStore>,
    rc: PathBuf,
}
impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("memctrl", env!("CARGO_PKG_VERSION")))
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools()))
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        let args = Value::Object(request.arguments.unwrap_or_default());
        let result = (|| -> Result<Value> {
            let rules = Rules::load(&self.rc)?; // Each request observes the latest rules, including invalid TOML.
            let mut store = self
                .store
                .lock()
                .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
            dispatch(&mut store, &rules, request.name.as_ref(), &args)
        })();
        let result = match result {
            Ok(v) => CallToolResult::success(vec![ContentBlock::text(v.to_string())]),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(
                json!({"error":sanitize_text(&e.to_string())}).to_string(),
            )]),
        };
        Ok(result.into())
    }
}
pub async fn serve(store: MemoryStore, rc: PathBuf) -> Result<()> {
    let server = MemoryServer {
        store: Mutex::new(store),
        rc,
    };
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_optional_values_never_mutate() -> Result<()> {
        let mut store = MemoryStore::open(":memory:")?;
        let rules = Rules::default();
        let id = store.insert_memory(NewMemory {
            content: "a verified local fact".into(),
            ..Default::default()
        })?;
        for args in [
            json!({"content":"new fact","layer":"session","source":null}),
            json!({"content":"new fact","layer":"session","source":34}),
            json!({"content":"new fact","layer":"session","context":[]}),
        ] {
            let revision = store.revision()?;
            assert!(dispatch(&mut store, &rules, "memctrl_add", &args).is_err());
            assert_eq!(revision, store.revision()?);
        }
        for args in [
            json!({"id":id,"action":"reject","reason":null}),
            json!({"id":id,"action":"reject","reason":{}}),
            json!({"id":id,"action":"reject","layer":false}),
        ] {
            let revision = store.revision()?;
            assert!(dispatch(&mut store, &rules, "memctrl_review", &args).is_err());
            assert_eq!(revision, store.revision()?);
        }
        let revision = store.revision()?;
        assert!(dispatch(
            &mut store,
            &rules,
            "memctrl_trigger",
            &json!({"event":"on_commit","context":"bad"})
        )
        .is_err());
        assert_eq!(revision, store.revision()?);
        assert_eq!(
            store.get_memory(&id, true)?.unwrap().lifecycle_state,
            LifecycleState::Accepted
        );
        Ok(())
    }
}
