"""MemCtrl — Typer CLI with rich formatting.

Commands:
    install, init, add, query, list, tree, forget, clear,
    trigger, audit, serve, otel-export, otel-stats, --version

Uses typer + rich for beautiful terminal output.
"""

from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
from typing import Optional

import typer
from rich.console import Console
from rich.panel import Panel
from rich.table import Table
from rich.tree import Tree as RichTree

from memctrl import __version__
from memctrl.cache import QueryCache
from memctrl.llm_client import create_llm_client
from memctrl.provenance import ProvenanceTracker
from memctrl.span import SpanTracker


# ---------------------------------------------------------------------------
# Cache factory (persistent across CLI invocations)
# ---------------------------------------------------------------------------

def _get_cache():
    """Get or create QueryCache with persistent SQLite storage.

    WHY: The module-level cache is fresh per process. By backing it with
    SQLite, repeat queries across separate CLI invocations hit cache too.
    """
    store = _get_store()
    db_dir = Path(store.db_path).parent
    cache_db = str(db_dir / "query_cache.db")
    return QueryCache(db_path=cache_db)


# ---------------------------------------------------------------------------
# LLM client factory
# ---------------------------------------------------------------------------

def _get_llm_client(provider: Optional[str] = None, model: Optional[str] = None, api_key: Optional[str] = None):
    """Create LLM client from CLI flags or environment."""
    return create_llm_client(provider=provider, model=model, api_key=api_key)


# ---------------------------------------------------------------------------
# Provenance tracker factory
# ---------------------------------------------------------------------------

def _get_provenance_tracker():
    """Get a ProvenanceTracker backed by SQLite for cross-process persistence.

    WHY: CLI commands run in separate processes, so an in-memory tracker
    loses all history on exit. Wiring the tracker to the same SQLite DB
    as the store ensures provenance survives across invocations.
    """
    from memctrl.provenance import ProvenanceTracker
    store = _get_store()
    return ProvenanceTracker(store=store, persist=True)


app = typer.Typer(
    name="memctrl",
    help="Rule-governed memory layer for AI coding assistants",
    add_completion=False,
)
console = Console()


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _get_store():
    """Get MemoryStore instance with project-local or global DB path."""
    from memctrl.store import MemoryStore

    # 1. Environment variable overrides everything
    db_path = os.environ.get("MEMCTRL_DB_PATH")
    if db_path:
        return MemoryStore(db_path)

    # 2. If .memoryrc exists in cwd, use its db_path
    rc_path = Path(".memoryrc")
    if rc_path.exists():
        try:
            from memctrl.rules import RuleEngine

            engine = RuleEngine(str(rc_path))
            rules = engine.load()
            if rules.db_path:
                # Resolve relative to .memoryrc location (cwd)
                resolved = Path(rules.db_path)
                if not resolved.is_absolute():
                    resolved = rc_path.parent / resolved
                resolved.parent.mkdir(parents=True, exist_ok=True)
                return MemoryStore(str(resolved))
        except Exception:
            pass  # Fallback to global default

    # 3. Global default
    return MemoryStore()


def _get_engine():
    """Get RuleEngine instance."""
    from memctrl.rules import RuleEngine

    return RuleEngine()


# ---------------------------------------------------------------------------
# Callback (version)
# ---------------------------------------------------------------------------


@app.callback(invoke_without_command=True)
def main(
    ctx: typer.Context,
    version: bool = typer.Option(False, "--version", help="Show version"),
):
    if version:
        console.print(f"MemCtrl v{__version__}")
        raise typer.Exit()


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------


@app.command()
def install(
    tool: Optional[str] = typer.Option(None, help="Specific tool to install for"),
    project: bool = typer.Option(
        False, help="Install at project level (.claude/ etc.)"
    ),
):
    """Register SKILL.md with AI coding tools (Claude Code, Cursor, etc.)"""
    from memctrl.installer import install_skill

    paths = install_skill(tool=tool, project=project, verbose=True)
    if paths:
        console.print(f"\n[green]Installed to {len(paths)} location(s)[/green]")
    else:
        console.print("\n[yellow]No tools installed. See paths above.[/yellow]")


