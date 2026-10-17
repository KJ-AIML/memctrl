use anyhow::{bail, ensure, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use memctrl::{
    agent, citations, context, decay, doctor, guard, heli, installer,
    llm::{LlmClient, LlmConfig, LlmSettings},
    mcp, mirror,
    model::{ClaimType, LifecycleState, NewMemory, VerificationState},
    project::{self, Project},
    recall, reflection, retriever,
    review::{self, Release},
    rules::{Rules, UserConfig, DEFAULT_RULES},
    sanitize::{sanitize_text, sanitize_value, validate_identifier},
    semantic,
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
        help = "SQLite database (otherwise db_path in .memoryrc, else .memctrl/memories.db in the project folder)"
    )]
    db: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        help = "TOML rules file (default: the nearest .memoryrc in this folder or a parent, up to the repository root)"
    )]
    rc: Option<PathBuf>,
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
    /// Claude Code hook output (additionalContext), reading the hook's JSON from stdin.
    Hook,
}
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum EvalRetriever {
    Lexical,
    Legacy,
    Both,
    /// Lexical fused with matches by meaning; needs [semantic] in your user config.
    Semantic,
    /// What the prompt hook would add (semantic too when [semantic] is on).
    Recall,
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
    /// The memories that match a prompt, minus what this session already saw (for a prompt hook).
    Recall {
        #[arg(help = "Prompt text (with --format hook: read from the hook's JSON on stdin)")]
        prompt: Option<String>,
        #[arg(long, default_value_t = recall::DEFAULT_LIMIT)]
        limit: usize,
        #[arg(
            long = "layer",
            help = "Layer to include (repeatable; default: project and user)"
        )]
        layers: Vec<String>,
        #[arg(long, value_enum, default_value_t = ContextFormat::Markdown)]
        format: ContextFormat,
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
    Refresh {
        memory_id: String,
        #[arg(
            long = "cite",
            value_name = "PATH:START-END",
            help = "Set the range for a path the memory cites, when its code can't be followed automatically (repeatable)"
        )]
        cite: Vec<String>,
    },
    /// Write shared memory to a Markdown file for review in pull requests.
    Export {
        #[arg(
            long,
            help = "Output file (default: .memctrl/memory.md in the project folder)"
        )]
        out: Option<PathBuf>,
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
        #[arg(
            long,
            help = "Write from your store alone, dropping entries it hasn't imported and retraction records it doesn't know"
        )]
        force: bool,
    },
    /// After pulling: import memory.md (new entries as candidates, retractions applied), then
    /// export your store merged with it.
    Sync {
        #[arg(help = "Export to sync (default: .memctrl/memory.md in the project folder)")]
        file: Option<PathBuf>,
        #[arg(long = "layer", help = "Layer to sync (repeatable; default: project)")]
        layers: Vec<String>,
        #[arg(
            long,
            help = "You have read the file's changes: accept new entries (asks in a terminal)"
        )]
        accept: bool,
    },
    /// Validate memory.md without a database (for CI): format, ids, hashes, layout, and with
    /// --base that no retraction was dropped.
    Check {
        #[arg(help = "Export to check (default: .memctrl/memory.md in the project folder)")]
        file: Option<PathBuf>,
        #[arg(
            long,
            value_name = "GIT_REF",
            help = "Compare with this commit's version of the file (e.g. origin/main)"
        )]
        base: Option<String>,
        #[arg(
            long,
            help = "Treat warnings (edited or hand-written entries, layout) as errors"
        )]
        strict: bool,
    },
    /// Measure retrieval quality on a suite (built in, or --file) without touching your database.
    Eval {
        #[arg(
            long,
            help = "JSON suite: {name, memories:[{key,content,...}], cases:[{query,expect,kind}]}"
        )]
        file: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = EvalRetriever::Both)]
        retriever: EvalRetriever,
        #[arg(long, default_value_t = 5)]
        top_k: usize,
        #[arg(
            long,
            help = "Fail unless lexical hit@k on answerable cases reaches this rate (0-1)"
        )]
        min_hit_rate: Option<f64>,
    },
    /// Import a memctrl export; never overwrites local memories. New entries arrive as
    /// candidates unless you pass --accept after reading the file's changes.
    Import {
        file: PathBuf,
        #[arg(
            long = "layer",
            help = "Layer to import (repeatable; default: project). Hand-written entries go to the first"
        )]
        layers: Vec<String>,
        #[arg(
            long,
            help = "You have read the file's changes: accept new entries instead of holding them as candidates"
        )]
        accept: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(
            long,
            help = "Keep `supported` verification from the file (by default imported claims arrive unverified)"
        )]
        trust_verification: bool,
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
    /// Show the memory tree (layers, clusters, counts); --rebuild builds it again.
    Tree {
        #[arg(long, help = "Build the tree again from the current memories")]
        rebuild: bool,
        #[arg(long)]
        layer: Option<String>,
        #[arg(long, default_value_t = 20, help = "Memory ids listed per cluster")]
        limit: usize,
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
    Maintenance {
        #[arg(
            long,
            help = "Also rewrite the database to reclaim free space (takes the write lock briefly)"
        )]
        vacuum: bool,
    },
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
    /// Optional semantic search, set up under [semantic] in your user config.
    Semantic {
        #[command(subcommand)]
        command: SemanticCommand,
    },
}
#[derive(Subcommand)]
enum SemanticCommand {
    /// Settings, whether the model answers, and how many memories have vectors.
    Status,
    /// Embed every memory without a vector (searches embed a few on the spot).
    Index {
        #[arg(long)]
        layer: Option<String>,
    },
    /// Measure similarity thresholds for your model on an eval suite (built in, or --file).
    Tune {
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long, default_value_t = 5)]
        top_k: usize,
    },
    /// Delete every stored vector; they're rebuilt as you search while [semantic] is on.
    Clear,
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
/// The project this command runs in: the nearest `.memoryrc` at or above the working directory
/// (see `project::locate`). `--rc` names the rules file instead, with the working directory as
/// the project. `init` always sets up the working directory, and refuses inside another project.
fn project(cli: &Cli) -> Result<Project> {
    let cwd = std::env::current_dir()?;
    if let Some(rc) = &cli.rc {
        let rc = cwd.join(rc);
        return Ok(Project {
            found: rc.is_file(),
            rc,
            dir: cwd.clone(),
            searched_to: cwd,
        });
    }
    let located = project::locate_here()?;
    if let Command::Init { force } = &cli.command {
        ensure!(
            *force || !located.found || located.dir == cwd,
            "{} is already a memctrl project, and commands work from any folder inside it; to start a separate project here, run `memctrl init --force`",
            located.dir.display()
        );
        let rc = cwd.join(project::RULES_FILE);
        return Ok(Project {
            found: rc.is_file(),
            rc,
            dir: cwd.clone(),
            searched_to: cwd,
        });
    }
    Ok(located)
}
/// The database: `--db` (or `MEMCTRL_DB_PATH`), else `db_path` from the rules file (relative to
/// that file), else the project's `.memctrl/memories.db`. True when the user named the path.
fn database(cli: &Cli, rules: &Rules, project: &Project) -> (PathBuf, bool) {
    if let Some(db) = &cli.db {
        return (db.clone(), true);
    }
    if let Some(path) = &rules.db_path {
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            project.rc.parent().unwrap_or(&project.dir).join(path)
        };
        return (path, true);
    }
    (project.default_db(), false)
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
    citations::root_for(rules)
}
/// Relative output paths must resolve inside the project, so a committed symlink can't redirect
/// a write. Absolute paths are the caller's explicit choice.
fn confinement(path: &Path, rules: &Rules) -> Result<Option<PathBuf>> {
    Ok(if path.is_relative() {
        Some(project_root(rules)?)
    } else {
        None
    })
}
/// True when a person can answer a prompt: stdin and stderr are both a terminal. An agent's
/// shell tool, a pipe or a script has no terminal.
fn at_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}
fn import_options(
    layers: &[String],
    accept: bool,
    dry_run: bool,
    trust_verification: bool,
) -> mirror::ImportOptions {
    mirror::ImportOptions {
        layers: if layers.is_empty() {
            mirror::DEFAULT_LAYERS.map(String::from).to_vec()
        } else {
            layers.to_vec()
        },
        dry_run,
        trust_verification,
        accept,
    }
}

