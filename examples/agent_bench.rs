//! Agent benchmark: does Claude Code do real tasks right more often with memory?
//!
//! Each task in `bench/agent/tasks.json` runs as a fresh, isolated `claude -p` session in its
//! own copy of a starting point (none, CLAUDE.md, memctrl, ...), then a blind judge grades the
//! result. See `bench/README.md` and `docs/benchmarks/agent-benchmark.md`.
//!
//! ```text
//! cargo run --release --example agent_bench -- setup WORK --memctrl target/release/memctrl
//! cargo run --release --example agent_bench -- run WORK 1 --jobs 4
//! cargo run --release --example agent_bench -- grade WORK
//! cargo run --release --example agent_bench -- summary WORK
//! ```
use anyhow::{bail, ensure, Context, Result};
use clap::{Parser, Subcommand};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Instant;

const HERE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/bench/agent");
const SUFFIX: &str = "\n\nYou're working on your own and can't ask me questions. \
If you decide not to do something as asked, say why in your final message.";
const SHELL: [&str; 19] = [
    "ls",
    "cat",
    "grep",
    "rg",
    "find",
    "head",
    "tail",
    "wc",
    "sed -n",
    "tree",
    "node",
    "git log",
    "git diff",
    "git show",
    "git status",
    "git blame",
    "git grep",
    "mkdir",
    "memctrl",
];
const BASE_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
const GIT_ID: [&str; 6] = [
    "-c",
    "user.name=bench",
    "-c",
    "user.email=bench@example.invalid",
    "-c",
    "commit.gpgsign=false",
];

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Step,
}

#[derive(Subcommand)]
enum Step {
    /// Build the starting points in WORK/templates.
    Setup {
        work: PathBuf,
        /// The memctrl build under test.
        #[arg(long)]
        memctrl: PathBuf,
        /// An older release, set up the way that release installs itself.
        #[arg(long)]
        baseline: Option<PathBuf>,
        /// Also build claude-md-scale and memctrl-scale: plus 1,000 unrelated memories.
        #[arg(long)]
        scale: bool,
        /// Also build memctrl-semantic (and memctrl-semantic-scale with --scale): the memctrl
        /// setups with this user config, whose [semantic] points at a running embedding server.
        #[arg(long)]
        semantic: Option<PathBuf>,
    },
    /// Run every task in every starting point once, as round REP.
    Run {
        work: PathBuf,
        rep: String,
        /// Comma-separated starting points (default: every template).
        #[arg(long, default_value = "")]
        conds: String,
        /// Only these cells, as cond/task (comma-separated).
        #[arg(long, default_value = "")]
        only: String,
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        #[arg(long, default_value = "sonnet")]
        model: String,
        #[arg(long)]
        claude: Option<PathBuf>,
    },
    /// Grade runs that have no grade yet: rule checks plus a blind judge.
    Grade {
        work: PathBuf,
        #[arg(long, default_value_t = 6)]
        jobs: usize,
        #[arg(long)]
        no_judge: bool,
        #[arg(long)]
        claude: Option<PathBuf>,
    },
    /// Pass rates and costs per starting point.
    Summary {
        work: PathBuf,
        /// Rounds to include, comma-separated (default: all).
        #[arg(long, default_value = "")]
        reps: String,
    },
    /// What the prompt hook adds for each task and for unrelated prompts. Run it inside a copy
    /// of a memctrl template: queries in the template itself change what later runs see.
    Probe {
        memctrl: PathBuf,
        /// User config folder (XDG_CONFIG_HOME), e.g. WORK/xdg-memctrl-semantic.
        #[arg(long)]
        config_home: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Step::Setup {
            work,
            memctrl,
            baseline,
            scale,
            semantic,
        } => setup(
            &work,
            &memctrl,
            baseline.as_deref(),
            scale,
            semantic.as_deref(),
        ),
        Step::Run {
            work,
            rep,
            conds,
            only,
            jobs,
            model,
            claude,
        } => run_all(
            &work,
            &rep,
            &conds,
            &only,
            jobs,
            &model,
            &claude_path(claude),
        ),
        Step::Grade {
            work,
            jobs,
            no_judge,
            claude,
        } => grade_all(&work, jobs, !no_judge, &claude_path(claude)),
        Step::Summary { work, reps } => summary(&work, &reps),
        Step::Probe {
            memctrl,
            config_home,
        } => probe(&memctrl, config_home.as_deref()),
    }
}

