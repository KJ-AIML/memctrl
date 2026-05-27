"""MemCtrl Killer Demo: "The Agent That Remembers"

Simulates a coding agent across 3 sprints to demonstrate the
"holy sh*t" moment of persistent memory:

Sprint 1: Agent builds auth, hits JWT expiry bug.
Sprint 2: Weeks later, agent starts similar work.
Sprint 3: SAME bug appears — but this time the agent REMEMBERS
           the previous failure and prevents it.

Run: python examples/killer_demo.py
"""

from __future__ import annotations

import os
import tempfile
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


def sprint(name: str) -> None:
    console.print()
    console.print(Panel(f"[bold]{name}[/bold]", border_style="blue", padding=(1, 2)))


def agent_says(message: str, emotion: str = "neutral") -> None:
    styles = {
        "neutral": "blue",
        "excited": "green",
        "worried": "yellow",
        "shocked": "red",
    }
    console.print(
        Panel(message, title="[Agent]", border_style=styles.get(emotion, "blue"))
    )


def main() -> None:
    tmpdir = tempfile.mkdtemp(prefix="memctrl_killer_")
    db_path = Path(tmpdir) / "demo.db"
    os.environ["MEMCTRL_DB_PATH"] = str(db_path)

    console.print(
        Panel.fit(
            "[bold]MemCtrl Killer Demo[/bold]\n"
            "The Agent That Remembers\n"
            "---\n"
            "Watch an AI coding agent learn from its mistakes\n"
            "across multiple sprints using persistent memory.",
            border_style="green",
        )
    )

    store = MemoryStore(str(db_path))
    engine = RuleEngine()
    retriever = MemoryRetriever()
    builder = MemoryTreeBuilder()
    import asyncio

    # ===================================================================
    # SPRINT 1: Building auth from scratch
    # ===================================================================
    sprint("SPRINT 1: Building Authentication")
    agent_says(
        "I'm implementing JWT authentication for the API. "
        "Access tokens expire in 15 minutes, refresh tokens in 7 days.",
        "neutral",
    )

    store.insert_memory(
        layer="project",
        content="JWT auth: access token 15min, refresh token 7 days. Use httponly cookies for refresh.",
        source="arch_decision",
        confidence=1.0,
        tags=["auth", "jwt", "security"],
    )
    store.insert_memory(
        layer="project",
        content="User model fields: id, email, hashed_password, refresh_token, created_at",
        source="schema",
        confidence=1.0,
        tags=["database", "users"],
    )

    console.print(
        "[dim]-> Stored architectural decisions to project memory (permanent)[/dim]"
    )

    # Bug happens
    agent_says(
        "PRODUCTION BUG REPORTED: Users logged out after 15 minutes even though "
        "they were actively using the app. The refresh token endpoint was returning "
        "401 because the access token expiry was checked BEFORE validating the refresh token.",
        "worried",
    )

    store.insert_memory(
        layer="session",
        content="BUG: Access token expiry checked before refresh token validation. "
        "Fixed by reordering middleware: validate refresh FIRST, then check access expiry.",
        source="incident",
        confidence=1.0,
        tags=["bug", "jwt", "middleware", "critical"],
    )
    store.insert_memory(
        layer="session",
        content="Root cause: middleware stack order. AuthMiddleware line 47 checks expiry "
        "before RefreshMiddleware can swap tokens.",
        source="postmortem",
        confidence=0.9,
        tags=["bug", "root_cause", "middleware"],
    )

    console.print("[dim]-> Stored incident details to session memory[/dim]")

    # End of sprint — consolidate
    console.print()
    console.print("[yellow]End of Sprint 1: Running consolidation...[/yellow]")
    ids = engine.fire_trigger("on_commit", {}, store)
    console.print(
        f"[green]Consolidated {len(ids)} session memories into project knowledge[/green]"
    )

    # ===================================================================
    # SPRINT 2: New feature, different context (weeks later)
    # ===================================================================
    sprint("SPRINT 2: Building OAuth Integration (3 weeks later)")
    agent_says(
        "Now I'm adding Google OAuth login. I need to handle token exchange "
        "and session management. Let me check our auth patterns...",
        "neutral",
    )

    # Agent queries memory
    memories = [m.to_dict() for m in store.list_memories()]
    tree = asyncio.run(builder.build_tree(memories))
    tree_dict = tree.to_dict() if tree else {}
    memory_lookup = {m["id"]: m for m in memories}

    result = asyncio.run(
        retriever.retrieve(
            "what auth patterns and past bugs should I know about?",
            tree_dict,
            memory_lookup=memory_lookup,
        )
    )

    console.print(
        "[bold]Memory query:[/bold] what auth patterns and past bugs should I know about?"
    )
    if result.facts:
        console.print("[bold green]Retrieved facts:[/bold green]")
        for fact in result.facts[:3]:
            console.print(f"  - {fact}")
        console.print(f"[dim]Trace: {' -> '.join(result.trace)}[/dim]")
    console.print()

    agent_says(
        "I see -- we had a middleware ordering bug in Sprint 1 where access token expiry "
        "was checked before refresh validation. I'll make sure OAuth tokens follow the same "
        "validation order: refresh first, then access expiry.",
        "excited",
    )

    store.insert_memory(
        layer="project",
        content="OAuth integration follows same middleware order as JWT: refresh validation BEFORE access expiry check",
        source="arch_decision",
        confidence=1.0,
        tags=["oauth", "auth", "middleware", "pattern"],
    )

    # ===================================================================
    # SPRINT 3: The same bug ALMOST happens again
    # ===================================================================
    sprint("SPRINT 3: Building Admin Dashboard (6 weeks later)")
    agent_says(
        "I'm adding admin endpoints that need session verification. "
        "I was about to copy the old auth middleware pattern from another project...",
        "neutral",
    )

    # Before implementing, agent queries memory
    result2 = asyncio.run(
        retriever.retrieve(
            "middleware order for token validation",
            tree_dict,
            memory_lookup=memory_lookup,
        )
    )

    console.print("[bold]Memory query:[/bold] middleware order for token validation")
    if result2.facts:
        console.print("[bold green]Retrieved facts:[/bold green]")
        for fact in result2.facts[:2]:
            console.print(f"  - {fact}")
        console.print(f"[dim]Trace: {' -> '.join(result2.trace)}[/dim]")
    console.print()

    agent_says(
        "WAIT. I almost made the same mistake again! The old pattern checks access expiry first. "
        "But our project memory clearly shows: validate refresh token FIRST, then check access expiry. "
        "This exact bug cost us a production incident in Sprint 1. I'll use the corrected order.",
        "shocked",
    )

    console.print(
        Panel(
            "[bold green]Bug prevented![/bold green]\n"
            "The agent remembered a 6-week-old incident and avoided repeating it.\n"
            "Without MemCtrl, this would have been another production outage.",
            border_style="green",
        )
    )

    # ===================================================================
    # Final stats
    # ===================================================================
    sprint("Memory Stats")
    final_memories = store.list_memories()
    by_layer = {}
    for mem in final_memories:
        by_layer[mem.layer] = by_layer.get(mem.layer, 0) + 1
    console.print(f"Total memories: {len(final_memories)}")
    console.print(f"Project layer: {by_layer.get('project', 0)}")
    console.print(f"Session layer: {by_layer.get('session', 0)}")

    logs = store.get_trigger_log()
    console.print(f"Consolidation events: {len(logs)}")

    # Cleanup
    del os.environ["MEMCTRL_DB_PATH"]
    console.print()
    console.print(
        Panel(
            "[bold]The Difference[/bold]\n"
            "Without MemCtrl: Agent repeats the same bug every 6 weeks.\n"
            "With MemCtrl: Agent learns from experience and prevents regressions.\n\n"
            "This is not RAG. This is cognition.",
            border_style="cyan",
        )
    )


if __name__ == "__main__":
    main()