/// Imports an export. With `accept`, which is the person's review step, it plans without holding
/// the lock while asking at the terminal, then imports only what was confirmed.
fn run_import(
    store: &mut MemoryStore,
    rules: &Rules,
    file: &Path,
    options: mirror::ImportOptions,
) -> Result<Value> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let mut report = if options.accept && !options.dry_run {
        let mut accepting = Vec::new();
        let preview = mirror::ImportOptions {
            dry_run: true,
            ..options.clone()
        };
        mirror::import_with(store, &text, &preview, &rules.guard, &mut |entries| {
            accepting = entries.to_vec();
            Ok(true)
        })?;
        if !accepting.is_empty() {
            ensure!(
                at_terminal(),
                "import --accept would accept {} {}, which only a person can do: run it yourself in a terminal. Without --accept they arrive as candidates for review",
                accepting.len(),
                if accepting.len() == 1 { "entry" } else { "entries" }
            );
            ensure!(
                review::confirm_import(
                    &shown(file),
                    &accepting,
                    &mut std::io::stdin().lock(),
                    &mut std::io::stderr()
                )?,
                "import cancelled; nothing was changed"
            );
        }
        mirror::import_with(store, &text, &options, &rules.guard, &mut |entries| {
            ensure!(
                entries.iter().all(|e| accepting.contains(e)),
                "the store or the file changed while you were confirming; run the import again"
            );
            Ok(true)
        })?
    } else {
        mirror::import(store, &text, &options, &rules.guard)?
    };
    report["file"] = json!(file);
    Ok(report)
}