@app.command()
def init(
    force: bool = typer.Option(False, help="Overwrite existing .memoryrc"),
):
    """Create .memoryrc and project-local database in current directory"""
    dest = Path(".memoryrc")
    if dest.exists() and not force:
        console.print(
            f"[yellow]{dest} already exists. Use --force to overwrite.[/yellow]"
        )
        raise typer.Exit(1)

    example = Path(__file__).parent / ".memoryrc.example"
    if example.exists():
        content = example.read_text()
    else:
        content = _default_memoryrc()

    dest.write_text(content)
    console.print(f"[green]Created {dest}[/green]")

    # Create project-local database directory and initialize empty DB
    db_dir = Path(".memctrl")
    db_dir.mkdir(parents=True, exist_ok=True)
    db_path = db_dir / "memories.db"
    if not db_path.exists():
        from memctrl.store import MemoryStore

        MemoryStore(str(db_path))  # initializes schema
        console.print(f"[green]Created {db_path}[/green]")
    else:
        console.print(f"[dim]{db_path} already exists[/dim]")


@app.command()
def add(
    content: str = typer.Argument(..., help="Memory content to store"),
    layer: str = typer.Option("session", help="Layer: project/session/user"),
    source: str = typer.Option("manual", help="Source of this memory"),
    llm_provider: Optional[str] = typer.Option(None, help="LLM provider (openai, anthropic, etc.)"),
    llm_model: Optional[str] = typer.Option(None, help="LLM model name"),
    llm_api_key: Optional[str] = typer.Option(None, help="LLM API key"),
):
    """Manually add a memory"""
    store = _get_store()
    mid = store.insert_memory(layer=layer, content=content, source=source)
    # Invalidate cache because the memory tree has changed.
    cache = _get_cache()
    cache.invalidate()
    # Run decay if needed
    from memctrl.decay import ConfidenceDecay
    decay = ConfidenceDecay(store)
    store.run_decay_if_needed(decay)
    console.print(
        f"[green]Added memory[/green] [dim]{mid}[/dim] to [bold]{layer}[/bold]"
    )


@app.command()
def query(
    query_text: str = typer.Argument(..., help="Query to search memory"),
    layer: Optional[str] = typer.Option(None, help="Filter by layer"),
    llm_provider: Optional[str] = typer.Option(None, help="LLM provider (openai, anthropic, etc.)"),
    llm_model: Optional[str] = typer.Option(None, help="LLM model name"),
    llm_api_key: Optional[str] = typer.Option(None, help="LLM API key"),
):
    """Retrieve relevant memories with reasoning trace.

    Uses the query result cache to skip tree building and retrieval
    for repeat queries against an unchanged memory tree.
    """
    cache = _get_cache()

    # Check cache first — fast path for repeat queries.
    cached = cache.get(query_text)
    if cached is not None:
        result = cached
    else:
        # Cache miss — do full retrieval and cache the result.
        store = _get_store()
        engine = _get_engine()
        engine.load()

        # Run decay if needed before querying
        from memctrl.decay import ConfidenceDecay
        decay = ConfidenceDecay(store)
        store.run_decay_if_needed(decay)

        # WAL checkpoint to prevent unbounded growth
        store.wal_checkpoint()

        memories = store.list_memories(layer=layer)
        if not memories:
            console.print("[yellow]No memories found.[/yellow]")
            return

        mem_dicts = [m.to_dict() for m in memories]
        memory_lookup = {m.id: m.to_dict() for m in memories}

        # Build tree (with LLM if configured)
        llm_client = _get_llm_client(provider=llm_provider, model=llm_model, api_key=llm_api_key)
        from memctrl.tree import MemoryTreeBuilder
        builder = MemoryTreeBuilder(llm_client=llm_client)

        async def _do_query():
            tree = await builder.build_tree(mem_dicts)
            tree_dict = tree.to_dict()

            # Retrieve with provenance tracking and optional LLM
            from memctrl.retriever import MemoryRetriever
            retriever = MemoryRetriever(
                llm_client=llm_client,
                provenance_tracker=_get_provenance_tracker(),
            )
            result = await retriever.retrieve(
                query_text, tree_dict, memory_lookup=memory_lookup
            )
            return result

        result = asyncio.run(_do_query())
        cache.set(query_text, result)

    if result.facts:
        console.print(Panel(f"[bold]Query:[/bold] {query_text}", title="memctrl"))
        console.print("\n[bold green]Facts:[/bold green]")
        for i, fact in enumerate(result.facts, 1):
            console.print(f"  {i}. {fact}")
        console.print(f"\n[bold blue]Trace:[/bold blue] {' -> '.join(result.trace)}")
        console.print(f"[bold]Confidence:[/bold] {result.confidence:.2f}")
    else:
        console.print(f"[yellow]No relevant memories found for:[/yellow] {query_text}")


