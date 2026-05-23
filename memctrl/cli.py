"""MemCtrl — Typer CLI with rich formatting.

Commands:
    install, init, add, query, list, tree, forget, clear,
    trigger, audit, serve, --version

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
    """Get MemoryStore instance with default DB path."""
    from memctrl.store import MemoryStore
    db_path = os.environ.get("MEMCTRL_DB_PATH")
    return MemoryStore(db_path)


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
    project: bool = typer.Option(False, help="Install at project level (.claude/ etc.)"),
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
    """Create .memoryrc in current directory"""
    dest = Path(".memoryrc")
    if dest.exists() and not force:
        console.print(f"[yellow]{dest} already exists. Use --force to overwrite.[/yellow]")
        raise typer.Exit(1)

    example = Path(__file__).parent / ".memoryrc.example"
    if example.exists():
        content = example.read_text()
    else:
        content = _default_memoryrc()

    dest.write_text(content)
    console.print(f"[green]Created {dest}[/green]")


@app.command()
def add(
    content: str = typer.Argument(..., help="Memory content to store"),
    layer: str = typer.Option("session", help="Layer: project/session/user"),
    source: str = typer.Option("manual", help="Source of this memory"),
):
    """Manually add a memory"""
    store = _get_store()
    mid = store.insert_memory(layer=layer, content=content, source=source)
    console.print(f"[green]Added memory[/green] [dim]{mid}[/dim] to [bold]{layer}[/bold]")


@app.command()
def query(
    query_text: str = typer.Argument(..., help="Query to search memory"),
    layer: Optional[str] = typer.Option(None, help="Filter by layer"),
):
    """Retrieve relevant memories with reasoning trace"""
    store = _get_store()
    engine = _get_engine()
    rules = engine.load()

    memories = store.list_memories(layer=layer)
    if not memories:
        console.print("[yellow]No memories found.[/yellow]")
        return

    mem_dicts = [m.to_dict() for m in memories]
    memory_lookup = {m.id: m.to_dict() for m in memories}

    # Build tree
    from memctrl.tree import MemoryTreeBuilder
    builder = MemoryTreeBuilder()

    async def _do_query():
        tree = await builder.build_tree(mem_dicts)
        tree_dict = tree.to_dict()

        # Retrieve
        from memctrl.retriever import MemoryRetriever
        retriever = MemoryRetriever()
        result = await retriever.retrieve(query_text, tree_dict, memory_lookup=memory_lookup)
        return result

    result = asyncio.run(_do_query())

    if result.facts:
        console.print(Panel(f"[bold]Query:[/bold] {query_text}", title="memctrl"))
        console.print(f"\n[bold green]Facts:[/bold green]")
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
def tree():
    """Display memory tree (rich formatted)"""
    store = _get_store()
    memories = store.list_memories()

    if not memories:
        console.print("[yellow]No memories to display.[/yellow]")
        return

    from memctrl.tree import MemoryTreeBuilder
    builder = MemoryTreeBuilder()

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
    if store.delete_memory(memory_id):
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

    if layer:
        for mem in memories:
            store.delete_memory(mem.id)
    else:
        for mem in memories:
            store.delete_memory(mem.id)

    console.print(f"[green]Cleared {count} memories.[/green]")


@app.command()
def trigger_cmd(
    event: str = typer.Argument(..., help="Event name (e.g., on_session_end)"),
    context: Optional[str] = typer.Option(None, help="JSON context string"),
):
    """Manually fire a trigger"""
    store = _get_store()
    engine = _get_engine()
    rules = engine.load()

    ctx = {}
    if context:
        try:
            ctx = json.loads(context)
        except json.JSONDecodeError:
            console.print("[red]Invalid JSON context[/red]")
            return

    ids = engine.fire_trigger(event, ctx, store)
    console.print(f"[green]Trigger '{event}' fired[/green] - {len(ids)} memories affected")


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
        color = "green" if layer == "project" else "yellow" if layer == "session" else "blue"
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
        events.append({
            "ts": mem.created_at,
            "type": "memory",
            "layer": mem.layer,
            "content": mem.content[:60],
            "icon": "[mem]",
        })
    for log in logs:
        events.append({
            "ts": log.timestamp,
            "type": "trigger",
            "layer": "",
            "content": f"{log.event}: {log.action}",
            "icon": "[refl]",
        })

    events.sort(key=lambda x: x["ts"], reverse=True)

    for ev in events[:limit]:
        ts = ev["ts"].strftime("%Y-%m-%d %H:%M") if ev["ts"] else "?"
        if ev["type"] == "memory":
            color = "green" if ev["layer"] == "project" else "yellow" if ev["layer"] == "session" else "blue"
            console.print(f"  [dim]{ts}[/dim] [{color}]{ev['icon']}[/{color}] ({ev['layer']}) {ev['content']}")
        else:
            console.print(f"  [dim]{ts}[/dim] [magenta]{ev['icon']}[/magenta] {ev['content']}")


@app.command()
def serve(
    port: int = typer.Option(8080, help="Port to run MCP server on"),
    host: str = typer.Option("127.0.0.1", help="Host to bind to"),
):
    """Start MCP server"""
    console.print(f"[green]Starting MCP server on {host}:{port}[/green]")
    console.print("[dim]Use Ctrl+C to stop[/dim]")

    from memctrl.mcp_server import serve_mcp
    asyncio.run(serve_mcp(host=host, port=port))


# ---------------------------------------------------------------------------
# Default .memoryrc content
# ---------------------------------------------------------------------------

def _default_memoryrc() -> str:
    return '''# MemCtrl configuration

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
'''
