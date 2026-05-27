"""MemCtrl Migration Demo: "Your Agent's Memory Travels With You"

Simulates a developer switching from Claude Code to Cursor and
showing how MemCtrl preserves agent memory across tools.

Run: python examples/migration_demo.py
"""

from __future__ import annotations

import os
import tempfile
from pathlib import Path

from rich.console import Console
from rich.panel import Panel
from rich.rule import Rule
from rich.table import Table

from memctrl.store import MemoryStore
from memctrl.tree import MemoryTreeBuilder
from memctrl.retriever import MemoryRetriever

console = Console()


def section(title: str) -> None:
    console.print(Rule(title, style="bold magenta"))


def agent_says(agent: str, message: str, color: str = "blue") -> None:
    console.print(
        Panel(message, title=f"[{color}]{agent}[/{color}]", border_style=color)
    )


def mcp_call(tool: str, args: dict) -> None:
    args_str = ", ".join(f"{k}={v!r}" for k, v in args.items())
    console.print(f"[dim]  -> MCP call:[/dim] [cyan]{tool}[/cyan]({args_str})")


def main() -> None:
    # Shared memory store — this is the magic. Both tools point here.
    tmpdir = tempfile.mkdtemp(prefix="memctrl_migration_")
    db_path = Path(tmpdir) / ".memctrl" / "memories.db"
    db_path.parent.mkdir(parents=True, exist_ok=True)
    os.environ["MEMCTRL_DB_PATH"] = str(db_path)

    console.print(
        Panel.fit(
            "[bold]MemCtrl Migration Demo[/bold]\n"
            "Your Agent's Memory Travels With You\n"
            "---\n"
            "Watch project context move seamlessly from Claude Code to Cursor.",
            border_style="green",
        )
    )

    store = MemoryStore(str(db_path))
    builder = MemoryTreeBuilder()
    retriever = MemoryRetriever()
    import asyncio

    # =====================================================================
    # PHASE 1: Claude Code Session
    # =====================================================================
    section("PHASE 1: Coding in Claude Code")

    agent_says(
        "Claude Code",
        "I'm reviewing the auth system. Let me store what I learn so I don't "
        "have to rediscover it in the next session.",
        "green",
    )

    # Simulate Claude Code MCP tool calls
    mcp_call(
        "memctrl_add",
        {"content": "FastAPI + PostgreSQL + Redis stack", "layer": "project"},
    )
    store.insert_memory(
        layer="project",
        content="FastAPI + PostgreSQL + Redis stack",
        source="mcp",
        confidence=1.0,
        tags=["tech_stack"],
    )

    mcp_call(
        "memctrl_add",
        {
            "content": "JWT auth: access 15min, refresh 7d, httponly cookies",
            "layer": "project",
        },
    )
    store.insert_memory(
        layer="project",
        content="JWT auth: access 15min, refresh 7d, httponly cookies",
        source="mcp",
        confidence=1.0,
        tags=["auth", "jwt"],
    )

    mcp_call(
        "memctrl_add",
        {
            "content": "BUG: middleware order caused 401 on refresh. Fix: validate refresh FIRST",
            "layer": "session",
        },
    )
    store.insert_memory(
        layer="session",
        content="BUG: middleware order caused 401 on refresh. Fix: validate refresh FIRST",
        source="mcp",
        confidence=1.0,
        tags=["bug", "critical"],
    )

    console.print("[dim]  -> 3 memories stored to .memctrl/memories.db[/dim]\n")

    agent_says(
        "Claude Code",
        "Done for today. My memories are safe in the project-local SQLite store.",
        "green",
    )

    # =====================================================================
    # PHASE 2: Developer Switches Tools
    # =====================================================================
    section("PHASE 2: Switching to Cursor")

    console.print(
        Panel(
            "[bold]Developer Action[/bold]\n"
            "Closes Claude Code. Opens Cursor.\n"
            "Cursor MCP config points to the same .memctrl/memories.db",
            border_style="yellow",
        )
    )

    # =====================================================================
    # PHASE 3: Cursor Session
    # =====================================================================
    section("PHASE 3: Coding in Cursor")

    agent_says(
        "Cursor",
        "New session. I need to add OAuth integration. Let me check what "
        "the project already knows about auth...",
        "blue",
    )

    # Simulate Cursor MCP query
    mcp_call("memctrl_query", {"query": "auth patterns and middleware order"})

    memories = [m.to_dict() for m in store.list_memories()]
    tree = asyncio.run(builder.build_tree(memories))
    tree_dict = tree.to_dict() if tree else {}
    memory_lookup = {m["id"]: m for m in memories}

    result = asyncio.run(
        retriever.retrieve(
            "auth patterns and middleware order",
            tree_dict,
            top_k=3,
            memory_lookup=memory_lookup,
        )
    )

    console.print("[bold]Retrieved facts:[/bold]")
    for fact in result.facts:
        console.print(f"  • {fact}")
    console.print(f"[dim]  Trace: {' -> '.join(result.trace)}[/dim]\n")

    agent_says(
        "Cursor",
        "Perfect — I can see the JWT setup AND the middleware bug from last week. "
        "I'll make sure OAuth follows the same validation order. No repeated mistakes.",
        "blue",
    )

    # =====================================================================
    # PHASE 4: Comparison
    # =====================================================================
    section("The Difference")

    table = Table(title="With vs Without MemCtrl", show_lines=True)
    table.add_column("Scenario", style="cyan")
    table.add_column("Without MemCtrl")
    table.add_column("With MemCtrl")

    table.add_row(
        "Switch Claude -> Cursor",
        "Blank slate. Agent re-discovers everything.",
        "Full context. Agent recalls stored memories instantly.",
    )
    table.add_row(
        "Auth bug from last week",
        "Repeated in OAuth sprint.",
        "Prevented. Memory shows exact fix.",
    )
    table.add_row(
        "Tech stack decisions",
        "Re-explained in every tool.",
        "Stored once. Available everywhere.",
    )
    table.add_row(
        "Data location",
        "Cloud vector DB or nothing.",
        "Local SQLite. Your data, your machine.",
    )

    console.print(table)

    # =====================================================================
    # Stats
    # =====================================================================
    section("Memory Stats")
    final_memories = store.list_memories()
    by_layer = {}
    for mem in final_memories:
        by_layer[mem.layer] = by_layer.get(mem.layer, 0) + 1

    console.print(f"Total memories: {len(final_memories)}")
    console.print(f"Project layer: {by_layer.get('project', 0)} (permanent)")
    console.print(f"Session layer: {by_layer.get('session', 0)} (ephemeral)")
    console.print(f"Database: {db_path}")

    del os.environ["MEMCTRL_DB_PATH"]
    console.print()
    console.print(
        Panel(
            "[bold]The Point[/bold]\n"
            "Agent memory shouldn't be locked to one IDE.\n"
            "MemCtrl makes it local, portable, and tool-agnostic.\n\n"
            "Claude Code. Cursor. Kimi. Codex. Same memory. Same context.",
            border_style="green",
        )
    )


if __name__ == "__main__":
    main()