@app.command("list")
def list_memories(
    layer: Optional[str] = typer.Option(None, help="Filter by layer"),
    limit: int = typer.Option(50, help="Max results"),
):
    """List all memories"""
    store = _get_store()
    memories = store.list_memories(layer=layer)[:limit]

    if not memories:
        console.print("[yellow]No memories found.[/yellow]")
        return

    table = Table(title="Memories", show_lines=True)
    table.add_column("ID", style="dim", max_width=8)
    table.add_column("Layer", style="cyan")
    table.add_column("Content", max_width=60)
    table.add_column("Source", style="green")
    table.add_column("Conf", justify="right")

    for mem in memories:
        table.add_row(
            mem.id[:8],
            mem.layer,
            mem.content[:80],
            mem.source,
            f"{mem.confidence:.1f}",
        )
    console.print(table)


@app.command()
def tree(
    llm_provider: Optional[str] = typer.Option(None, help="LLM provider (openai, anthropic, etc.)"),
    llm_model: Optional[str] = typer.Option(None, help="LLM model name"),
    llm_api_key: Optional[str] = typer.Option(None, help="LLM API key"),
):
    """Display memory tree (rich formatted)"""
    store = _get_store()
    memories = store.list_memories()

    if not memories:
        console.print("[yellow]No memories to display.[/yellow]")
        return

    llm_client = _get_llm_client(provider=llm_provider, model=llm_model, api_key=llm_api_key)
    from memctrl.tree import MemoryTreeBuilder
    builder = MemoryTreeBuilder(llm_client=llm_client)

    async def _do_tree():
        mem_dicts = [m.to_dict() for m in memories]
        root = await builder.build_tree(mem_dicts)
        return root

    root = asyncio.run(_do_tree())

    def _build_rich(node, rich_node):
        for child in node.children:
            if child.is_leaf() or not child.children:
                label = f"[dim][mem][/dim] {child.title[:50]}"
            else:
                label = f"[bold][dir] {child.title}[/bold]"
                if child.confidence < 1.0:
                    label += f" [dim](conf: {child.confidence})[/dim]"
            branch = rich_node.add(label)
            _build_rich(child, branch)

    rich_root = RichTree("[bold]Memory Tree[/bold]")
    _build_rich(root, rich_root)
    console.print(rich_root)


@app.command()
def forget(
    memory_id: str = typer.Argument(..., help="Memory ID to forget"),
):
    """Remove a memory by ID"""
    store = _get_store()
    deleted = store.delete_memory(memory_id)
    if deleted:
        cache = _get_cache()
        cache.invalidate()
        console.print(f"[green]Forgot memory[/green] [dim]{memory_id}[/dim]")
    else:
        console.print(f"[red]Memory not found:[/red] {memory_id}")


