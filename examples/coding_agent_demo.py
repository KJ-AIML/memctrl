"""MemCtrl Demo: AI Coding Agent with Persistent Memory

Simulates a coding assistant working across multiple sessions to demonstrate:
- Project layer (permanent architectural decisions)
- Session layer (daily WIP, auto-expiring)
- Automatic consolidation (session -> project)
- Tree-based retrieval with reasoning traces
- Security redaction (secrets never stored)

Run: python examples/coding_agent_demo.py
"""

from __future__ import annotations

import os
import tempfile
from datetime import datetime, timedelta
from pathlib import Path

from rich.console import Console
from rich.panel import Panel
from rich.rule import Rule

from memctrl.store import MemoryStore
from memctrl.rules import RuleEngine
from memctrl.retriever import MemoryRetriever
from memctrl.tree import MemoryTreeBuilder

console = Console()


def section(title: str) -> None:
    console.print(Rule(title, style="bold magenta"))


def agent_thought(message: str) -> None:
    console.print(Panel(message, title="[Agent]", border_style="blue"))


def main() -> None:
    # Use a temporary DB so we don't pollute the user's real memory
    tmpdir = tempfile.mkdtemp(prefix="memctrl_demo_")
    db_path = Path(tmpdir) / "demo.db"
    os.environ["MEMCTRL_DB_PATH"] = str(db_path)

    console.print(
        Panel.fit(
            "[bold]MemCtrl Demo[/bold]\n"
            "Cognitive Memory Runtime for AI Agents\n"
            f"Temp DB: {db_path}",
            border_style="green",
        )
    )

    store = MemoryStore(str(db_path))
    engine = RuleEngine()
    retriever = MemoryRetriever()
    builder = MemoryTreeBuilder()

    # -----------------------------------------------------------------------
    # Session 1: Project kickoff
    # -----------------------------------------------------------------------
    section("SESSION 1: Project Kickoff")
    agent_thought("Starting a new project. I need to remember my tech stack forever.")

    store.insert_memory(
        layer="project",
        content="Tech stack: FastAPI + PostgreSQL + Redis + Docker",
        source="arch_decision",
        confidence=1.0,
        tags=["tech_stack", "backend"],
    )
    store.insert_memory(
        layer="project",
        content="Use JWT auth with refresh tokens. Access token 15min, refresh 7 days.",
        source="adr-001",
        confidence=1.0,
        tags=["auth", "security"],
    )
    store.insert_memory(
        layer="project",
        content="API_KEY=sk-live-51fake... (DO NOT COMMIT)",
        source="env_file",
        confidence=1.0,
        tags=["secret"],
    )

    console.print("[dim]-> Stored 3 memories. The API key will be redacted by security scan.[/dim]")
    console.print()

    # -----------------------------------------------------------------------
    # Session 2: Daily work
    # -----------------------------------------------------------------------
    section("SESSION 2: Daily Development")
    agent_thought("Working on the auth module today. Debugging a CORS issue.")

    store.insert_memory(
        layer="session",
        content="Fixing CORS preflight bug on /login endpoint. Need to allow credentials.",
        source="debug_session",
        confidence=0.9,
        tags=["bugfix", "cors"],
    )
    store.insert_memory(
        layer="session",
        content="Refactored user service into repository pattern. Tests passing.",
        source="refactor",
        confidence=0.8,
        tags=["refactor", "testing"],
    )

    console.print("[dim]-> Stored 2 session memories (expire in 7 days by default).[/dim]")
    console.print()

    # -----------------------------------------------------------------------
    # Session 3: Days later — agent asks a question
    # -----------------------------------------------------------------------
    section("SESSION 3: Retrieval with Reasoning Trace")
    agent_thought("I need to know what auth method we chose. I don't remember...")

    memories = [m.to_dict() for m in store.list_memories()]
    import asyncio

    tree = asyncio.run(builder.build_tree(memories))
    tree_dict = tree.to_dict() if tree else {}
    memory_lookup = {m["id"]: m for m in memories}
    result = asyncio.run(retriever.retrieve("what auth method do we use?", tree_dict, memory_lookup=memory_lookup))

    console.print("[bold green]Query:[/bold green] what auth method do we use?")
    console.print()
    if result.facts:
        console.print("[bold]Facts found:[/bold]")
        for i, fact in enumerate(result.facts, 1):
            console.print(f"  {i}. {fact}")
        console.print()
        console.print(f"[bold blue]Trace:[/bold blue] {' -> '.join(result.trace)}")
        console.print(f"[bold]Confidence:[/bold] {result.confidence:.2f}")
    else:
        console.print("[yellow]No relevant memories found.[/yellow]")
    console.print()

    # -----------------------------------------------------------------------
    # Show the memory tree
    # -----------------------------------------------------------------------
    section("Memory Tree Visualization")
    console.print("[dim]Hierarchical view of how memories are organized:[/dim]")
    console.print()

    def print_tree(node, indent: int = 0) -> None:
        prefix = "    " * indent
        if node.is_leaf():
            console.print(f"{prefix}[dim][mem] {node.title}[/dim]")
        else:
            console.print(f"{prefix}[bold][dir] {node.title}[/bold]")
        for child in node.children:
            print_tree(child, indent + 1)

    if tree:
        console.print("[bold]Memory Tree[/bold]")
        print_tree(tree)
    console.print()

    # -----------------------------------------------------------------------
    # Trigger consolidation (session -> project)
    # -----------------------------------------------------------------------
    section("Automatic Consolidation")
    agent_thought("End of sprint. Let me consolidate my session notes into project knowledge.")

    ids = engine.fire_trigger("on_commit", {}, store)
    console.print(f"[green]Trigger 'on_commit' fired[/green] - {len(ids)} memories consolidated from session -> project")
    console.print()

    # -----------------------------------------------------------------------
    # Audit log
    # -----------------------------------------------------------------------
    section("Audit Log")
    logs = store.get_trigger_log(limit=5)
    for log in logs:
        ts = log.timestamp.strftime("%Y-%m-%d %H:%M") if log.timestamp else "?"
        console.print(f"  {ts} | {log.event} | {log.action} | {len(log.memories_affected)} memories")
    console.print()

    # -----------------------------------------------------------------------
    # Security check
    # -----------------------------------------------------------------------
    section("Security Scan")
    all_memories = store.list_memories()
    secret_count = sum(1 for m in all_memories if "REDACTED" in m.content or "API_KEY" in m.content)
    if secret_count == 0:
        console.print("[green]+ No raw secrets found in memory store.[/green]")
    else:
        console.print(f"[yellow]! Found {secret_count} memories referencing secrets.[/yellow]")
    console.print()

    # -----------------------------------------------------------------------
    # Cleanup
    # -----------------------------------------------------------------------
    section("Cleanup")
    console.print(f"[dim]Removing temp database: {db_path}[/dim]")
    del os.environ["MEMCTRL_DB_PATH"]

    console.print()
    console.print(
        Panel(
            "[bold]Demo Complete[/bold]\n"
            "MemCtrl gave the agent:\n"
            "- Permanent project memory (tech stack, ADRs)\n"
            "- Ephemeral session memory (daily WIP)\n"
            "- Automatic consolidation (session -> project)\n"
            "- Explainable retrieval (every answer shows its trace)\n"
            "- Security by default (secrets redacted before storage)",
            border_style="green",
        )
    )


if __name__ == "__main__":
    main()