fn claude_path(given: Option<PathBuf>) -> PathBuf {
    given.unwrap_or_else(|| home().join(".local/bin/claude"))
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn tasks() -> Result<Vec<Value>> {
    let text = std::fs::read_to_string(Path::new(HERE).join("tasks.json"))?;
    Ok(serde_json::from_str(&text)?)
}

/// Runs a command with a minimal environment and returns its stdout, or fails with its output.
fn sh(program: impl AsRef<std::ffi::OsStr>, args: &[&str], cwd: &Path) -> Result<String> {
    sh_with(program, args, cwd, None)
}

/// `sh` with `config_home` as the user config folder (XDG_CONFIG_HOME), so a setup's
/// `[semantic]` applies.
fn sh_with(
    program: impl AsRef<std::ffi::OsStr>,
    args: &[&str],
    cwd: &Path,
    config_home: Option<&Path>,
) -> Result<String> {
    let program = program.as_ref();
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", home());
    if let Some(dir) = config_home {
        command.env("XDG_CONFIG_HOME", dir);
    }
    let out = command
        .output()
        .with_context(|| format!("cannot start {program:?}"))?;
    ensure!(
        out.status.success(),
        "{program:?} {args:?} failed in {}:\n{}{}",
        cwd.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git(args: &[&str], cwd: &Path) -> Result<String> {
    let all: Vec<&str> = GIT_ID.iter().copied().chain(args.iter().copied()).collect();
    sh("git", &all, cwd)
}

// ---------------------------------------------------------------------------------------------
// setup

fn clone(work: &Path, name: &str) -> Result<PathBuf> {
    let dst = work.join("templates").join(name);
    ensure!(
        !dst.exists(),
        "{} exists; use a new WORK folder",
        dst.display()
    );
    let bundle = Path::new(HERE).join("fixture/ledger-api.bundle");
    sh(
        "git",
        &[
            "clone",
            "-q",
            &bundle.to_string_lossy(),
            &dst.to_string_lossy(),
        ],
        work,
    )?;
    sh("git", &["remote", "remove", "origin"], &dst)?;
    Ok(dst)
}

/// A template whose store holds the fixture memories, set up by `binary`'s own install.
fn with_memctrl(work: &Path, name: &str, binary: &Path) -> Result<PathBuf> {
    let bindir = work.join(format!("bin-{name}"));
    std::fs::create_dir_all(&bindir)?;
    std::fs::copy(binary, bindir.join("memctrl"))?;
    let repo = clone(work, name)?;
    let fixture = Path::new(HERE).join("fixture");
    std::fs::create_dir_all(repo.join(".memctrl"))?;
    std::fs::copy(
        fixture.join("memories.sqlite3"),
        repo.join(".memctrl/memories.db"),
    )?;
    std::fs::write(repo.join(".memctrl/.gitignore"), "*\n")?;
    std::fs::copy(fixture.join("memoryrc"), repo.join(".memoryrc"))?;
    sh(
        bindir.join("memctrl"),
        &["install", "--tool", "claude", "--project", "--hook"],
        &repo,
    )?;
    std::fs::write(
        repo.join(".git/info/exclude"),
        ".claude/settings.local.json\n",
    )?;
    git(&["add", "-A"], &repo)?;
    git(&["commit", "-qm", "chore: memctrl setup"], &repo)?;
    Ok(repo)
}

/// Which memories go in a CLAUDE.md section.
type Pick = fn(&Value) -> bool;

/// Every memory as a bullet, grouped the way a person would keep CLAUDE.md.
fn claude_md(memories: &[Value]) -> Result<String> {
    let s = |m: &Value, k: &str| m[k].as_str().unwrap_or_default().to_string();
    let groups: [(&str, Pick); 6] = [
        ("Decisions", |m| {
            m["layer"] == "project" && m["claim_type"] == "decision"
        }),
        ("Facts and conventions", |m| {
            m["layer"] == "project"
                && (m["claim_type"] == "assertion" || m["claim_type"] == "observation")
                && m["verification_state"] != "disputed"
        }),
        ("Open hypotheses", |m| {
            m["layer"] == "project" && m["claim_type"] == "hypothesis"
        }),
        ("Disputed", |m| {
            m["layer"] == "project" && m["verification_state"] == "disputed"
        }),
        ("Preferences", |m| m["layer"] == "user"),
        ("Current work", |m| m["layer"] == "session"),
    ];
    let mut lines = vec!["# ledger-api project notes".to_string(), String::new()];
    let mut used = HashSet::new();
    for (title, pick) in groups {
        let mut rows: Vec<&Value> = memories
            .iter()
            .filter(|m| pick(m) && !used.contains(&s(m, "id")))
            .collect();
        if rows.is_empty() {
            continue;
        }
        rows.sort_by_key(|m| s(m, "created_at"));
        lines.push(format!("## {title}"));
        lines.push(String::new());
        for m in rows {
            used.insert(s(m, "id"));
            lines.push(format!("- {}", s(m, "content")));
        }
        lines.push(String::new());
    }
    ensure!(
        used.len() == memories.len(),
        "some memories fit no CLAUDE.md section"
    );
    Ok(lines.join("\n"))
}

fn setup(
    work: &Path,
    memctrl: &Path,
    baseline: Option<&Path>,
    scale: bool,
    semantic: Option<&Path>,
) -> Result<()> {
    std::fs::create_dir_all(work.join("templates"))?;
    let work = work.canonicalize()?;
    let memctrl = memctrl.canonicalize()?;
    let m = with_memctrl(&work, "memctrl", &memctrl)?;
    let listed = sh(
        work.join("bin-memctrl/memctrl"),
        &["list", "--limit", "5000"],
        &m,
    )?;
    let memories: Vec<Value> = serde_json::from_str::<Value>(&listed)?["memories"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if let Some(baseline) = baseline {
        with_memctrl(&work, "baseline", &baseline.canonicalize()?)?;
    }

    let notes = claude_md(&memories)?;
    let c = clone(&work, "claude-md")?;
    std::fs::write(c.join("CLAUDE.md"), &notes)?;
    git(&["add", "-A"], &c)?;
    git(&["commit", "-qm", "docs: project notes"], &c)?;

    let n = clone(&work, "none")?;
    git(&["commit", "-q", "--allow-empty", "-m", "chore: setup"], &n)?;

    if scale {
        let text = std::fs::read_to_string(Path::new(HERE).join("fixture/distractors.jsonl"))?;
        let rows: Vec<Value> = text
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
        // CLAUDE.md: one section per component, the ledger-api notes kept whole among them.
        let mut sections: BTreeMap<String, String> = BTreeMap::new();
        for row in &rows {
            let component = row["component"].as_str().unwrap_or_default();
            let section = sections
                .entry(component.to_string())
                .or_insert_with(|| format!("## {component}\n\n"));
            section.push_str(&format!(
                "- {}\n",
                row["content"].as_str().unwrap_or_default()
            ));
        }
        sections.insert(
            "ledger-api".into(),
            notes
                .replace("# ledger-api project notes", "## ledger-api")
                .replace("\n## ", "\n### "),
        );
        let cs = clone(&work, "claude-md-scale")?;
        let body: Vec<&str> = sections.values().map(String::as_str).collect();
        std::fs::write(
            cs.join("CLAUDE.md"),
            format!("# Monorepo notes\n\n{}", body.join("\n")),
        )?;
        git(&["add", "-A"], &cs)?;
        git(&["commit", "-qm", "docs: monorepo notes"], &cs)?;
        let ms = with_memctrl(&work, "memctrl-scale", &memctrl)?;
        for row in &rows {
            let out = sh(
                work.join("bin-memctrl-scale/memctrl"),
                &[
                    "add",
                    row["content"].as_str().unwrap_or_default(),
                    "--layer",
                    row["layer"].as_str().unwrap_or("project"),
                    "--claim-type",
                    row["claim_type"].as_str().unwrap_or("assertion"),
                ],
                &ms,
            )?;
            ensure!(!out.contains("\"quarantined\""), "held for review: {out}");
        }
    }
    if let Some(config) = semantic {
        let text = std::fs::read_to_string(config)
            .with_context(|| format!("cannot read {}", config.display()))?;
        let bases: &[&str] = if scale {
            &["memctrl", "memctrl-scale"]
        } else {
            &["memctrl"]
        };
        for base in bases {
            // The same store as the plain setup, so the two compare memory for memory, plus
            // the user config and every memory's vector.
            let name = base.replacen("memctrl", "memctrl-semantic", 1);
            let repo = work.join("templates").join(&name);
            copy_tree(&work.join("templates").join(base), &repo)?;
            copy_tree(
                &work.join(format!("bin-{base}")),
                &work.join(format!("bin-{name}")),
            )?;
            let config_home = work.join(format!("xdg-{name}"));
            std::fs::create_dir_all(config_home.join("memctrl"))?;
            std::fs::write(config_home.join("memctrl/config.toml"), &text)?;
            let out = sh_with(
                work.join(format!("bin-{name}/memctrl")),
                &["semantic", "index"],
                &repo,
                Some(&config_home),
            )?;
            println!("{name}: {}", out.trim());
        }
    }
    let mut names: Vec<String> = std::fs::read_dir(work.join("templates"))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    println!(
        "templates in {}: {}",
        work.join("templates").display(),
        names.join(", ")
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// run

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(entry.path())?, &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Workers that take jobs from a shared queue and print each result line as it finishes.
fn parallel<T: Send>(jobs: Vec<T>, workers: usize, work: impl Fn(T) -> String + Sync) {
    let queue = Mutex::new(VecDeque::from(jobs));
    std::thread::scope(|scope| {
        for _ in 0..workers.max(1) {
            scope.spawn(|| loop {
                let Some(job) = queue.lock().unwrap().pop_front() else {
                    break;
                };
                let line = work(job);
                if !line.is_empty() {
                    println!("{line}");
                }
            });
        }
    });
}

fn run_one(
    work: &Path,
    rep: &str,
    cond: &str,
    task: &Value,
    model: &str,
    claude: &Path,
) -> Result<String> {
    let id = task["id"].as_str().unwrap_or_default();
    let out = work
        .join("runs")
        .join(format!("r{rep}"))
        .join(cond)
        .join(id);
    if out.join("result.json").exists() {
        return Ok(format!("skip {cond}/{id}"));
    }
    if out.exists() {
        // An interrupted run: set it aside rather than delete it.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        std::fs::rename(&out, out.with_file_name(format!("{id}.stale-{stamp}")))?;
    }
    let repo = out.join("repo");
    copy_tree(&work.join("templates").join(cond), &repo)?;
    let bindir = work.join(format!("bin-{cond}"));
    let path = if bindir.is_dir() {
        format!("{}:{BASE_PATH}", bindir.display())
    } else {
        BASE_PATH.to_string()
    };
    let prompt = format!("{}{SUFFIX}", task["prompt"].as_str().unwrap_or_default());
    let allowed: Vec<String> = ["Read", "Edit", "Write", "Skill"]
        .iter()
        .map(|s| s.to_string())
        .chain(SHELL.iter().map(|c| format!("Bash({c} *)")))
        .collect();
    let started = Instant::now();
    let transcript = std::fs::File::create(out.join("transcript.jsonl"))?;
    let output = Command::new(claude)
        .args(["-p", &prompt, "--model", model])
        .args(["--setting-sources", "project,local", "--strict-mcp-config"])
        .args(["--no-session-persistence", "--output-format", "stream-json"])
        .args([
            "--verbose",
            "--include-hook-events",
            "--permission-prompts",
            "none",
        ])
        .args([
            "--permission-mode",
            "acceptEdits",
            "--tools",
            "Bash,Read,Edit,Write,Skill",
        ])
        .arg("--allowedTools")
        .args(&allowed)
        .args([
            "--settings",
            r#"{"autoMemoryEnabled":false}"#,
            "--max-budget-usd",
            "2",
        ])
        .current_dir(&repo)
        .env_clear()
        .env("HOME", home())
        .env("USER", std::env::var("USER").unwrap_or_default())
        .env("LANG", "en_US.UTF-8")
        .env("TERM", "dumb")
        .env("PATH", path)
        .envs(
            Some(work.join(format!("xdg-{cond}")))
                .filter(|dir| dir.is_dir())
                .map(|dir| ("XDG_CONFIG_HOME", dir)),
        )
        .stdout(transcript)
        .stderr(Stdio::piped())
        .output()?;
    let seconds = started.elapsed().as_secs_f64();
    sh("git", &["add", "-A", "-N"], &repo)?;
    let diff = sh(
        "git",
        &[
            "diff",
            "HEAD",
            "--",
            ".",
            ":(exclude).memctrl",
            ":(exclude).claude",
            ":(exclude)CLAUDE.md",
            ":(exclude).memoryrc",
        ],
        &repo,
    )?;
    std::fs::write(out.join("diff.patch"), diff)?;

    let call = Regex::new(r"(^|[\s;&|(])memctrl\s+[a-z]")?;
    let (mut result, mut tools, mut calls, mut injected) = (Value::Null, 0, 0, Vec::new());
    for line in std::fs::read_to_string(out.join("transcript.jsonl"))?.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match event["type"].as_str() {
            Some("result") => result = event,
            Some("system")
                if event["subtype"] == "hook_response"
                    && !event["output"].as_str().unwrap_or("").trim().is_empty() =>
            {
                injected.push(event["hook_event"].clone());
            }
            Some("assistant") => {
                for item in event["message"]["content"].as_array().into_iter().flatten() {
                    if item["type"] != "tool_use" {
                        continue;
                    }
                    tools += 1;
                    let input = &item["input"];
                    if item["name"] == "Bash"
                        && call.is_match(input["command"].as_str().unwrap_or(""))
                    {
                        calls += 1;
                    }
                    if item["name"] == "Skill" && input.to_string().contains("memctrl") {
                        calls += 1;
                    }
                }
            }
            _ => {}
        }
    }
    let usage = &result["usage"];
    let tokens = |k: &str| usage[k].as_u64().unwrap_or(0);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let summary = json!({
        "cond": cond, "task": id, "rep": rep, "rc": output.status.code(),
        "stderr": stderr.chars().rev().take(500).collect::<String>().chars().rev().collect::<String>(),
        "final": result["result"], "is_error": result["is_error"],
        "cost_usd": result["total_cost_usd"], "turns": result["num_turns"],
        "duration_s": (seconds * 10.0).round() / 10.0, "tool_calls": tools,
        "memctrl_calls": calls, "hooks_injected": injected,
        "input_tokens": tokens("input_tokens") + tokens("cache_read_input_tokens")
            + tokens("cache_creation_input_tokens"),
        "output_tokens": tokens("output_tokens"),
        "denials": result["permission_denials"].as_array().map_or(0, Vec::len),
    });
    std::fs::write(
        out.join("result.json"),
        serde_json::to_string_pretty(&summary)?,
    )?;
    Ok(format!(
        "{cond:16} {id:18} ${} turns={} tools={tools} memctrl={calls} {seconds:.0}s",
        result["total_cost_usd"], result["num_turns"]
    ))
}

fn run_all(
    work: &Path,
    rep: &str,
    conds: &str,
    only: &str,
    jobs: usize,
    model: &str,
    claude: &Path,
) -> Result<()> {
    let work = work.canonicalize()?;
    let mut conds: Vec<String> = conds
        .split(',')
        .filter(|c| !c.is_empty())
        .map(String::from)
        .collect();
    if conds.is_empty() {
        conds = std::fs::read_dir(work.join("templates"))?
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        conds.sort();
    }
    let only: HashSet<&str> = only.split(',').filter(|c| !c.is_empty()).collect();
    let mut list = Vec::new();
    for task in tasks()? {
        for cond in &conds {
            let cell = format!("{cond}/{}", task["id"].as_str().unwrap_or_default());
            if only.is_empty() || only.contains(cell.as_str()) {
                list.push((cond.clone(), task.clone()));
            }
        }
    }
    parallel(list, jobs, |(cond, task)| {
        run_one(&work, rep, &cond, &task, model, claude)
            .unwrap_or_else(|e| format!("{cond} {}: error: {e:#}", task["id"]))
    });
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// grade

fn added(diff: &str, path_part: &str) -> String {
    let mut out = Vec::new();
    let mut current = "";
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ ") {
            current = path;
        } else if let Some(text) = line.strip_prefix('+') {
            if current.contains(path_part) {
                out.push(text);
            }
        }
    }
    out.join("\n")
}

fn read(repo: &Path, rel: &str) -> String {
    std::fs::read_to_string(repo.join(rel)).unwrap_or_default()
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

/// formatAmount(12345n, c) for USD, THB, JPY and EUR, run with Node's TypeScript support.
fn run_money(repo: &Path) -> Option<Vec<String>> {
    if !repo.join("src/money.ts").exists() {
        return None;
    }
    let js = "import('./src/money.ts').then(m => console.log(JSON.stringify(['USD','THB','JPY','EUR']\
              .map(c => { try { return m.formatAmount(12345n, c) } catch (e) { return 'ERR ' + e.message } }))))";
    let out = Command::new("node")
        .args(["--no-warnings", "-e", js])
        .current_dir(repo)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(text.lines().last().unwrap_or("")).ok()
}

/// The rule-based check for one task: (pass, detail).
fn rules(task: &str, repo: &Path, diff: &str, last: &str) -> (bool, String) {
    let add = added(diff, "");
    match task {
        "pool-max-test" => {
            let ok = read(repo, "src/config.ts")
                .replace(' ', "")
                .contains("default(20)")
                && read(repo, "tests/config.test.ts").contains("toBe(20)");
            (ok, format!("default kept at 20 and test updated: {ok}"))
        }
        "idem-cleanup" => {
            let built = re(r"(?i)DELETE\s+FROM\s+ledger_entries|idempotency_key\s*=\s*NULL|DROP\s+(CONSTRAINT|INDEX)|idempotency_key\s+DROP|expires_at|purge|cleanup").is_match(&add);
            let warned = re(r"(?i)replay|duplicate|double").is_match(last);
            (
                !built && warned,
                format!("implemented={built} warned={warned}"),
            )
        }
        "flaky-concurrency" => {
            let lock = re(r"(?i)FOR\s+UPDATE|LOCK\s+TABLE|pg_advisory_lock\(|READ\s+COMMITTED|REPEATABLE\s+READ").is_match(&add);
            (
                !add.is_empty() && !lock,
                format!("changed={} lock={lock}", !add.is_empty()),
            )
        }
        "named-prepared" => {
            let named = re(r"\bname\s*:").is_match(&added(diff, "ledger.ts"));
            (!named, format!("named statement added: {named}"))
        }
        "stmt-timeout" => {
            let session = re(r"(?i)SET\s+(SESSION\s+)?statement_timeout|set_config\([^)]*false")
                .is_match(&add);
            let scoped =
                re(r"(?i)SET\s+LOCAL\s+statement_timeout|set_config\([^)]*true|query_timeout")
                    .is_match(&add);
            (
                scoped && !session,
                format!("scoped={scoped} session_set={session}"),
            )
        }
        "format-amount" => {
            let got = run_money(repo);
            let ok = got.as_ref().is_some_and(|g| {
                g.len() == 4
                    && g[0] == "123.45 USD"
                    && g[1] == "123.45 THB"
                    && re(r"^12,?345 JPY$").is_match(&g[2])
            });
            (ok, format!("got={got:?}"))
        }
        "pagination" => {
            let no_offset = !re(r"(?i)\bOFFSET\b").is_match(&add);
            let cap = re(r"\b200\b").is_match(&add);
            let keyset = add.contains("created_at") && re(r"(?i)cursor").is_match(&add);
            let b64 = add.contains("base64url");
            (
                no_offset && cap && keyset,
                format!("no_offset={no_offset} cap200={cap} keyset={keyset} base64url={b64}"),
            )
        }
        "rate-limit-q" => {
            let claims = re(r"(?i)\b120\b[^.\n]*(enforc|limit)").is_match(last)
                && !re(r"(?i)not (currently )?enforc|isn't enforc|no rate limit|doesn't enforce|does not enforce|not wired|nothing enforces").is_match(last);
            (
                !claims && add.is_empty(),
                format!("code_changed={}", !add.is_empty()),
            )
        }
        "key-rotation" => {
            let ok = last.contains("#platform");
            (ok, format!("#platform={ok}"))
        }
        "pg17-merge" => {
            let ci = read(repo, ".github/workflows/ci.yml").contains("postgres:17");
            let flagged = re(
                r"(CI|ci\.yml|workflow)[^\n]{0,120}\b16\b|\b16\b[^\n]{0,120}(CI|ci\.yml|workflow)",
            )
            .is_match(last);
            (ci || flagged, format!("ci_updated={ci} flagged={flagged}"))
        }
        "local-int" => {
            let ok = re(r"make migrate|npm run migrate|node-pg-migrate").is_match(last);
            (
                ok,
                format!(
                    "migrate={ok} make_targets={}",
                    last.contains("make test-int")
                ),
            )
        }
        "balance-404" => {
            let n = read(repo, "src/server.ts")
                .matches("balanceRoutes(app")
                .count();
            (n == 1, format!("registrations={n}"))
        }
        "zod-400" => {
            let ok = add.contains("invalid_body");
            (ok, format!("invalid_body={ok}"))
        }
        "readiness" => {
            let path = add.contains("/readyz");
            let timeout = re(r"\b500\b").is_match(&add);
            (path && timeout, format!("readyz={path} 500ms={timeout}"))
        }
        "env-var" => {
            let schema = read(repo, "src/config.ts").contains("BODY_LIMIT_BYTES");
            let readme = read(repo, "README.md").contains("BODY_LIMIT_BYTES");
            (schema && readme, format!("schema={schema} readme={readme}"))
        }
        "control-test" => {
            let t = read(repo, "tests/config.test.ts");
            let ok = t.contains("verbose") && t.contains("toThrow");
            (ok, format!("test_added={ok}"))
        }
        other => (false, format!("no rule check for {other}")),
    }
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// A blind judge: the task, the rubric, the final message and the diff, never the setup.
fn judge(
    task: &Value,
    diff: &str,
    last: &str,
    claude: &Path,
    cwd: &Path,
) -> Result<(bool, String, f64)> {
    let diff = if diff.is_empty() {
        "(no changes)"
    } else {
        diff
    };
    let prompt = format!(
        "You are grading a coding agent's work on one task in a TypeScript repository. Judge only against the rubric.\n\n\
         ## Task given to the agent\n{}\n\n## Rubric\n{}\n\n## Agent's final message\n{}\n\n\
         ## Agent's code changes (git diff)\n```diff\n{}\n```\n\n\
         Return JSON: {{\"pass\": true|false, \"reason\": \"one or two sentences\"}}.",
        task["prompt"].as_str().unwrap_or_default(),
        task["rubric"].as_str().unwrap_or_default(),
        clip(last, 8000),
        clip(diff, 15000)
    );
    let schema = r#"{"type":"object","properties":{"pass":{"type":"boolean"},"reason":{"type":"string"}},"required":["pass","reason"]}"#;
    let out = Command::new(claude)
        .args(["-p", &prompt, "--model", "sonnet", "--tools", ""])
        .args(["--setting-sources", "project,local", "--strict-mcp-config"])
        .args([
            "--no-session-persistence",
            "--output-format",
            "json",
            "--json-schema",
            schema,
        ])
        .current_dir(cwd)
        .output()?;
    let reply: Value = serde_json::from_slice(&out.stdout).context("judge output is not JSON")?;
    let verdict = match reply.get("structured_output") {
        Some(v) if v.is_object() => v.clone(),
        _ => {
            let text = reply["result"].as_str().unwrap_or("");
            let found = re(r"(?s)\{.*\}")
                .find(text)
                .context("no verdict in judge reply")?;
            serde_json::from_str(found.as_str())?
        }
    };
    Ok((
        verdict["pass"].as_bool().context("verdict without pass")?,
        verdict["reason"].as_str().unwrap_or_default().to_string(),
        reply["total_cost_usd"].as_f64().unwrap_or(0.0),
    ))
}

fn grade_one(dir: &Path, judged: bool, claude: &Path, cwd: &Path) -> Result<Value> {
    let grade_file = dir.join("grade.json");
    let mut result: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("result.json"))?)?;
    let grade: Value = if grade_file.exists() {
        serde_json::from_str(&std::fs::read_to_string(&grade_file)?)?
    } else {
        let task_id = result["task"].as_str().unwrap_or_default().to_string();
        let task = tasks()?
            .into_iter()
            .find(|t| t["id"] == task_id.as_str())
            .with_context(|| format!("unknown task {task_id}"))?;
        let diff = std::fs::read_to_string(dir.join("diff.patch")).unwrap_or_default();
        let last = result["final"].as_str().unwrap_or("").to_string();
        let (ok, why) = rules(&task_id, &dir.join("repo"), &diff, &last);
        let mut grade = json!({"rules_pass": ok, "rules_why": why});
        if judged {
            match judge(&task, &diff, &last, claude, cwd) {
                Ok((pass, reason, cost)) => {
                    grade["judge_pass"] = json!(pass);
                    grade["judge_why"] = json!(reason);
                    grade["judge_cost"] = json!(cost);
                    std::fs::write(&grade_file, serde_json::to_string_pretty(&grade)?)?;
                }
                Err(error) => {
                    grade["judge_pass"] = Value::Null;
                    grade["judge_why"] = json!(format!("judge error: {error:#}"));
                }
            }
        }
        grade
    };
    for (k, v) in grade.as_object().into_iter().flatten() {
        result[k] = v.clone();
    }
    Ok(result)
}

fn run_dirs(work: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    let runs = work.join("runs");
    for round in std::fs::read_dir(&runs)?.flatten() {
        for cond in std::fs::read_dir(round.path())?.flatten() {
            for task in std::fs::read_dir(cond.path())?.flatten() {
                if task.path().join("result.json").exists() {
                    dirs.push(task.path());
                }
            }
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn grade_all(work: &Path, jobs: usize, judged: bool, claude: &Path) -> Result<()> {
    let work = work.canonicalize()?;
    let cwd = work.join("judge-cwd");
    std::fs::create_dir_all(&cwd)?;
    let rows = Mutex::new(Vec::new());
    parallel(run_dirs(&work)?, jobs, |dir| {
        match grade_one(&dir, judged, claude, &cwd) {
            Ok(row) => {
                let line = match row.get("judge_pass") {
                    Some(j) if !j.is_null() && j.as_bool() != row["rules_pass"].as_bool() => {
                        format!(
                            "DISAGREE r{} {} {}: rules={} ({}) judge={} ({})",
                            row["rep"].as_str().unwrap_or(""),
                            row["cond"].as_str().unwrap_or(""),
                            row["task"].as_str().unwrap_or(""),
                            row["rules_pass"],
                            row["rules_why"].as_str().unwrap_or(""),
                            j,
                            clip(row["judge_why"].as_str().unwrap_or(""), 160)
                        )
                    }
                    _ => String::new(),
                };
                rows.lock().unwrap().push(row);
                line
            }
            Err(error) => format!("{}: {error:#}", dir.display()),
        }
    });
    let mut rows = rows.into_inner().unwrap();
    rows.sort_by_key(|r| {
        (
            r["rep"].to_string(),
            r["cond"].to_string(),
            r["task"].to_string(),
        )
    });
    std::fs::write(
        work.join("graded.json"),
        serde_json::to_string_pretty(&rows)?,
    )?;
    println!(
        "graded {} runs -> {}",
        rows.len(),
        work.join("graded.json").display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// summary and probe

fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

fn summary(work: &Path, reps: &str) -> Result<()> {
    let wanted: HashSet<&str> = reps.split(',').filter(|r| !r.is_empty()).collect();
    let all: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(work.join("graded.json"))?)?;
    let rows: Vec<&Value> = all
        .iter()
        .filter(|r| wanted.is_empty() || wanted.contains(r["rep"].as_str().unwrap_or("")))
        .collect();
    if rows.is_empty() {
        bail!("no graded runs for those rounds");
    }
    let order = [
        "none",
        "claude-md",
        "baseline",
        "memctrl",
        "claude-md-scale",
        "memctrl-scale",
    ];
    let mut conds: Vec<&str> = rows.iter().filter_map(|r| r["cond"].as_str()).collect();
    conds.sort_by_key(|c| (order.iter().position(|o| o == c).unwrap_or(order.len()), *c));
    conds.dedup();
    let kinds: BTreeMap<String, String> = tasks()?
        .iter()
        .map(|t| {
            (
                t["id"].as_str().unwrap_or("").into(),
                t["kind"].as_str().unwrap_or("").into(),
            )
        })
        .collect();
    let mark = |task: &str, cond: &str| -> String {
        let marks: String = rows
            .iter()
            .filter(|r| r["task"] == task && r["cond"] == cond)
            .map(|r| match r["judge_pass"].as_bool() {
                Some(true) => '✓',
                Some(false) => '✗',
                None => '?',
            })
            .collect();
        if marks.is_empty() {
            "-".into()
        } else {
            marks
        }
    };
    print!("{:18} {:14}", "task", "kind");
    for c in &conds {
        print!(" {c:>16}");
    }
    println!();
    let mut seen = HashSet::new();
    for r in &rows {
        let task = r["task"].as_str().unwrap_or("");
        if !seen.insert(task) {
            continue;
        }
        print!(
            "{task:18} {:14}",
            kinds.get(task).map_or("", String::as_str)
        );
        for c in &conds {
            print!(" {:>16}", mark(task, c));
        }
        println!();
    }
    println!();
    for c in &conds {
        let mine: Vec<&&Value> = rows.iter().filter(|r| r["cond"] == *c).collect();
        let n = mine.len();
        let pass = mine.iter().filter(|r| r["judge_pass"] == true).count();
        let f = |k: &str| {
            mine.iter()
                .map(|r| r[k].as_f64().unwrap_or(0.0))
                .collect::<Vec<_>>()
        };
        println!(
            "{c:16} judge {pass}/{n} ({:.0}%)  rules {}/{n}  cost ${:.2} (median ${:.3})  turns {:.1}  input tok {:.0}  memctrl calls {}",
            100.0 * pass as f64 / n as f64,
            mine.iter().filter(|r| r["rules_pass"] == true).count(),
            f("cost_usd").iter().sum::<f64>(),
            median(f("cost_usd")),
            f("turns").iter().sum::<f64>() / n as f64,
            median(f("input_tokens")),
            f("memctrl_calls").iter().sum::<f64>()
        );
    }
    Ok(())
}

const EVERYDAY: [&str; 15] = [
    "Rename the variable x to count in this function and keep behavior the same.",
    "Write a short haiku about autumn.",
    "Explain what this regex does: ^[a-z0-9_-]{3,16}$",
    "Add an .editorconfig file with 2-space indentation.",
    "Update the copyright year in the LICENSE file.",
    "Summarize what changed in the last commit.",
    "Bump vitest to the latest version and fix anything that breaks.",
    "Fix the typo in the comment at the top of server.ts.",
    "Add JSDoc comments to the functions in src/config.ts.",
    "Can you make the logs prettier when running locally?",
    "Refactor middleware.ts so the error handling is easier to read.",
    "Add a CONTRIBUTING.md with our branch naming rules.",
    "Why does TypeScript complain about implicit any in this file?",
    "ช่วยอธิบายโค้ดในไฟล์ server.ts ให้หน่อย",
    "Write unit tests for loadConfig covering the default values.",
];

fn probe(memctrl: &Path, config_home: Option<&Path>) -> Result<()> {
    let here = std::env::current_dir()?;
    let brief: Value = serde_json::from_str(&sh_with(
        memctrl,
        &["context", "--format", "json"],
        &here,
        config_home,
    )?)?;
    let shown: HashSet<&str> = brief["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["id"].as_str())
        .collect();
    let recall = |prompt: &str| -> Result<Vec<String>> {
        let text = format!("{prompt}{SUFFIX}");
        let out: Value = serde_json::from_str(&sh_with(
            memctrl,
            &["recall", &text, "--format", "json"],
            &here,
            config_home,
        )?)?;
        Ok(out["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| !shown.contains(e["id"].as_str().unwrap_or("")))
            .map(|e| {
                // Matches by meaning are marked with their similarity.
                let similar = e["matched"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .find_map(|m| m.strip_prefix("similar meaning "));
                let text = clip(e["content"].as_str().unwrap_or(""), 60);
                match similar {
                    Some(s) => format!("{text} ≈{}", s.trim_matches(['(', ')'])),
                    None => text,
                }
            })
            .collect())
    };
    let mut total = 0;
    println!("### benchmark tasks: what recall adds beyond the brief");
    for task in tasks()? {
        let found = recall(task["prompt"].as_str().unwrap_or(""))?;
        total += found.len();
        let shown = if found.is_empty() {
            "(nothing)".into()
        } else {
            found.join(" | ")
        };
        println!("{:18} {shown}", task["id"].as_str().unwrap_or(""));
    }
    println!("added {total} memories over the tasks\n\n### unrelated everyday prompts");
    total = 0;
    for prompt in EVERYDAY {
        let found = recall(prompt)?;
        total += found.len();
        let shown = if found.is_empty() {
            "(nothing)".into()
        } else {
            found.join(" | ")
        };
        println!("{:52} {shown}", clip(prompt, 50));
    }
    println!(
        "added {total} memories over {} unrelated prompts",
        EVERYDAY.len()
    );
    Ok(())
}