@app.command()
def clear(
    layer: Optional[str] = typer.Option(None, help="Clear specific layer"),
    yes: bool = typer.Option(False, "--yes", help="Skip confirmation"),
):
    """Clear memories (all or by layer)"""
    store = _get_store()
    memories = store.list_memories(layer=layer)
    count = len(memories)

    if count == 0:
        console.print("[yellow]No memories to clear.[/yellow]")
        return

    target = f"'{layer}' layer" if layer else "ALL memories"
    if not yes:
        confirm = typer.confirm(f"Clear {target}? ({count} memories)")
        if not confirm:
            console.print("Cancelled.")
            return

    deleted_any = False
    if layer:
        for mem in memories:
            if store.delete_memory(mem.id):
                deleted_any = True
    else:
        for mem in memories:
            if store.delete_memory(mem.id):
                deleted_any = True

    if deleted_any:
        cache = _get_cache()
        cache.invalidate()

    console.print(f"[green]Cleared {count} memories.[/green]")


@app.command()
def decay(
    dry_run: bool = typer.Option(False, help="Show what would decay without applying"),
    threshold: float = typer.Option(0.3, help="Confidence threshold below which memories are flagged"),
):
    """Run confidence decay on all memories.

    Reduces confidence scores of older memories based on age.
    Memories that drop below the threshold are flagged for review.
    """
    store = _get_store()
    from memctrl.decay import ConfidenceDecay
    decay_engine = ConfidenceDecay(store)

    if dry_run:
        flagged = store.get_memories_below_confidence(threshold)
        console.print(f"[dim]{len(flagged)} memories below confidence {threshold}[/dim]")
        for mem in flagged[:20]:
            console.print(f"  [yellow]{mem.id[:8]}[/yellow] {mem.confidence:.2f} {mem.content[:60]}")
        return

    decayed = decay_engine.decay_memories()
    store._last_decay_at = __import__('datetime').datetime.now()
    console.print(f"[green]Decayed {len(decayed)} memories[/green]")

    flagged = store.get_memories_below_confidence(threshold)
    if flagged:
        console.print(f"[yellow]⚠ {len(flagged)} memories now below threshold {threshold}[/yellow]")


@app.command()
def trigger_cmd(
    event: str = typer.Argument(..., help="Event name (e.g., on_session_end)"),
    context: Optional[str] = typer.Option(None, help="JSON context string"),
):
    """Manually fire a trigger"""
    store = _get_store()
    engine = _get_engine()
    engine.load()

    ctx = {}
    if context:
        try:
            ctx = json.loads(context)
        except json.JSONDecodeError:
            console.print("[red]Invalid JSON context[/red]")
            return

    ids = engine.fire_trigger(event, ctx, store)
    if ids:
        cache = _get_cache()
        cache.invalidate()
    console.print(
        f"[green]Trigger '{event}' fired[/green] - {len(ids)} memories affected"
    )


@app.command()
def audit(
    limit: int = typer.Option(50, help="Number of log entries to show"),
):
    """Show audit log of triggers"""
    store = _get_store()
    logs = store.get_trigger_log(limit=limit)

    if not logs:
        console.print("[yellow]No audit entries.[/yellow]")
        return

    table = Table(title="Trigger Audit Log")
    table.add_column("Timestamp", style="dim")
    table.add_column("Event", style="cyan")
    table.add_column("Action")
    table.add_column("Memories", justify="right")

    for log in logs:
        table.add_row(
            log.timestamp.strftime("%Y-%m-%d %H:%M"),
            log.event,
            log.action,
            str(len(log.memories_affected)),
        )
    console.print(table)