/// Writes (or with `check`, compares) the export. An existing file is merged: entries the store
/// hasn't decided on are carried over and retraction records kept. `force` renders from the store
/// alone, dropping them.
fn run_export(
    store: &MemoryStore,
    rules: &Rules,
    project: &Project,
    out: Option<&Path>,
    layers: &[String],
    check: bool,
    force: bool,
) -> Result<Value> {
    // The default path is anchored to the project folder, and confined like a relative one.
    let (out, confine) = match out {
        Some(path) => (path.to_path_buf(), confinement(path, rules)?),
        None => (
            project.dir.join(mirror::DEFAULT_PATH),
            Some(project_root(rules)?),
        ),
    };
    let out = &out;
    let layers: Vec<String> = if layers.is_empty() {
        mirror::DEFAULT_LAYERS.map(String::from).to_vec()
    } else {
        layers.to_vec()
    };
    // Refuse a symlink that escapes the project before reading through it.
    let target = memctrl::fsutil::confined_target(out, confine.as_deref())?;
    // Compare as text: a CRLF checkout or a BOM is not a content change.
    let current = std::fs::read_to_string(&target)
        .ok()
        .map(|t| mirror::normalize(&t));
    let merge_with = current.as_deref().filter(|_| !force);
    let (text, count) = mirror::render_merged(store, &layers, merge_with).with_context(|| {
        format!(
            "{} exists but can't be read as a memctrl export; fix it, or pass --force to overwrite it",
            shown(out)
        )
    })?;
    let carried = match merge_with {
        Some(existing) => mirror::unmerged(store, existing)?.len(),
        None => 0,
    };
    let up_to_date = current.as_deref() == Some(text.as_str());
    let mut output = if check {
        ensure!(
            up_to_date,
            "{} is out of date; run `memctrl sync` (or `memctrl import`, then `memctrl export`)",
            shown(out)
        );
        json!({"output":out,"memories":count,"up_to_date":true})
    } else {
        if !up_to_date {
            memctrl::fsutil::write_atomic(out, text.as_bytes(), confine.as_deref())?;
        }
        mirror::ensure_gitignore(out)?;
        json!({"output":out,"memories":count,"changed":!up_to_date})
    };
    if carried > 0 {
        output["carried"] = json!(carried);
        output["note"] = json!(format!(
            "kept {carried} entries your store hasn't imported; review them with `memctrl import {}`",
            shown(out)
        ));
    }
    Ok(output)
}
/// A path as the user would type it: relative to the working directory when it is inside it.
fn shown(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(|p| p.display().to_string()))
        .unwrap_or_else(|| path.display().to_string())
}
/// The full id of an existing memory, from the id or a unique prefix (the brief shows 8 characters).
fn require_memory(store: &MemoryStore, id: &str) -> Result<String> {
    store.resolve_id(id)
}
/// Installs the skill (and hook) without opening a store: installing creates no database.
fn install(
    tool: Option<&str>,
    project_scope: bool,
    hook: bool,
    project: &Project,
) -> Result<Value> {
    let tool =
        tool.context("--tool is required; choose claude, cursor, kimi, codex, axga or pi")?;
    ensure!(
        !hook || matches!(tool, "claude" | "claude_code"),
        "--hook is only available for Claude Code; for other tools run `memctrl context --write AGENTS.md`"
    );
    let root = if project_scope {
        project.dir.clone()
    } else {
        PathBuf::from(
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .context("home directory is not configured; use --project")?,
        )
    };
    let path = installer::install(tool, project_scope, &root)?;
    let mut output = json!({"path": path});
    if let Some(legacy) = installer::legacy_skill_path(tool)
        .map(|relative| root.join(relative))
        .filter(|p| p.exists())
    {
        output["legacy_skill"] = json!(legacy);
        output["note"] =
            json!("memctrl 2.0 installed the skill where this tool never reads it; you can delete legacy_skill");
    }
    if hook {
        let executable = std::env::current_exe()?;
        let (settings, changed) = installer::install_hook(
            &root,
            installer::settings_file(project_scope),
            &installer::hook_command(&executable),
        )?;
        let (_, recall_changed) = installer::install_recall_hook(
            &root,
            installer::settings_file(project_scope),
            &installer::recall_hook_command(&executable),
        )?;
        output["hook"] = json!(settings);
        output["hook_changed"] = json!(changed || recall_changed);
    }
    Ok(output)
}

