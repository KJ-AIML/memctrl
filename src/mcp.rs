//! MCP stdio server using the official SDK; schemas are descriptive, so dispatch validates inputs.
use crate::{
    model::{ClaimType, LifecycleState, NewMemory, VerificationState},
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
fn layers(args: &Value) -> Result<Vec<String>> {
    match args.get("layers") {
        None => Ok(crate::context::DEFAULT_LAYERS
            .iter()
            .map(|l| l.to_string())
            .collect()),
        Some(v) => {
            let items = v
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("layers must be an array of strings"))?;
            ensure!(!items.is_empty(), "layers must not be empty");
            items
                .iter()
                .map(|item| {
                    let layer = item
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("layers must be an array of strings"))?;
                    validate_identifier(layer, "layer")?;
                    Ok(layer.to_string())
                })
                .collect()
        }
    }
}
fn budget(args: &Value) -> Result<usize> {
    match args.get("budget") {
        None => Ok(crate::context::DEFAULT_BUDGET),
        Some(v) => {
            let n = v
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("budget must be a nonnegative integer"))?;
            ensure!(n <= 100_000, "budget must be at most 100000");
            Ok(n as usize)
        }
    }
}
fn strings(args: &Value, key: &str) -> Result<Vec<String>> {
    match args.get(key) {
        None => Ok(Vec::new()),
        Some(v) => {
            let items = v
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("{key} must be an array of strings"))?;
            ensure!(items.len() <= 64, "{key} accepts at most 64 entries");
            items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(String::from)
                        .ok_or_else(|| anyhow::anyhow!("{key} must be an array of strings"))
                })
                .collect()
        }
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
            let claim_type = match optional_string(args, "claim_type")? {
                Some(text) => text.parse::<ClaimType>()?,
                None => ClaimType::default(),
            };
            let tags = strings(args, "tags")?;
            crate::review::check_tags(&tags)?;
            let specs = strings(args, "cite")?;
            let mut memory = NewMemory {
                content: content.into(),
                layer: layer.into(),
                source: source.into(),
                claim_type,
                tags,
                expires_at: rules.expiry(layer),
                ..Default::default()
            };
            memory.validate()?;
            if let Some(same) = store.existing_for_add(layer, content)? {
                return Ok(sanitize_value(&json!({
                    "id": same.id,
                    "status": "exists",
                    "lifecycle_state": same.lifecycle_state,
                })));
            }
            let verdict = crate::guard::apply(&mut memory, &rules.guard);
            let mut output = if specs.is_empty() {
                json!({"id": store.insert_memory(memory)?, "status": "stored"})
            } else {
                // Resolve and hash every citation before writing anything.
                let root = crate::citations::root_for(rules)?;
                let cwd = std::env::current_dir()?;
                let anchors = specs
                    .iter()
                    .map(|spec| {
                        crate::citations::anchor(&root, &cwd, &crate::citations::parse_spec(spec)?)
                    })
                    .collect::<Result<Vec<_>>>()?;
                let commit = crate::citations::current_commit(&root);
                let id = store.insert_memory_with_citations(memory, &anchors, commit.as_deref())?;
                json!({"id": id, "status": "stored", "citations": anchors.len()})
            };
            if verdict.quarantined() {
                output["status"] = json!("quarantined");
                output["lifecycle_state"] = json!("candidate");
                output["guard"] = json!(verdict);
            }
            let redacted = crate::sanitize::redactions(content);
            if !redacted.is_empty() {
                output["redacted"] = json!(redacted);
            }
            output
        }
        "memctrl_query" => {
            let query = string(args, "query")?;
            let layer = optional_string(args, "layer")?;
            if let Some(l) = layer {
                validate_identifier(l, "layer")?;
            }
            let mut result = crate::retriever::retrieve_with(
                store,
                query,
                layer,
                limit(args)?,
                &crate::retriever::SearchOptions::from_rules(rules),
            )?;
            if let Ok(root) = crate::citations::root_for(rules) {
                crate::citations::annotate(store, &root, &mut result)?;
            }
            serde_json::to_value(result)?
        }
        "memctrl_context" => {
            let layers = layers(args)?;
            let budget = budget(args)?;
            let notes = crate::citations::stale_notes(store, rules);
            serde_json::to_value(crate::context::session_brief(
                store,
                &layers,
                budget,
                &|m| notes.get(&m.id).cloned(),
            )?)?
        }
        "memctrl_stale" => {
            let layer = optional_string(args, "layer")?;
            if let Some(l) = layer {
                validate_identifier(l, "layer")?;
            }
            let root = crate::citations::root_for(rules)?;
            crate::citations::stale_report(store, &root, layer, false)?
        }
        "memctrl_trigger" => {
            let event = string(args, "event")?;
            validate_identifier(event, "event")?;
            let context = args.get("context").cloned().unwrap_or_else(|| json!({}));
            let ids = rules.fire_trigger(event, &context, store)?;
            json!({"status":"fired", "event":event, "affected":ids.len(), "memory_ids":ids})
        }
        "memctrl_tree" => {
            // Reading never writes; rebuilding is explicit and stores only the compact tree.
            let layer = optional_string(args, "layer")?;
            if let Some(l) = layer {
                validate_identifier(l, "layer")?;
            }
            let count = match args.get("limit") {
                None => 20,
                Some(_) => limit(args)?,
            };
            if args.get("rebuild").and_then(Value::as_bool) == Some(true) {
                let memories = store.list_memories(None, false)?;
                store.store_tree(crate::tree::build_tree(&memories))?;
            }
            store.tree_overview(layer, count)?
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
            let action = string(args, "action")?;
            let reason = optional_string(args, "reason")?.unwrap_or("");
            ensure!(
                ["accept", "reject", "dispute", "refute"].contains(&action),
                "unknown review action"
            );
            let id = store.resolve_id(id)?;
            let id = id.as_str();
            let memory = store
                .get_memory(id, true)?
                .ok_or_else(|| anyhow::anyhow!("memory not found"))?;
            match action {
                "accept" => {
                    ensure!(
                        memory.verification_state != VerificationState::Refuted,
                        "cannot accept refuted memory"
                    );
                    // An agent never releases what the guard or an import held for review.
                    return Ok(sanitize_value(&crate::review::accept(
                        store,
                        id,
                        crate::review::Release::Refuse,
                    )?));
                }
                "reject" => {
                    store.update_lifecycle(id, LifecycleState::Rejected)?;
                    if !reason.is_empty() {
                        store.log_event("reject", reason, &[id.to_string()])?;
                    }
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
            json!({"content":{"type":"string"},"layer":{"type":"string"},"source":{"type":"string","default":"mcp"},"claim_type":{"type":"string","enum":["assertion","decision","observation","hypothesis"]},"tags":{"type":"array","items":{"type":"string"}},"cite":{"type":"array","items":{"type":"string"},"description":"Code the memory describes: path, path:LINE or path:START-END, relative to the project root"}}),
            vec!["content", "layer"],
        ),
        (
            "memctrl_stale",
            "List memories whose cited code changed or disappeared since they were saved",
            json!({"layer":{"type":"string"}}),
            vec![],
        ),
        (
            "memctrl_context",
            "Brief of the highest-trust project memories within a token budget; call at session start",
            json!({"layers":{"type":"array","items":{"type":"string"},"default":["project","user"]},"budget":{"type":"integer","minimum":0,"maximum":100000,"default":1500}}),
            vec![],
        ),
        (
            "memctrl_trigger",
            "Fire a configured trigger",
            json!({"event":{"type":"string"},"context":{"type":"object"}}),
            vec!["event"],
        ),
        (
            "memctrl_tree",
            "Overview of the memory tree: layers, clusters, counts and sample ids. Read-only unless rebuild is true",
            json!({"rebuild":{"type":"boolean"},"layer":{"type":"string"},"limit":{"type":"integer"}}),
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
    project_dir: Option<PathBuf>,
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
            // Each request observes the latest rules, including invalid TOML.
            let mut rules = Rules::load(&self.rc)?;
            rules.project_dir = self.project_dir.clone();
            let mut store = self
                .store
                .lock()
                .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
            dispatch(&mut store, &rules, request.name.as_ref(), &args)
        })();
        let result = match result {
            Ok(v) => CallToolResult::success(vec![ContentBlock::text(v.to_string())]),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(
                json!({"error":sanitize_text(&format!("{e:#}"))}).to_string(),
            )]),
        };
        Ok(result.into())
    }
}
/// `rc` should be absolute so the server keeps its project's rules whatever its working directory.
pub async fn serve(store: MemoryStore, rc: PathBuf, project_dir: Option<PathBuf>) -> Result<()> {
    let server = MemoryServer {
        store: Mutex::new(store),
        rc,
        project_dir,
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

#[cfg(test)]
mod context_tests {
    use super::*;
    #[test]
    fn context_tool_validates_before_writing_provenance() -> Result<()> {
        let mut store = MemoryStore::open(":memory:")?;
        let rules = Rules::default();
        store.insert_memory(NewMemory {
            layer: "project".into(),
            content: "Releases are cut from main".into(),
            ..Default::default()
        })?;
        for args in [
            json!({"layers":"project"}),
            json!({"layers":[]}),
            json!({"layers":[7]}),
            json!({"budget":-1}),
            json!({"budget":1_000_000}),
        ] {
            assert!(dispatch(&mut store, &rules, "memctrl_context", &args).is_err());
        }
        assert!(
            store.provenance(10)?.is_empty(),
            "invalid input must not record anything"
        );
        let brief = dispatch(&mut store, &rules, "memctrl_context", &json!({}))?;
        assert_eq!(brief["entries"][0]["content"], "Releases are cut from main");
        assert!(brief["markdown"]
            .as_str()
            .unwrap()
            .contains("## Facts and conventions"));
        Ok(())
    }
}