@app.command()
def provenance(
    query: Optional[str] = typer.Argument(None, help="Query to show provenance for"),
    full: bool = typer.Option(False, help="Show full provenance details"),
):
    """Show retrieval provenance for the last query or a specific query.

    Displays the provenance trail — which memories were retrieved, why they
    matched, their source layers, and confidence scores. This enables
    trust, debugging, and audit of memory retrieval decisions.

    Without a query argument, shows a summary of recent retrieval history.
    With a query, shows provenance for that specific query.
    """
    tracker = _get_provenance_tracker()
    history = tracker.get_history()

    if not history:
        console.print(
            "[yellow]No provenance recorded yet.[/yellow]\n"
            "Run [bold]memctrl query '<question>'[/bold] first to generate provenance."
        )
        return

    if query:
        # Show provenance for a specific query
        records = [r for r in history if r.query == query]
        if not records:
            console.print(f"[yellow]No provenance found for query:[/yellow] {query}")
            return
        record = records[-1]  # Most recent
    else:
        # Show the most recent retrieval's provenance
        record = history[-1]

    # Header
    console.print(
        Panel(
            f"[bold]Query:[/bold] {record.query}\n"
            f"[bold]Method:[/bold] {record.retrieval_method}\n"
            f"[bold]Memories searched:[/bold] {record.total_memories_searched}\n"
            f"[bold]Avg confidence:[/bold] {record.avg_confidence:.2f}",
            title="Retrieval Provenance",
            border_style="cyan",
        )
    )

    # Sources table
    if record.sources:
        table = Table(title="Retrieved Memories", show_lines=True)
        table.add_column("ID", style="dim", max_width=8)
        table.add_column("Layer", style="cyan")
        table.add_column("Source Type", style="green")
        table.add_column("Confidence", justify="right")
        table.add_column("Match Reason", max_width=40)

        for src in record.sources:
            table.add_row(
                src.memory_id[:8] if len(src.memory_id) > 8 else src.memory_id,
                src.layer,
                src.source_type,
                f"{src.confidence:.2f}",
                src.match_reason,
            )
        console.print(table)

        # Layer breakdown bar chart
        console.print("\n[bold]Layer Breakdown:[/bold]")
        for layer, count in record.layer_breakdown.items():
            bar = "█" * count + "░" * (10 - min(count, 10))
            console.print(f"  {layer:12} {bar} {count}")

        # Source type breakdown
        console.print("\n[bold]Source Type Breakdown:[/bold]")
        for st, count in record.source_type_breakdown.items():
            bar = "█" * count + "░" * (10 - min(count, 10))
            console.print(f"  {st:12} {bar} {count}")
    else:
        console.print("[dim]No memories retrieved in this operation.[/dim]")

    # Detect anomalies
    low_conf = tracker.detect_low_confidence_retrievals(threshold=0.5)
    if low_conf:
        console.print(
            f"\n[yellow]⚠ {len(low_conf)} low-confidence retrieval(s) detected.[/yellow]"
        )

    imbalance = tracker.detect_source_type_imbalance()
    if imbalance:
        console.print(
            f"\n[yellow]⚠ Source imbalance detected:[/yellow] {imbalance['message']}"
        )

    if full:
        # Show full JSON serialization
        console.print("\n[bold]Full Provenance (JSON):[/bold]")
        console.print_json(json.dumps(record.to_dict(), indent=2))


@app.command()
def heatmap():
    """Show memory distribution heatmap by layer and tags"""
    store = _get_store()
    memories = store.list_memories()

    if not memories:
        console.print("[yellow]No memories found.[/yellow]")
        return

    console.print(Panel("[bold]Memory Heatmap[/bold]", border_style="cyan"))

    # Layer distribution
    console.print("\n[bold]By Layer:[/bold]")
    by_layer: dict = {}
    for mem in memories:
        by_layer[mem.layer] = by_layer.get(mem.layer, 0) + 1
    total = len(memories)
    for layer, count in sorted(by_layer.items(), key=lambda x: -x[1]):
        pct = count / total * 100
        bar_len = int(pct / 5)
        bar = "#" * bar_len + "-" * (20 - bar_len)
        color = (
            "green"
            if layer == "project"
            else "yellow"
            if layer == "session"
            else "blue"
        )
        console.print(f"  [{color}]{layer:10}[/{color}] {bar} {count:3} ({pct:.0f}%)")

    # Tag distribution
    tag_counts: dict = {}
    for mem in memories:
        for tag in mem.tags:
            tag_counts[tag] = tag_counts.get(tag, 0) + 1

    if tag_counts:
        console.print("\n[bold]By Tag:[/bold]")
        for tag, count in sorted(tag_counts.items(), key=lambda x: -x[1])[:10]:
            bar_len = min(count, 20)
            bar = "#" * bar_len + "-" * (20 - bar_len)
            console.print(f"  {tag:15} {bar} {count}")

    console.print(f"\n[dim]Total: {total} memories[/dim]")