/// `memctrl recall`, kept out of `execute` so its locals don't enlarge that function's frame
/// (Windows gives the main thread 1 MB of stack).
fn recall_command(
    store: &MemoryStore,
    rules: &Rules,
    db: &Path,
    prompt: Option<&str>,
    limit: usize,
    layers: &[String],
    format: ContextFormat,
) -> Result<Value> {
    let layers: Vec<String> = if layers.is_empty() {
        context::DEFAULT_LAYERS
            .iter()
            .map(|l| l.to_string())
            .collect()
    } else {
        for layer in layers {
            validate_identifier(layer, "layer")?;
        }
        layers.to_vec()
    };
    let input = if format == ContextFormat::Hook {
        hook_input(std::time::Duration::from_secs(2))
    } else {
        Value::Null
    };
    let text = prompt
        .map(String::from)
        .or_else(|| {
            input
                .get("prompt")
                .and_then(Value::as_str)
                .map(String::from)
        })
        .unwrap_or_default();
    let record = session_record(&input, db);
    let seen = record.as_deref().map(recall::seen_in).unwrap_or_default();
    let root = project_root(rules).ok();
    let stale = |ids: &[String]| -> std::collections::HashMap<String, String> {
        let Some(root) = root.as_deref() else {
            return Default::default();
        };
        citations::check_memories(store, root, ids)
            .map(|checked| {
                checked
                    .into_iter()
                    .filter_map(|(id, list)| citations::stale_note(&list).map(|note| (id, note)))
                    .collect()
            })
            .unwrap_or_default()
    };
    let found = recall::recall(
        store,
        &text,
        &layers,
        limit,
        &retriever::SearchOptions::from_rules(rules),
        &rules.guard,
        &stale,
        &seen,
    )?;
    if let Some(record) = record.filter(|_| !found.entries.is_empty()) {
        let ids: Vec<String> = found.entries.iter().map(|e| e.id.clone()).collect();
        if let Err(error) = recall::remember_recalled(&record, &ids) {
            eprintln!(
                "{}",
                json!({"warning": sanitize_text(&format!("cannot record recalled memories: {error:#}"))})
            );
        }
    }
    Ok(if format == ContextFormat::Json {
        serde_json::to_value(&found)?
    } else {
        Value::String(found.markdown)
    })
}

/// Notes which memories the brief put in this session's context, so the prompt hook knows.
fn record_brief(brief: &context::Brief, db: &Path) {
    let input = hook_input(std::time::Duration::from_millis(500));
    if let Some(record) = session_record(&input, db) {
        let ids: Vec<String> = brief.entries.iter().map(|e| e.id.clone()).collect();
        if let Err(error) = recall::remember_brief(&record, &ids) {
            eprintln!(
                "{}",
                json!({"warning": sanitize_text(&format!("cannot record the session brief: {error:#}"))})
            );
        }
    }
}

