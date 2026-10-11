use anyhow::{bail, ensure, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use memctrl::{
    agent, citations, context, decay, doctor, guard, heli, installer,
    llm::{LlmClient, LlmConfig, LlmSettings},
    mcp, mirror,
    model::{ClaimType, LifecycleState, NewMemory, VerificationState},
    reflection, retriever,
    rules::{Rules, DEFAULT_RULES},
    sanitize::{sanitize_text, sanitize_value, validate_identifier},
    store::MemoryStore,
    telemetry, tree,
};
use serde_json::{json, Value};
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Parser)]
#[command(
    name = "memctrl",
    version,
    about = "Rule-governed local memory for AI agents. Results are JSON; external LLM calls require --llm.",
    arg_required_else_help = true
)]
struct Cli {
    #[arg(
        long,
        global = true,
        env = "MEMCTRL_DB_PATH",
        help = "SQLite database (otherwise .memoryrc db_path or .memctrl/memories.db)"
    )]
    db: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        default_value = ".memoryrc",
        help = "TOML rules file; missing files use defaults"
    )]
    rc: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Args, Default)]
struct LlmOptions {
    #[arg(long, help = "Explicitly enable calls to an LLM provider")]
    llm: bool,
    #[arg(
        long,
        help = "openai, xai, anthropic or compatible (default: MEMCTRL_LLM_PROVIDER or openai)"
    )]
    llm_provider: Option<String>,
    #[arg(
        long,
        help = "Model name (default: MEMCTRL_LLM_MODEL; required for xai and anthropic)"
    )]
    llm_model: Option<String>,
    #[arg(long, help = "Base URL, e.g. http://localhost:11434/v1 for Ollama")]
    llm_base_url: Option<String>,
    #[arg(long, help = "Name of the environment variable that holds the API key")]
    llm_api_key_env: Option<String>,
    /// Deprecated: a key in argv is visible in process listings and shell history.
    #[arg(long, hide = true)]
    llm_api_key: Option<String>,
}
impl LlmOptions {
    fn client(&self) -> Result<Option<LlmClient>> {
        if !self.llm {
            return Ok(None);
        }
        if self.llm_api_key.is_some() {
            eprintln!(
                "{}",
                json!({"warning": "--llm-api-key exposes the key to other processes; use --llm-api-key-env or MEMCTRL_LLM_API_KEY"})
            );
        }
        let settings = LlmSettings {
            provider: self.llm_provider.clone(),
            model: self.llm_model.clone(),
            base_url: self.llm_base_url.clone(),
            api_key_env: self.llm_api_key_env.clone(),
            api_key: self.llm_api_key.clone(),
            max_retries: None,
        };
        let config = LlmConfig::resolve(&settings, |name| std::env::var(name).ok())?;
        Ok(Some(LlmClient::from_config(config)?))
    }
}
#[derive(Args)]
struct Listing {
    #[arg(long)]
    layer: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[arg(long, help = "Include expired and inactive lifecycle records")]
    include_history: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ContextFormat {
    /// Markdown for people and AGENTS.md.
    Markdown,
    /// Machine-readable entries plus the Markdown.
    Json,
    /// Claude Code SessionStart hook output (additionalContext).
    Hook,
}
#[derive(Subcommand)]
enum Command {
    /// Create rules and initialize the database.
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Install SKILL.md for a coding tool (and optionally a Claude Code session-start hook).
    Install {
        #[arg(long)]
        tool: Option<String>,
        #[arg(long)]
        project: bool,
        #[arg(
            long,
            help = "Claude Code only: add a SessionStart hook that injects `memctrl context`"
        )]
        hook: bool,
    },
    /// Print a budgeted brief of the highest-trust memories for a session start.
    Context {
        #[arg(long, default_value_t = context::DEFAULT_BUDGET, help = "Approximate token budget")]
        budget: usize,
        #[arg(
            long = "layer",
            help = "Layer to include (repeatable; default: project and user)"
        )]
        layers: Vec<String>,
        #[arg(long, value_enum, default_value_t = ContextFormat::Markdown)]
        format: ContextFormat,
        #[arg(
            long,
            help = "Write the brief between memctrl markers in this file (e.g. AGENTS.md) instead of printing it"
        )]
        write: Option<PathBuf>,
    },
    /// Store a memory (or explicitly request LLM extraction).
    Add {
        content: String,
        #[arg(long, default_value = "session")]
        layer: String,
        #[arg(long, default_value = "manual")]
        source: String,
        #[arg(long, default_value_t = 1.0)]
        confidence: f64,
        #[arg(long, default_value = "assertion")]
        claim_type: String,
        #[arg(long, default_value = "accepted")]
        lifecycle_state: String,
        #[arg(long, default_value = "unverified")]
        verification_state: String,
        #[arg(long)]
        tags: Vec<String>,
        #[arg(long)]
        expires_at: Option<String>,
        #[arg(
            long = "cite",
            value_name = "PATH[:LINE[-LINE]]",
            help = "Code this memory describes (repeatable); memctrl flags the memory when it changes"
        )]
        cite: Vec<String>,
        #[command(flatten)]
        llm: LlmOptions,
    },
    /// Link an existing memory to the code it describes.
    Cite {
        memory_id: String,
        #[arg(required = true, value_name = "PATH[:LINE[-LINE]]")]
        specs: Vec<String>,
    },
    /// List memories whose cited code changed or disappeared.
    Stale {
        #[arg(long)]
        layer: Option<String>,
        #[arg(long, help = "Include memories whose citations are still fresh")]
        all: bool,
    },
    /// Re-anchor a memory's citations to the current code after re-checking the memory.
    Refresh { memory_id: String },
    /// Write shared memory to a Markdown file for review in pull requests.
    Export {
        #[arg(long, default_value = mirror::DEFAULT_PATH)]
        out: PathBuf,
        #[arg(
            long = "layer",
            help = "Layer to export (repeatable; default: project)"
        )]
        layers: Vec<String>,
        #[arg(
            long,
            help = "Fail if the file is not up to date instead of writing it"
        )]
        check: bool,
    },
    /// Import a memctrl export; never overwrites local memories.
    Import {
        file: PathBuf,
        #[arg(
            long,
            default_value = "project",
            help = "Layer for hand-written entries"
        )]
        layer: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Retrieve relevant active memories with provenance.
    Query {
        query_text: String,
        #[arg(long)]
        layer: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[command(flatten)]
        llm: LlmOptions,
    },
    /// List memories.
    List(Listing),
    /// List candidate memories awaiting review.
    Candidates(Listing),
    /// Inspect a memory, relations and evidence.
    Show { memory_id: String },
    /// Accept a candidate into active knowledge.
    Accept { memory_id: String },
    /// Reject a memory.
    Reject {
        memory_id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Show relationship lineage and evidence.
    History { memory_id: String },
    /// Mark a memory as disputed.
    Dispute { memory_id: String },
    /// Refute a memory; refuted claims cannot be accepted.
    Refute {
        memory_id: String,
        #[arg(long, short = 'r')]
        reason: String,
        #[arg(long)]
        refuting_id: Option<String>,
    },
    /// Supersede an old memory with its replacement.
    Supersede { old_id: String, new_id: String },
    /// Build and display the memory tree.
    Tree {
        #[command(flatten)]
        llm: LlmOptions,
    },
    /// Remove a memory by ID.
    Forget { memory_id: String },
    /// Clear memories; requires --yes in this noninteractive JSON CLI.
    Clear {
        #[arg(long)]
        layer: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Run confidence decay or inspect low-confidence records.
    Decay {
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value_t = 0.3)]
        threshold: f64,
        #[arg(long, default_value_t = 0.0)]
        min_hours: f64,
    },
    /// Reinforce a memory's confidence.
    Reinforce { memory_id: String },
    /// Fire a configured trigger.
    #[command(alias = "trigger-cmd")]
    Trigger {
        event: String,
        #[arg(long)]
        context: Option<String>,
    },
    /// Show trigger audit records.
    Audit {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Report store health (always JSON).
    Doctor {
        #[arg(long, default_value_t = 0.5)]
        low_confidence_threshold: f64,
        #[arg(long = "json")]
        json_output: bool,
    },
    /// Show retrieval provenance.
    Provenance {
        query: Option<String>,
        #[arg(long)]
        full: bool,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Show memory access frequencies.
    Heatmap,
    /// Show recent activity.
    Timeline {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Explicit session end: force reflection.
    Done {
        #[command(flatten)]
        llm: LlmOptions,
    },
    /// Reflect when maintenance heuristics permit.
    Reflect {
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        llm: LlmOptions,
    },
    /// Inspect persisted operation spans.
    Spans {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        demo: bool,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Export persisted spans as JSON or OTLP JSON.
    OtelExport {
        #[arg(long, default_value = "memctrl_spans.json")]
        output: PathBuf,
        #[arg(long)]
        otlp: bool,
        #[arg(long, default_value_t = 1000)]
        limit: usize,
    },
    /// Show operation telemetry statistics.
    OtelStats,
    /// Checkpoint SQLite WAL and report store statistics.
    Maintenance,
    /// Show store statistics.
    Stats,
    /// Start the official MCP SDK stdio server.
    Serve,
    /// Review external task history.
    Review {
        #[command(subcommand)]
        command: ReviewCommand,
    },
    /// JSON agent checkpoint and memory-node APIs.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
}
#[derive(Subcommand)]
enum ReviewCommand {
    /// Distill candidate lessons from Heli task history, read-only by default.
    Heli {
        #[arg(long, short = 'w', default_value = ".")]
        workspace: PathBuf,
        #[arg(long, short = 'c', default_value_t = 20)]
        completed: usize,
        #[arg(long, conflicts_with = "dry_run")]
        persist: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        repo: Option<String>,
    },
}
#[derive(Subcommand)]
enum AgentCommand {
    /// Save arbitrary JSON state for a thread.
    Put { thread_id: String, state: String },
    /// Load a thread checkpoint.
    Get { thread_id: String },
    /// List recent checkpoints.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Read memories and extract new ones from JSON agent state.
    Node {
        state: String,
        #[arg(long)]
        persist: bool,
    },
}
fn parse_object(text: &str, label: &str) -> Result<Value> {
    let value: Value =
        serde_json::from_str(text).with_context(|| format!("{label} must be valid JSON"))?;
    ensure!(value.is_object(), "{label} must be a JSON object");
    Ok(value)
}
fn validate_layer(layer: Option<&str>) -> Result<()> {
    if let Some(layer) = layer {
        validate_identifier(layer, "layer")?;
    }
    Ok(())
}
fn find_db(cli: &Cli) -> Result<PathBuf> {
    if let Some(db) = &cli.db {
        return Ok(db.clone());
    }
    if cli.rc.exists() {
        let value: toml::Value = toml::from_str(&std::fs::read_to_string(&cli.rc)?)?;
        if let Some(path) = value.get("memctrl").and_then(|v| v.get("db_path")) {
            let path = PathBuf::from(path.as_str().context("memctrl.db_path must be a string")?);
            return Ok(if path.is_absolute() {
                path
            } else {
                cli.rc.parent().unwrap_or(Path::new(".")).join(path)
            });
        }
    }
    Ok(PathBuf::from(".memctrl/memories.db"))
}
fn selected_memory_ids(
    node: &memctrl::model::TreeNode,
    selected: &[String],
    ancestor_selected: bool,
    output: &mut Vec<String>,
) {
    let include = ancestor_selected || selected.contains(&node.id);
    if include {
        for id in &node.memory_ids {
            if !output.contains(id) {
                output.push(id.clone());
            }
        }
    }
    for child in &node.children {
        selected_memory_ids(child, selected, include, output);
    }
}
fn project_root(rules: &Rules) -> Result<PathBuf> {
    citations::project_root(rules.citations_root.as_deref())
}
fn require_memory(store: &MemoryStore, id: &str) -> Result<()> {
    validate_identifier(id, "memory_id")?;
    ensure!(store.get_memory(id, true)?.is_some(), "memory not found");
    Ok(())
}
async fn execute(cli: &Cli, store: &mut MemoryStore, rules: &Rules) -> Result<Value> {
    let result = match &cli.command {
        Command::Init { force } => {
            ensure!(
                *force || !cli.rc.exists(),
                "rules file already exists; use --force"
            );
            if let Some(parent) = cli.rc.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&cli.rc, DEFAULT_RULES)?;
            let database = find_db(cli)?;
            mirror::ensure_gitignore(&database)?;
            json!({"status":"initialized","rules":cli.rc,"database":database})
        }
        Command::Install {
            tool,
            project,
            hook,
        } => {
            let tool = tool
                .as_deref()
                .context("--tool is required; choose claude, cursor, kimi, codex, axga or pi")?;
            ensure!(
                !*hook || matches!(tool, "claude" | "claude_code"),
                "--hook is only available for Claude Code; for other tools run `memctrl context --write AGENTS.md`"
            );
            let root = if *project {
                std::env::current_dir()?
            } else {
                PathBuf::from(
                    std::env::var_os("HOME")
                        .or_else(|| std::env::var_os("USERPROFILE"))
                        .context("home directory is not configured; use --project")?,
                )
            };
            let path = installer::install(tool, *project, &root)?;
            let mut output = json!({"path": path});
            if let Some(legacy) = installer::legacy_skill_path(tool)
                .map(|relative| root.join(relative))
                .filter(|p| p.exists())
            {
                output["legacy_skill"] = json!(legacy);
                output["note"] =
                    json!("memctrl 2.0 installed the skill where this tool never reads it; you can delete legacy_skill");
            }
            if *hook {
                let executable = std::env::current_exe()?;
                let (settings, changed) =
                    installer::install_hook(&root, &installer::hook_command(&executable))?;
                output["hook"] = json!(settings);
                output["hook_changed"] = json!(changed);
            }
            output
        }
        Command::Context {
            budget,
            layers,
            format,
            write,
        } => {
            let layers: Vec<String> = if layers.is_empty() {
                context::DEFAULT_LAYERS
                    .iter()
                    .map(|l| l.to_string())
                    .collect()
            } else {
                layers.clone()
            };
            let notes = citations::stale_notes(store, rules);
            let brief =
                context::session_brief(store, &layers, *budget, &|m| notes.get(&m.id).cloned())?;
            if let Some(path) = write {
                let existing = match std::fs::read_to_string(path) {
                    Ok(text) => text,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                    Err(error) => return Err(error.into()),
                };
                let updated = memctrl::fsutil::upsert_block(
                    &existing,
                    context::BEGIN_MARKER,
                    context::END_MARKER,
                    &context::marked_block(&brief),
                );
                let written = memctrl::fsutil::write_atomic(path, updated.as_bytes())?;
                json!({"written":written,"shown":brief.entries.len(),"eligible":brief.eligible,"approx_tokens":brief.approx_tokens})
            } else if *format == ContextFormat::Json {
                serde_json::to_value(&brief)?
            } else {
                Value::String(brief.markdown)
            }
        }
        Command::Add {
            content,
            layer,
            source,
            confidence,
            claim_type,
            lifecycle_state,
            verification_state,
            tags,
            expires_at,
            cite,
            llm,
        } => {
            validate_layer(Some(layer))?;
            validate_identifier(source, "source")?;
            ensure!(
                cite.is_empty() || !llm.llm,
                "--cite applies to one memory; add cited memories without --llm"
            );
            // Resolve and hash every citation before writing anything.
            let mut anchors = Vec::new();
            let mut root = None;
            if !cite.is_empty() {
                let base = project_root(rules)?;
                let cwd = std::env::current_dir()?;
                for spec in cite {
                    anchors.push(citations::anchor(
                        &base,
                        &cwd,
                        &citations::parse_spec(spec)?,
                    )?);
                }
                root = Some(base);
            }
            let mut memory = NewMemory {
                content: content.clone(),
                layer: layer.clone(),
                source: source.clone(),
                confidence: *confidence,
                claim_type: claim_type.parse::<ClaimType>()?,
                lifecycle_state: lifecycle_state.parse::<LifecycleState>()?,
                verification_state: verification_state.parse::<VerificationState>()?,
                tags: tags.clone(),
                expires_at: expires_at.clone().or_else(|| rules.expiry(layer)),
                ..Default::default()
            };
            memory.validate()?;
            if let Some(client) = llm.client()? {
                let mut memories = client.extract(content, layer).await?;
                let mut quarantined = 0;
                for m in &mut memories {
                    m.source = source.clone();
                    m.expires_at = rules.expiry(&m.layer);
                    m.validate()?;
                    if guard::apply(m, &rules.guard).quarantined() {
                        quarantined += 1;
                    }
                }
                let ids = store.insert_memories(memories)?;
                json!({"memory_ids":ids,"status":"stored","quarantined":quarantined})
            } else {
                let verdict = guard::apply(&mut memory, &rules.guard);
                let id = match root {
                    Some(root) => {
                        let commit = citations::current_commit(&root);
                        store.insert_memory_with_citations(memory, &anchors, commit.as_deref())?
                    }
                    None => store.insert_memory(memory)?,
                };
                let mut output = json!({"id":id,"status":"stored"});
                if !anchors.is_empty() {
                    output["citations"] = json!(anchors.len());
                }
                if verdict.quarantined() {
                    output["status"] = json!("quarantined");
                    output["lifecycle_state"] = json!("candidate");
                    output["guard"] = json!(verdict);
                    output["note"] = json!("held for review: `memctrl candidates`, then `memctrl accept <id>` or `memctrl reject <id>`");
                }
                output
            }
        }
        Command::Cite { memory_id, specs } => {
            require_memory(store, memory_id)?;
            let root = project_root(rules)?;
            let cwd = std::env::current_dir()?;
            let anchors = specs
                .iter()
                .map(|spec| citations::anchor(&root, &cwd, &citations::parse_spec(spec)?))
                .collect::<Result<Vec<_>>>()?;
            let commit = citations::current_commit(&root);
            let mut added = 0;
            for anchor in &anchors {
                if store.add_citation(memory_id, anchor, commit.as_deref())?.1 {
                    added += 1;
                }
            }
            json!({"id":memory_id,"added":added,"citations":citations::describe(store,&root,memory_id)?})
        }
        Command::Stale { layer, all } => {
            validate_layer(layer.as_deref())?;
            citations::stale_report(store, &project_root(rules)?, layer.as_deref(), *all)?
        }
        Command::Refresh { memory_id } => {
            require_memory(store, memory_id)?;
            citations::refresh(store, &project_root(rules)?, memory_id)?
        }
        Command::Export { out, layers, check } => {
            let layers: Vec<String> = if layers.is_empty() {
                mirror::DEFAULT_LAYERS
                    .iter()
                    .map(|l| l.to_string())
                    .collect()
            } else {
                layers.clone()
            };
            let (text, count) = mirror::render(store, &layers)?;
            let current = std::fs::read_to_string(out).ok();
            let up_to_date = current.as_deref() == Some(text.as_str());
            if *check {
                ensure!(
                    up_to_date,
                    "{} is out of date; run `memctrl export`",
                    out.display()
                );
                json!({"output":out,"memories":count,"up_to_date":true})
            } else {
                if !up_to_date {
                    memctrl::fsutil::write_atomic(out, text.as_bytes())?;
                }
                mirror::ensure_gitignore(out)?;
                json!({"output":out,"memories":count,"changed":!up_to_date})
            }
        }
        Command::Import {
            file,
            layer,
            dry_run,
        } => {
            validate_layer(Some(layer))?;
            let text = std::fs::read_to_string(file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            let mut report = mirror::import(store, &text, layer, &rules.guard, *dry_run)?;
            report["file"] = json!(file);
            report
        }
        Command::Query {
            query_text,
            layer,
            limit,
            llm,
        } => {
            validate_layer(layer.as_deref())?;
            if let Some(client) = llm.client()? {
                let memories = store.list_memories(layer.as_deref(), false)?;
                let root = tree::build_tree(&memories);
                let node_ids = client.select(query_text, &root).await?;
                let mut ids = Vec::new();
                selected_memory_ids(&root, &node_ids, false, &mut ids);
                serde_json::to_value(retriever::publish_selection(
                    store,
                    query_text,
                    layer.as_deref(),
                    &ids,
                    *limit,
                )?)?
            } else {
                let mut result = retriever::retrieve_with(
                    store,
                    query_text,
                    layer.as_deref(),
                    *limit,
                    &retriever::SearchOptions::from_rules(rules),
                )?;
                if let Ok(root) = project_root(rules) {
                    citations::annotate(store, &root, &mut result)?;
                }
                serde_json::to_value(result)?
            }
        }
        Command::List(options) | Command::Candidates(options) => {
            validate_layer(options.layer.as_deref())?;
            let candidate = matches!(&cli.command, Command::Candidates(_));
            let memories: Vec<_> = store
                .list_memories(
                    options.layer.as_deref(),
                    options.include_history || candidate,
                )?
                .into_iter()
                .filter(|m| !candidate || m.lifecycle_state == LifecycleState::Candidate)
                .take(options.limit)
                .collect();
            if candidate {
                json!({"candidates":memories})
            } else {
                json!({"memories":memories})
            }
        }
        Command::Show { memory_id } | Command::History { memory_id } => {
            require_memory(store, memory_id)?;
            let cited = match project_root(rules) {
                Ok(root) => citations::describe(store, &root, memory_id)?,
                Err(_) => json!(store.citations(memory_id)?),
            };
            json!({"memory":store.get_memory(memory_id,true)?,"relations":store.relations(memory_id)?,"evidence":store.evidence(memory_id)?,"citations":cited})
        }
        Command::Accept { memory_id } => mcp::dispatch(
            store,
            rules,
            "memctrl_review",
            &json!({"id":memory_id,"action":"accept"}),
        )?,
        Command::Reject { memory_id, reason } => {
            let mut args = json!({"id":memory_id,"action":"reject"});
            if let Some(reason) = reason {
                args["reason"] = json!(reason);
            }
            mcp::dispatch(store, rules, "memctrl_review", &args)?
        }
        Command::Dispute { memory_id } => mcp::dispatch(
            store,
            rules,
            "memctrl_review",
            &json!({"id":memory_id,"action":"dispute"}),
        )?,
        Command::Refute {
            memory_id,
            reason,
            refuting_id,
        } => {
            require_memory(store, memory_id)?;
            if let Some(id) = refuting_id {
                require_memory(store, id)?;
            }
            store.refute_memory(memory_id, reason, refuting_id.as_deref())?;
            json!({"id":memory_id,"status":"refuted"})
        }
        Command::Supersede { old_id, new_id } => {
            require_memory(store, old_id)?;
            require_memory(store, new_id)?;
            store.supersede_memory(old_id, new_id, json!({}))?;
            json!({"old_id":old_id,"new_id":new_id,"status":"superseded"})
        }
        Command::Tree { llm } => {
            let mut root = tree::build_tree(&store.list_memories(None, false)?);
            if let Some(client) = llm.client()? {
                root = client.summarize_tree(&root).await?;
            }
            store.rebuild_tree(&root)?;
            serde_json::to_value(root)?
        }
        Command::Forget { memory_id } => {
            require_memory(store, memory_id)?;
            ensure!(store.delete_memory(memory_id)?, "memory not found");
            json!({"id":memory_id,"status":"deleted"})
        }
        Command::Clear { layer, yes } => {
            validate_layer(layer.as_deref())?;
            ensure!(*yes, "clear requires --yes");
            let count = match layer {
                Some(layer) => store.clear_layer(layer)?,
                None => store.clear()?,
            };
            json!({"deleted":count})
        }
        Command::Decay {
            dry_run,
            threshold,
            min_hours,
        } => {
            memctrl::model::validate_confidence(*threshold)?;
            ensure!(
                min_hours.is_finite() && *min_hours >= 0.0,
                "min-hours must be finite and nonnegative"
            );
            let changed = if *dry_run {
                0
            } else {
                decay::run_decay(store, *min_hours)?
            };
            let flagged: Vec<_> = store
                .list_memories(None, false)?
                .into_iter()
                .filter(|m| m.confidence < *threshold)
                .collect();
            json!({"dry_run":dry_run,"decayed":changed,"flagged":flagged})
        }
        Command::Reinforce { memory_id } => {
            require_memory(store, memory_id)?;
            json!({"id":memory_id,"reinforced":decay::reinforce(store,memory_id)?})
        }
        Command::Trigger { event, context } => {
            let context = match context {
                Some(c) => parse_object(c, "context")?,
                None => json!({}),
            };
            mcp::dispatch(
                store,
                rules,
                "memctrl_trigger",
                &json!({"event":event,"context":context}),
            )?
        }
        Command::Audit { limit } => json!({"logs":store.audit(*limit)?}),
        Command::Doctor {
            low_confidence_threshold,
            json_output: _,
        } => {
            memctrl::model::validate_confidence(*low_confidence_threshold)?;
            let mut report = doctor::report(store)?;
            report["low_confidence_threshold"] = json!(low_confidence_threshold);
            let weak: Vec<_> = store
                .list_memories(None, false)?
                .into_iter()
                .filter(|m| m.confidence < *low_confidence_threshold)
                .map(|m| m.id)
                .collect();
            report["weak_memory_ids"] = json!(weak);
            report["stale_memories"] = match project_root(rules) {
                Ok(root) => json!(citations::stale_report(store, &root, None, false)?["stale"]
                    .as_array()
                    .map_or(0, Vec::len)),
                Err(_) => Value::Null,
            };
            report
        }
        Command::Provenance {
            query,
            full: _,
            limit,
        } => {
            let records: Vec<_> = store
                .provenance(*limit)?
                .into_iter()
                .filter(|v| {
                    query
                        .as_ref()
                        .is_none_or(|q| v.get("query").and_then(Value::as_str) == Some(q))
                })
                .collect();
            json!({"provenance":records})
        }
        Command::Heatmap => {
            let mut memories = store.list_memories(None, true)?;
            memories.sort_by_key(|m| std::cmp::Reverse(m.access_count));
            json!({"memories":memories.into_iter().map(|m|json!({"id":m.id,"layer":m.layer,"access_count":m.access_count,"last_accessed_at":m.last_accessed_at})).collect::<Vec<_>>()})
        }
        Command::Timeline { limit } => {
            json!({"audit":store.audit(*limit)?,"provenance":store.provenance(*limit)?,"spans":telemetry::spans(store,*limit)?})
        }
        Command::Done { llm } | Command::Reflect { llm, .. } => {
            ensure!(
                !llm.llm,
                "LLM reflection is unavailable; omit --llm for local derived candidates"
            );
            let force = matches!(
                &cli.command,
                Command::Done { .. } | Command::Reflect { force: true, .. }
            );
            json!({"memory_ids":reflection::reflect(store,rules,force)?})
        }
        Command::Spans { name, demo, limit } => {
            if *demo {
                telemetry::record(store, "demo", 0.0, json!({"demo":true}))?;
            }
            let spans: Vec<_> = telemetry::spans(store, *limit)?
                .into_iter()
                .filter(|v| {
                    name.as_ref().is_none_or(|n| {
                        v.get("operation")
                            .or_else(|| v.get("name"))
                            .and_then(Value::as_str)
                            .is_some_and(|s| s.contains(n))
                    })
                })
                .collect();
            json!({"spans":spans})
        }
        Command::OtelExport {
            output,
            otlp,
            limit,
        } => {
            let spans = telemetry::spans(store, *limit)?;
            let value = if *otlp {
                telemetry::otlp_json(&spans)
            } else {
                json!(spans)
            };
            std::fs::write(output, serde_json::to_vec_pretty(&sanitize_value(&value))?)?;
            json!({"output":output,"spans":spans.len(),"otlp":otlp})
        }
        Command::OtelStats => telemetry::stats(store)?,
        Command::Maintenance => {
            store.rebuild_search_index()?;
            store.checkpoint()?;
            json!({"status":"checkpointed","stats":store.stats()?})
        }
        Command::Stats => store.stats()?,
        Command::Review {
            command:
                ReviewCommand::Heli {
                    workspace,
                    completed,
                    persist,
                    dry_run: _,
                    repo,
                },
        } => heli::review_filtered(store, workspace, *persist, *completed, repo.as_deref())?,
        Command::Agent { command } => match command {
            AgentCommand::Put { thread_id, state } => {
                validate_identifier(thread_id, "thread_id")?;
                let state: Value =
                    serde_json::from_str(state).context("state must be valid JSON")?;
                agent::put_checkpoint(store, thread_id, state)?;
                json!({"thread_id":thread_id,"status":"saved"})
            }
            AgentCommand::Get { thread_id } => {
                validate_identifier(thread_id, "thread_id")?;
                json!({"thread_id":thread_id,"state":agent::get_checkpoint(store,thread_id)?})
            }
            AgentCommand::List { limit } => {
                json!({"checkpoints":agent::list_checkpoints(store,*limit)?})
            }
            AgentCommand::Node { state, persist } => {
                let state = parse_object(state, "state")?;
                agent::memory_node(store, state, rules, *persist)?
            }
        },
        Command::Serve => bail!("serve is handled separately"),
    };
    Ok(sanitize_value(&result))
}
async fn run(cli: Cli) -> Result<()> {
    // Init must be able to replace malformed rules explicitly; every other command fails closed.
    let rules = if matches!(&cli.command, Command::Init { .. }) {
        Rules::default()
    } else {
        Rules::load(&cli.rc)?
    };
    let db = if matches!(&cli.command, Command::Init { force: true }) && cli.db.is_none() {
        PathBuf::from(".memctrl/memories.db")
    } else {
        find_db(&cli)?
    };
    // A session-start hook runs in every repository; where memctrl was never set up it must
    // print nothing and create nothing.
    if let Command::Context { format, write, .. } = &cli.command {
        if !db.exists() {
            if write.is_some() {
                bail!(
                    "no memctrl database at {}; run `memctrl init` first",
                    db.display()
                );
            }
            if *format == ContextFormat::Json {
                let empty = json!({"entries":[],"eligible":0,"store":"missing"});
                std::io::stdout()
                    .lock()
                    .write_all((empty.to_string() + "\n").as_bytes())?;
            }
            return Ok(());
        }
    }
    if let Some(parent) = db.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut store = MemoryStore::open(&db)?;
    if matches!(&cli.command, Command::Serve) {
        return mcp::serve(store, cli.rc).await;
    }
    let started = Instant::now();
    let result = execute(&cli, &mut store, &rules).await?;
    let operation = command_name(&cli.command);
    if !matches!(
        &cli.command,
        Command::Init { .. }
            | Command::Install { .. }
            | Command::Spans { .. }
            | Command::OtelStats
            | Command::OtelExport { .. }
            | Command::Agent {
                command: AgentCommand::Node { persist: false, .. }
            }
            | Command::Review {
                command: ReviewCommand::Heli { persist: false, .. }
            }
            | Command::Decay { dry_run: true, .. }
    ) {
        telemetry::record(
            &mut store,
            operation,
            started.elapsed().as_secs_f64() * 1000.0,
            json!({"interface":"cli"}),
        )?;
    }
    let encoded = match (&cli.command, &result) {
        (Command::Context { format, .. }, Value::String(markdown)) => match format {
            ContextFormat::Hook => {
                json!({"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":markdown}})
                    .to_string()
                    + "\n"
            }
            _ => markdown.clone(),
        },
        _ => serde_json::to_string(&sanitize_value(&result))? + "\n",
    };
    std::io::stdout().lock().write_all(encoded.as_bytes())?;
    Ok(())
}
fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Init { .. } => "init",
        Command::Install { .. } => "install",
        Command::Context { .. } => "context",
        Command::Cite { .. } => "cite",
        Command::Stale { .. } => "stale",
        Command::Refresh { .. } => "refresh",
        Command::Export { .. } => "export",
        Command::Import { .. } => "import",
        Command::Add { .. } => "store",
        Command::Query { .. } => "retrieve",
        Command::List(_) => "list",
        Command::Candidates(_) => "candidates",
        Command::Show { .. } => "show",
        Command::History { .. } => "history",
        Command::Accept { .. } => "accept",
        Command::Reject { .. } => "reject",
        Command::Dispute { .. } => "dispute",
        Command::Refute { .. } => "refute",
        Command::Supersede { .. } => "supersede",
        Command::Tree { .. } => "tree",
        Command::Forget { .. } => "forget",
        Command::Clear { .. } => "clear",
        Command::Decay { .. } => "decay",
        Command::Reinforce { .. } => "reinforce",
        Command::Trigger { .. } => "trigger",
        Command::Audit { .. } => "audit",
        Command::Doctor { .. } => "doctor",
        Command::Provenance { .. } => "provenance",
        Command::Heatmap => "heatmap",
        Command::Timeline { .. } => "timeline",
        Command::Done { .. } | Command::Reflect { .. } => "reflect",
        Command::Spans { .. } => "spans",
        Command::OtelExport { .. } => "otel-export",
        Command::OtelStats => "otel-stats",
        Command::Maintenance => "maintenance",
        Command::Stats => "stats",
        Command::Serve => "serve",
        Command::Review { .. } => "review",
        Command::Agent { .. } => "agent",
    }
}
#[tokio::main]
async fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let rendered = sanitize_text(&error.to_string());
            if error.use_stderr() {
                eprint!("{rendered}");
            } else {
                let _ = std::io::stdout().lock().write_all(rendered.as_bytes());
            }
            std::process::exit(code);
        }
    };
    if let Err(error) = run(cli).await {
        eprintln!("{}", json!({"error":sanitize_text(&error.to_string())}));
        std::process::exit(1);
    }
}