@app.command()
def timeline(
    limit: int = typer.Option(20, help="Max events to show"),
):
    """Show chronological memory timeline"""
    store = _get_store()
    memories = store.list_memories()[:limit]
    logs = store.get_trigger_log(limit=limit)

    if not memories and not logs:
        console.print("[yellow]No timeline events.[/yellow]")
        return

    console.print(Panel("[bold]Memory Timeline[/bold]", border_style="cyan"))

    # Merge and sort events
    events = []
    for mem in memories:
        events.append(
            {
                "ts": mem.created_at,
                "type": "memory",
                "layer": mem.layer,
                "content": mem.content[:60],
                "icon": "[mem]",
            }
        )
    for log in logs:
        events.append(
            {
                "ts": log.timestamp,
                "type": "trigger",
                "layer": "",
                "content": f"{log.event}: {log.action}",
                "icon": "[refl]",
            }
        )

    events.sort(key=lambda x: x["ts"], reverse=True)

    for ev in events[:limit]:
        ts = ev["ts"].strftime("%Y-%m-%d %H:%M") if ev["ts"] else "?"
        if ev["type"] == "memory":
            color = (
                "green"
                if ev["layer"] == "project"
                else "yellow"
                if ev["layer"] == "session"
                else "blue"
            )
            console.print(
                f"  [dim]{ts}[/dim] [{color}]{ev['icon']}[/{color}] ({ev['layer']}) {ev['content']}"
            )
        else:
            console.print(
                f"  [dim]{ts}[/dim] [magenta]{ev['icon']}[/magenta] {ev['content']}"
            )


@app.command()
def done(
    llm_provider: Optional[str] = typer.Option(None, help="LLM provider (openai, anthropic, etc.)"),
    llm_model: Optional[str] = typer.Option(None, help="LLM model name"),
    llm_api_key: Optional[str] = typer.Option(None, help="LLM API key"),
):
    """Explicit session end — triggers reflection immediately

    This is the shorthand for "I'm done with this session". It forces
    reflection to run, consolidating all session memories into project
    and user layers regardless of heuristics.
    """
    from memctrl.reflection import ReflectionEngine

    store = _get_store()
    engine = _get_engine()
    engine.load()

    llm_client = _get_llm_client(provider=llm_provider, model=llm_model, api_key=llm_api_key)
    reflection = ReflectionEngine(store, engine=engine, llm_client=llm_client)
    result = reflection.check_and_reflect(force=True)

    if result.triggered:
        console.print(
            f"[green]Session consolidated[/green] — "
            f"{len(result.consolidated_ids)} memories moved"
        )
        if result.summary:
            console.print(
                Panel(f"[bold]Summary:[/bold] {result.summary}", border_style="green")
            )
        if result.new_memories:
            console.print(
                f"[dim]Created {len(result.new_memories)} reflection memory[/dim]"
            )
    else:
        console.print("[yellow]Nothing to consolidate.[/yellow]")


@app.command()
def reflect(
    llm_provider: Optional[str] = typer.Option(None, help="LLM provider (openai, anthropic, etc.)"),
    llm_model: Optional[str] = typer.Option(None, help="LLM model name"),
    llm_api_key: Optional[str] = typer.Option(None, help="LLM API key"),
):
    """Manual reflection — checks heuristics and consolidates if triggered

    Checks detection heuristics (time-based, git-based) and runs consolidation
    if any fire. Use ``memctrl done`` to force reflection regardless of
    heuristics.
    """
    from memctrl.reflection import ReflectionEngine

    store = _get_store()
    engine = _get_engine()
    engine.load()

    llm_client = _get_llm_client(provider=llm_provider, model=llm_model, api_key=llm_api_key)
    reflection = ReflectionEngine(store, engine=engine, llm_client=llm_client)
    result = reflection.check_and_reflect(force=False)

    if result.triggered:
        console.print(
            f"[green]Reflection triggered[/green] ([cyan]{result.event}[/cyan]) — "
            f"{len(result.consolidated_ids)} memories consolidated"
        )
        if result.summary:
            console.print(
                Panel(f"[bold]Summary:[/bold] {result.summary}", border_style="green")
            )
    else:
        console.print(
            "[dim]No reflection triggered. Heuristics: time-based ("
            f"{reflection.inactivity_hours}h inactivity), git commit.[/dim]"
        )