/// The JSON Claude Code passes a hook on stdin. A hook command run by hand has a terminal, and
/// a caller that never closes stdin must not hang a session, so reading gives up after `wait`.
fn hook_input(wait: std::time::Duration) -> Value {
    use std::io::{IsTerminal, Read};
    if std::io::stdin().is_terminal() {
        return Value::Null;
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = std::io::stdin()
            .lock()
            .take(4 << 20)
            .read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    receiver
        .recv_timeout(wait)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

/// This session's record of memories already in its context, from the hook's `session_id`.
fn session_record(input: &Value, db: &Path) -> Option<PathBuf> {
    let dir = db.parent().filter(|p| !p.as_os_str().is_empty())?;
    recall::session_file(dir, input.get("session_id")?.as_str()?)
}
async fn execute(
    cli: &Cli,
    store: &mut MemoryStore,
    rules: &Rules,
    project: &Project,
    db: &Path,
) -> Result<Value> {
    let result = match &cli.command {
        Command::Init { force } => {
            // An existing .memoryrc (often committed by a teammate) is kept unless --force.
            let written = *force || !project.rc.exists();
            if written {
                if let Some(parent) = project.rc.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&project.rc, DEFAULT_RULES)?;
            }
            mirror::ensure_gitignore(db)?;
            json!({"status":"initialized","rules":project.rc,"rules_written":written,"database":db})
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
            let brief = context::session_brief_with(
                store,
                &layers,
                *budget,
                &|m| notes.get(&m.id).cloned(),
                &rules.guard,
            )?;
            if *format == ContextFormat::Hook && write.is_none() {
                record_brief(&brief, db);
            }
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
                )
                .with_context(|| format!("cannot update {}", path.display()))?;
                let confine = confinement(path, rules)?;
                let written =
                    memctrl::fsutil::write_atomic(path, updated.as_bytes(), confine.as_deref())?;
                json!({"written":written,"shown":brief.entries.len(),"eligible":brief.eligible,"approx_tokens":brief.approx_tokens})
            } else if *format == ContextFormat::Json {
                serde_json::to_value(&brief)?
            } else {
                Value::String(brief.markdown)
            }
        }
        Command::Recall {
            prompt,
            limit,
            layers,
            format,
        } => recall_command(store, rules, db, prompt.as_deref(), *limit, layers, *format)?,
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
            review::check_tags(tags)?;
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
            } else if let Some(same) = store.existing_for_add(layer, content)? {
                // Adding the same text again returns the memory that holds it.
                let mut output =
                    json!({"id":same.id,"status":"exists","lifecycle_state":same.lifecycle_state});
                if !anchors.is_empty() {
                    output["note"] =
                        json!("not changed; add citations with `memctrl cite <id> <path>`");
                }
                output
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
                // Say what was removed, so nobody assumes the stored text is what they typed.
                let redacted = memctrl::sanitize::redactions(content);
                if !redacted.is_empty() {
                    output["redacted"] = json!(redacted);
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
            let memory_id = &require_memory(store, memory_id)?;
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
        Command::Refresh { memory_id, cite } => {
            let memory_id = require_memory(store, memory_id)?;
            let overrides = cite
                .iter()
                .map(|spec| citations::parse_spec(spec))
                .collect::<Result<Vec<_>>>()?;
            citations::refresh(store, &project_root(rules)?, &memory_id, &overrides)?
        }
        Command::Export {
            out,
            layers,
            check,
            force,
        } => run_export(
            store,
            rules,
            project,
            out.as_deref(),
            layers,
            *check,
            *force,
        )?,
        Command::Import {
            file,
            layers,
            accept,
            dry_run,
            trust_verification,
        } => run_import(
            store,
            rules,
            file,
            import_options(layers, *accept, *dry_run, *trust_verification),
        )?,
        Command::Sync {
            file,
            layers,
            accept,
        } => {
            // Import first, so the export that follows keeps what teammates wrote.
            let path = file
                .clone()
                .unwrap_or_else(|| project.dir.join(mirror::DEFAULT_PATH));
            let imported = if path.exists() {
                run_import(
                    store,
                    rules,
                    &path,
                    import_options(layers, *accept, false, false),
                )?
            } else {
                Value::Null
            };
            let exported = run_export(store, rules, project, Some(&path), layers, false, false)?;
            json!({"import": imported, "export": exported})
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
            let memories = store.recent_memories(
                options.layer.as_deref(),
                options.include_history || candidate,
                candidate,
                options.limit,
            )?;
            if candidate {
                json!({"candidates":memories})
            } else {
                json!({"memories":memories})
            }
        }
        Command::Show { memory_id } | Command::History { memory_id } => {
            let memory_id = &require_memory(store, memory_id)?;
            let cited = match project_root(rules) {
                Ok(root) => citations::describe(store, &root, memory_id)?,
                Err(_) => json!(store.citations(memory_id)?),
            };
            json!({"memory":store.get_memory(memory_id,true)?,"relations":store.relations(memory_id)?,"evidence":store.evidence(memory_id)?,"citations":cited})
        }
        Command::Accept { memory_id } => {
            let id = require_memory(store, memory_id)?;
            let memory = store.get_memory(&id, true)?.context("memory not found")?;
            // Held by the guard or an import, or left out of the brief because the guard flags
            // it now: only a person at a terminal releases it.
            let mut reasons = review::held_for_review(&memory);
            reasons.extend(review::withheld(&memory, &rules.guard));
            let release = if reasons.is_empty() {
                Release::Refuse
            } else {
                ensure!(
                    at_terminal(),
                    "{} is held for review; only a person can release it: run `memctrl accept {}` yourself in a terminal",
                    &id[..8.min(id.len())],
                    &id[..8.min(id.len())]
                );
                ensure!(
                    review::confirm_because(
                        &memory,
                        &reasons,
                        &mut std::io::stdin().lock(),
                        &mut std::io::stderr()
                    )?,
                    "cancelled; nothing changed"
                );
                Release::Confirmed
            };
            review::accept_with(store, &id, release, &rules.guard)?
        }
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
            let memory_id = &require_memory(store, memory_id)?;
            let refuting_id = match refuting_id {
                Some(id) => Some(require_memory(store, id)?),
                None => None,
            };
            store.refute_memory(memory_id, reason, refuting_id.as_deref())?;
            json!({"id":memory_id,"status":"refuted"})
        }
        Command::Supersede { old_id, new_id } => {
            let old_id = &require_memory(store, old_id)?;
            let new_id = &require_memory(store, new_id)?;
            store.supersede_memory(old_id, new_id, json!({}))?;
            json!({"old_id":old_id,"new_id":new_id,"status":"superseded"})
        }
        Command::Tree {
            rebuild,
            layer,
            limit,
            llm,
        } => {
            validate_layer(layer.as_deref())?;
            if *rebuild || llm.llm {
                let mut root = tree::build_tree(&store.list_memories(None, false)?);
                if let Some(client) = llm.client()? {
                    root = client.summarize_tree(&root).await?;
                }
                store.store_tree(root)?;
            }
            store.tree_overview(layer.as_deref(), *limit)?
        }
        Command::Forget { memory_id } => {
            let memory_id = &require_memory(store, memory_id)?;
            ensure!(store.forget_memory(memory_id)?, "memory not found");
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
            let memory_id = &require_memory(store, memory_id)?;
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
            let stray = project::stray_stores(&project.dir, db);
            if !stray.is_empty() {
                report["stray_stores_note"] = json!("stores an earlier memctrl created in subfolders; this project no longer reads them. To keep their memories: memctrl --db <stray> export --out stray.md --layer project --layer user, then memctrl import stray.md");
            }
            report["stray_stores"] = json!(stray);
            report["config_warnings"] = json!(rules.warnings);
            let withheld: Vec<String> = store
                .list_memories(None, false)?
                .iter()
                .filter(|m| !review::withheld(m, &rules.guard).is_empty())
                .map(|m| m.id.chars().take(8).collect())
                .collect();
            if !withheld.is_empty() {
                report["withheld_note"] = json!("accepted memories the guard flags now, left out of the brief: read each with `memctrl show <id>`, then `memctrl accept <id>` in a terminal to release it, or `memctrl forget <id>`");
            }
            report["withheld_from_brief"] = json!(withheld);
            report["oversized_memories"] = json!(store.count_oversized()?);
            report["reclaimable_bytes"] = json!(store.reclaimable_bytes()?);
            report["committed_database"] = json!(project::tracked_by_git(db));
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
        Command::Maintenance { vacuum } => {
            store.rebuild_search_index()?;
            store.checkpoint()?;
            let mut report = json!({"status":"checkpointed","stats":store.stats()?});
            if *vacuum {
                report["reclaimed_bytes"] = json!(store.vacuum()?);
            }
            report
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
        } => heli::review_with_policy(
            store,
            workspace,
            *persist,
            *completed,
            repo.as_deref(),
            &rules.guard,
        )?,
        Command::Semantic { command } => match command {
            SemanticCommand::Status => {
                let user = UserConfig::load()?;
                let config = user.path.as_ref().map(|p| p.display().to_string());
                let stored = semantic::stored(store)?;
                match (&user.semantic, &rules.semantic) {
                    (None, _) => {
                        let reason = match &user.semantic_error {
                            Some(error) => format!("semantic search is off: {error}"),
                            None => "no [semantic] section in your user config".into(),
                        };
                        json!({"enabled": false, "config": config, "stored_vectors": stored,
                               "reason": reason})
                    }
                    (Some(settings), None) => {
                        let reason = rules
                            .warnings
                            .iter()
                            .find(|w| w.starts_with("semantic search is off"))
                            .cloned()
                            .unwrap_or_else(|| "turned off by MEMCTRL_SEMANTIC=off".into());
                        json!({"enabled": false, "config": config, "settings": settings.summary(),
                               "stored_vectors": stored, "reason": reason})
                    }
                    (Some(settings), Some(engine)) => {
                        let started = Instant::now();
                        let ping = engine.ping();
                        let ms = started.elapsed().as_millis() as u64;
                        let (indexed, eligible) = engine.coverage(store, None)?;
                        let mut out = json!({"enabled": true, "config": config,
                            "settings": settings.summary(), "indexed": indexed,
                            "eligible": eligible, "stored_vectors": stored});
                        match ping {
                            Ok(dimensions) => {
                                out["reachable"] = json!(true);
                                out["dimensions"] = json!(dimensions);
                                out["ping_ms"] = json!(ms);
                            }
                            Err(error) => {
                                out["reachable"] = json!(false);
                                out["error"] = json!(format!("{error:#}"));
                            }
                        }
                        if indexed < eligible {
                            out["hint"] = json!(format!("{} memories have no vector yet; run `memctrl semantic index` (searches embed up to {} at a time)", eligible - indexed, semantic::LAZY_LIMIT));
                        }
                        out
                    }
                }
            }
            SemanticCommand::Index { layer } => {
                if let Some(layer) = layer {
                    validate_identifier(layer, "layer")?;
                }
                let engine = rules.semantic.as_ref().with_context(|| {
                    rules
                        .warnings
                        .iter()
                        .find(|w| w.starts_with("semantic search is off"))
                        .cloned()
                        .unwrap_or_else(|| {
                            "semantic search is off: add [semantic] to your user config".into()
                        })
                })?;
                engine.index(store, layer.as_deref())?
            }
            SemanticCommand::Clear => json!({"cleared": semantic::clear(store)?}),
            SemanticCommand::Tune { .. } => bail!("tune is handled separately"),
        },
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
        Command::Eval { .. } => bail!("eval is handled separately"),
        Command::Install { .. } => bail!("install is handled separately"),
        Command::Check { .. } => bail!("check is handled separately"),
    };
    Ok(sanitize_value(&result))
}
async fn run(cli: Cli) -> Result<()> {
    let project = project(&cli)?;
    let user = UserConfig::load()?;
    // Init must be able to replace malformed rules explicitly; every other command fails closed.
    // Init without --force keeps an existing rules file, so its db_path still applies.
    let mut rules = match &cli.command {
        Command::Init { force } if *force || !project.rc.exists() => Rules::default(),
        _ => Rules::load_with(&project.rc, &user)?,
    };
    for warning in &rules.warnings {
        // `semantic` commands report why it's off themselves; tune is how you fix it.
        if matches!(&cli.command, Command::Semantic { .. })
            && warning.starts_with("semantic search is off")
        {
            continue;
        }
        eprintln!("{}", json!({"warning": sanitize_text(warning)}));
    }
    rules.project_dir = Some(project.dir.clone());
    let (db, named) = database(&cli, &rules, &project);
    // Evaluation runs on an in-memory store and never opens or creates the real database.
    if let Command::Eval {
        file,
        retriever,
        top_k,
        min_hit_rate,
    } = &cli.command
    {
        let text = match file {
            Some(path) => std::fs::read_to_string(path)
                .with_context(|| format!("cannot read {}", path.display()))?,
            None => memctrl::eval::BUILTIN_SUITE.to_string(),
        };
        let suite = memctrl::eval::parse_suite(&text)?;
        let retrievers = match retriever {
            EvalRetriever::Lexical => vec![memctrl::eval::Retriever::Lexical],
            EvalRetriever::Legacy => vec![memctrl::eval::Retriever::Legacy],
            EvalRetriever::Both => vec![
                memctrl::eval::Retriever::Lexical,
                memctrl::eval::Retriever::Legacy,
            ],
            EvalRetriever::Semantic => vec![memctrl::eval::Retriever::Semantic],
            EvalRetriever::Recall => vec![memctrl::eval::Retriever::Recall],
        };
        let options = retriever::SearchOptions::from_rules(&rules);
        let report = memctrl::eval::run(&suite, &retrievers, *top_k, &options)?;
        if let Some(floor) = min_hit_rate {
            memctrl::model::validate_confidence(*floor)?;
            let measured = match retriever {
                EvalRetriever::Semantic => "semantic",
                EvalRetriever::Recall => "recall",
                _ => "lexical",
            };
            let rate = report["results"][measured]["hit_at_k"]
                .as_f64()
                .context("--min-hit-rate needs answerable cases")?;
            ensure!(
                rate >= *floor,
                "{measured} hit@{top_k} is {rate}, below --min-hit-rate {floor}"
            );
        }
        let encoded = serde_json::to_string(&sanitize_value(&report))? + "\n";
        std::io::stdout().lock().write_all(encoded.as_bytes())?;
        return Ok(());
    }
    // Tuning embeds a suite, never your memories, so it needs no store.
    if let Command::Semantic {
        command: SemanticCommand::Tune { file, top_k },
    } = &cli.command
    {
        let settings = user.semantic.as_ref().with_context(|| {
            user.semantic_error.clone().unwrap_or_else(|| {
                format!(
                    "add [semantic] with a model to {} first",
                    user.path
                        .as_ref()
                        .map_or("your user config".into(), |p| p.display().to_string())
                )
            })
        })?;
        ensure!(*top_k > 0, "top-k must be at least 1");
        let text = match file {
            Some(path) => std::fs::read_to_string(path)
                .with_context(|| format!("cannot read {}", path.display()))?,
            None => memctrl::eval::BUILTIN_SUITE.to_string(),
        };
        let suite = memctrl::eval::parse_suite(&text)?;
        let report = semantic::tune(settings, &suite, *top_k)?;
        let encoded = serde_json::to_string(&sanitize_value(&report))? + "\n";
        std::io::stdout().lock().write_all(encoded.as_bytes())?;
        return Ok(());
    }
    if let Command::Install {
        tool,
        project: project_scope,
        hook,
    } = &cli.command
    {
        let output = install(tool.as_deref(), *project_scope, *hook, &project)?;
        let encoded = serde_json::to_string(&sanitize_value(&output))? + "\n";
        std::io::stdout().lock().write_all(encoded.as_bytes())?;
        return Ok(());
    }
    // Check reads only the file (and git), never a store: it runs in CI checkouts.
    if let Command::Check { file, base, strict } = &cli.command {
        let path = file
            .clone()
            .unwrap_or_else(|| project.dir.join(mirror::DEFAULT_PATH));
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let mut warnings = Vec::new();
        let base_text = match base {
            Some(rev) => {
                let dir = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                let name = path.file_name().context("check needs a file")?;
                let shown_at = std::process::Command::new("git")
                    .arg("-C")
                    .arg(dir)
                    .arg("show")
                    .arg(format!("{rev}:./{}", name.to_string_lossy()))
                    .output()
                    .context("check --base needs git")?;
                if shown_at.status.success() {
                    Some(String::from_utf8_lossy(&shown_at.stdout).into_owned())
                } else {
                    warnings.push(format!("no {} at {rev}; nothing to compare", shown(&path)));
                    None
                }
            }
            None => None,
        };
        let mut report = mirror::check(&text, base_text.as_deref())?;
        report.warnings.extend(warnings);
        let failing = !report.errors.is_empty() || (*strict && !report.warnings.is_empty());
        ensure!(
            !failing,
            "{} failed the check: {}",
            shown(&path),
            report
                .errors
                .iter()
                .chain(report.warnings.iter().filter(|_| *strict))
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        );
        let encoded = serde_json::to_string(&sanitize_value(&json!(report)))? + "\n";
        std::io::stdout().lock().write_all(encoded.as_bytes())?;
        return Ok(());
    }
    // A session-start hook runs in every repository; where memctrl was never set up it must
    // print nothing and create nothing.
    if let Command::Recall { format, .. } = &cli.command {
        if !db.exists() {
            if *format == ContextFormat::Json {
                std::io::stdout()
                    .lock()
                    .write_all(b"{\"entries\":[],\"store\":\"missing\"}\n")?;
            }
            return Ok(());
        }
    }
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
    // CI checkouts have no store (the database stays out of git).
    if matches!(&cli.command, Command::Export { check: true, .. }) && !db.exists() {
        bail!(
            "export --check compares the file with your local store, and there is none here. To validate memory.md without a store (in CI), run `memctrl check` (add `--base origin/main` to catch dropped retractions)"
        );
    }
    // Only init, or a path the user named, creates a store, so a subfolder never gets its own.
    if !db.exists() && !named && !matches!(&cli.command, Command::Init { .. }) {
        if project.found {
            bail!(
                "{} has memctrl rules but no store yet; run `memctrl init` there (it keeps your rules)",
                project.dir.display()
            );
        }
        bail!(
            "no memctrl project in {} or its parent folders (searched up to {}); run `memctrl init` at the project root, or pass --db",
            std::env::current_dir()?.display(),
            project.searched_to.display()
        );
    }
    // A database committed to git is never loaded into a session or searched for an agent.
    if db.exists()
        && !matches!(
            &cli.command,
            Command::Init { .. } | Command::Doctor { .. } | Command::Stats
        )
        && project::tracked_by_git(&db)
        && !user.trusts(&db)
    {
        let message = format!(
            "{} is committed to git, so anyone with push access could have written its memories; memctrl won't load it. Run `git rm --cached {}` and share memory through memory.md (export/import), or list this repository under [trust] repos in {}",
            shown(&db),
            shown(&db),
            user.path.as_deref().map_or("your user config".into(), |p| p.display().to_string())
        );
        if matches!(
            &cli.command,
            Command::Context { write: None, .. } | Command::Recall { .. }
        ) {
            // The session-start and prompt hooks stay silent rather than failing every session.
            eprintln!("{}", json!({"warning": sanitize_text(&message)}));
            return Ok(());
        }
        bail!(message);
    }
    if let Some(parent) = db.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut store = MemoryStore::open(&db)?;
    if matches!(&cli.command, Command::Serve) {
        // A client starts the server in a folder of its choosing, so with --rc the project is
        // where the rules file is.
        let dir = match &cli.rc {
            Some(_) => project.rc.parent().map(Path::to_path_buf),
            None => Some(project.dir.clone()),
        };
        return mcp::serve(store, project.rc.clone(), dir).await;
    }
    let started = Instant::now();
    // Boxed: the future of a function this large would otherwise sit on the main thread's stack.
    let result = Box::pin(execute(&cli, &mut store, &rules, &project, &db)).await?;
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
            | Command::Import { dry_run: true, .. }
    ) {
        // Best effort: a command whose result is ready never fails or waits on telemetry.
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        store.best_effort(|s| telemetry::record(s, operation, elapsed, json!({"interface":"cli"})));
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
        (Command::Recall { format, .. }, Value::String(markdown)) => match format {
            // Nothing matched: the prompt goes on unchanged.
            _ if markdown.is_empty() => String::new(),
            ContextFormat::Hook => {
                json!({"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":markdown}})
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
        Command::Recall { .. } => "recall",
        Command::Cite { .. } => "cite",
        Command::Stale { .. } => "stale",
        Command::Refresh { .. } => "refresh",
        Command::Export { .. } => "export",
        Command::Sync { .. } => "sync",
        Command::Check { .. } => "check",
        Command::Eval { .. } => "eval",
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
        Command::Maintenance { .. } => "maintenance",
        Command::Stats => "stats",
        Command::Serve => "serve",
        Command::Review { .. } => "review",
        Command::Agent { .. } => "agent",
        Command::Semantic { .. } => "semantic",
    }
}
fn main() {
    // Debug builds of the argument parser alone use close to 1 MB of stack, all that Windows
    // gives the main thread, so the command runs on a thread with room to spare.
    let worker = std::thread::Builder::new()
        .name("memctrl".into())
        .stack_size(16 << 20)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("start the async runtime")
                .block_on(cli_main())
        })
        .expect("start the command thread");
    if worker.join().is_err() {
        std::process::exit(101);
    }
}
async fn cli_main() {
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
    if let Err(error) = Box::pin(run(cli)).await {
        // The whole chain ("entry at line 6: memory id must be a UUID: …"), still sanitized.
        eprintln!("{}", json!({"error":sanitize_text(&format!("{error:#}"))}));
        std::process::exit(1);
    }
}