@app.command()
def spans(
    name: Optional[str] = typer.Option(None, help="Filter spans by name pattern"),
    demo: bool = typer.Option(False, help="Show demo spans with sample operations"),
):
    """Show recent memory spans

    Displays memory operation spans for debugging, compliance, and
    observability. Spans are created programmatically via the
    SpanTracker context manager.

    Usage:
        memctrl spans              # Show usage info
        memctrl spans --demo       # Show demo spans
        memctrl spans --name auth  # Show spans matching 'auth'
    """
    tracker = SpanTracker()

    if demo:
        # Create demo spans with sample operations for illustration
        with tracker.span("debug_auth_issue", agent="dev1") as span:
            tracker.record_operation(
                "retrieve",
                memory_id="m1",
                layer="project",
                content_preview="OAuth2 requirements",
                query="auth requirements",
            )
            tracker.record_operation(
                "store",
                memory_id="m2",
                layer="session",
                content_preview="discovered JWT validation bug",
                confidence=1.0,
            )
            tracker.record_operation(
                "retrieve",
                memory_id="m3",
                layer="project",
                content_preview="JWT implementation details",
                query="jwt implementation",
            )

        with tracker.span("implement_feature_x", agent="dev1", task="backend"):
            tracker.record_operation("store", memory_id="m4", layer="session")
            tracker.record_operation("store", memory_id="m5", layer="session")
            tracker.record_operation("retrieve", memory_id="m6", layer="project")

    completed = tracker.get_completed_spans()

    # Apply name filter if provided
    if name and completed:
        completed = [s for s in completed if name.lower() in s.name.lower()]

    if not completed:
        if name:
            console.print(f"[yellow]No spans matching '{name}'.[/yellow]")
        else:
            console.print(
                "[dim]No memory spans recorded yet.[/dim]\n\n"
                "Spans are created programmatically:\n"
                "  from memctrl.span import SpanTracker\n"
                "  tracker = SpanTracker()\n"
                '  with tracker.span("my_task"):\n'
                "      store.insert_memory(...)\n"
                "      tracker.record_operation('store', memory_id='m1', layer='project')\n\n"
                "Use --demo to see example output."
            )
        return

    table = Table(title="Memory Spans", show_lines=True)
    table.add_column("Span Name", style="cyan")
    table.add_column("Duration (ms)", justify="right")
    table.add_column("Operations", justify="right")
    table.add_column("Breakdown", max_width=40)
    table.add_column("Metadata", style="dim")

    for span in completed:
        counts = span.operation_counts
        breakdown = ", ".join(f"{k}: {v}" for k, v in counts.items())
        meta_str = ", ".join(f"{k}={v}" for k, v in span.metadata.items())
        table.add_row(
            span.name,
            f"{span.duration_ms:.1f}",
            str(len(span.operations)),
            breakdown,
            meta_str,
        )

    console.print(table)

    # Show per-span operation details
    for span in completed:
        op_table = Table(title=f"  Operations: {span.name}", show_lines=False)
        op_table.add_column("Op", style="bold")
        op_table.add_column("Layer", style="cyan")
        op_table.add_column("Memory ID", style="dim", max_width=12)
        op_table.add_column("Preview / Query", max_width=50)

        for op in span.operations:
            preview = op.query if op.query else (op.content_preview or "")
            op_table.add_row(
                op.operation,
                op.layer or "-",
                (op.memory_id or "-")[:12],
                preview,
            )
        console.print(op_table)
        console.print()


@app.command()
def serve(
    port: int = typer.Option(8080, help="Port to run MCP server on"),
    host: str = typer.Option("127.0.0.1", help="Host to bind to"),
):
    """Start MCP server (stdio-based, not HTTP)"""
    console.print(f"[green]Starting MCP server[/green]")
    console.print("[dim]Use Ctrl+C to stop[/dim]")

    from memctrl.mcp_server import serve_mcp

    asyncio.run(serve_mcp())


# ---------------------------------------------------------------------------
# Default .memoryrc content
# ---------------------------------------------------------------------------


@app.command("otel-export")
def otel_export(
    output: str = typer.Option("memctrl_spans.json", help="Output file path"),
    otlp: bool = typer.Option(False, help="Export in OTLP-compatible format"),
):
    """Export recent memory operation spans to OTel JSON"""
    from memctrl.otel_exporter import MemoryOTelExporter

    exporter = MemoryOTelExporter(service_name="memctrl-cli")
    # Export synthetic spans from actual store activity
    exporter.start()
    store = _get_store()
    _ = store.stats()

    # Record spans for each memory in the store
    memories = store.list_memories()
    for mem in memories:
        exporter.record_store(
            memory_id=mem.id,
            layer=mem.layer,
            content=mem.content,
            confidence=mem.confidence,
            duration_ms=0.0,
        )

    if otlp:
        exporter.export_otlp_json(output)
        console.print(
            f"[green]Exported {len(memories)} spans (OTLP) to[/green] {output}"
        )
    else:
        exporter.export_json(output)
        console.print(f"[green]Exported {len(memories)} spans to[/green] {output}")
    exporter.stop()


@app.command("otel-stats")
def otel_stats():
    """Show memory operation statistics from OTel perspective"""
    from memctrl.otel_exporter import MemoryOTelExporter

    exporter = MemoryOTelExporter(service_name="memctrl-cli")
    exporter.start()
    store = _get_store()

    # Record spans for current state
    memories = store.list_memories()
    for mem in memories:
        exporter.record_store(
            memory_id=mem.id,
            layer=mem.layer,
            content=mem.content,
            confidence=mem.confidence,
            duration_ms=0.0,
        )

    stats = exporter.get_stats()
    exporter.stop()

    console.print(
        Panel("[bold]OpenTelemetry Memory Statistics[/bold]", border_style="cyan")
    )
    console.print(f"  Total spans: [bold]{stats['total_spans']}[/bold]")
    console.print(f"  Total duration: [bold]{stats['total_duration_ms']} ms[/bold]")
    console.print(f"  Avg duration: [bold]{stats['avg_duration_ms']} ms[/bold]")
    console.print(
        f"  Errors: [bold]{stats['error_count']}[/bold] "
        f"([bold]{stats['error_rate'] * 100:.1f}%[/bold])"
    )

    if stats["by_operation"]:
        console.print("\n[bold]By Operation:[/bold]")
        for op, count in sorted(stats["by_operation"].items()):
            console.print(f"  {op:12} {count:4d}")

    if stats["by_layer"]:
        console.print("\n[bold]By Layer:[/bold]")
        for layer, count in sorted(stats["by_layer"].items()):
            console.print(f"  {layer:12} {count:4d}")

    console.print(f"\n[dim]Trace ID: {exporter._trace_id}[/dim]")


def _default_memoryrc() -> str:
    return """# MemCtrl configuration

[memctrl]
db_path = ".memctrl/memories.db"

[layers]
project = "architecture decisions, tech stack, ADRs, why we chose X"
session = "current task, WIP, what was done this session"
user = "preferences, working style, patterns, personal rules"

[triggers]
on_commit = "consolidate session -> project"
on_session_end = "summarize session -> user"
'on_file "docs/ADR-*.md"' = "extract -> project"
'on_file "*.md"' = "extract -> project if contains decision"

[forget]
never = ["passwords", "keys", "PII", "secrets"]
after_days = { session = 7, user = 90 }

[extract]
confidence = { explicit = 1.0, inferred = 0.7, mentioned = 0.5 }
"""
