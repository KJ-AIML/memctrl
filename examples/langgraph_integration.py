"""MemCtrl + LangGraph Integration Example

Shows how to use MemCtrl as persistent memory for LangGraph agents.

Install dependencies:
    pip install "memctrl[langgraph]"

Run:
    python examples/langgraph_integration.py
"""

from __future__ import annotations

import tempfile
from pathlib import Path

from memctrl.integrations.langgraph import MemCtrlMemory, MemoryNode


def demo_memory_manager():
    """Demo: Use MemCtrlMemory directly inside any LangGraph node."""
    print("=" * 60)
    print("Demo 1: MemCtrlMemory Manager")
    print("=" * 60)

    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = Path(tmpdir) / "langgraph_demo.db"
        memory = MemCtrlMemory(str(db_path))

        # Agent remembers things during work
        memory.remember(
            content="User prefers dark mode in all tools",
            layer="user",
            tags=["preference", "ui"],
        )
        memory.remember(
            content="Tech stack: FastAPI + PostgreSQL + Redis",
            layer="project",
            tags=["tech_stack", "backend"],
        )
        memory.remember(
            content="Fixed CORS bug on /login endpoint",
            layer="session",
            tags=["bugfix", "cors"],
        )

        # Later, agent recalls relevant knowledge
        print("\nQuery: what is our tech stack?")
        facts = memory.recall("what is our tech stack?")
        for fact in facts:
            print(f"  -> {fact}")

        print("\nQuery: what auth bugs have we fixed?")
        facts = memory.recall("what auth bugs have we fixed?")
        for fact in facts:
            print(f"  -> {fact}")

        # Consolidate session -> project
        print("\nConsolidating session memories...")
        affected = memory.consolidate("on_commit")
        print(f"  Consolidated {len(affected)} memories")

        stats = memory.get_stats()
        print(f"\nTotal memories: {stats.get('memories', 0)}")


def demo_memory_node():
    """Demo: Use MemoryNode as a LangGraph node."""
    print("\n" + "=" * 60)
    print("Demo 2: MemoryNode (LangGraph Node)")
    print("=" * 60)

    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = Path(tmpdir) / "langgraph_node.db"
        node = MemoryNode(str(db_path), auto_extract=True)

        # Simulate LangGraph state
        state = {
            "messages": [
                {
                    "role": "assistant",
                    "content": "We decided to use FastAPI with PostgreSQL for the backend.",
                },
                {"role": "user", "content": "What database should we use?"},
            ],
            "memory_query": "what did we decide about the backend?",
        }

        # Process through memory node
        new_state = node(state)

        print(f"\nInput query: {state['memory_query']}")
        print(f"Retrieved facts: {new_state.get('memory_facts', [])}")
        print(f"Reasoning trace: {new_state.get('memory_trace', [])}")
        print(f"Confidence: {new_state.get('memory_confidence', 0):.2f}")

        # Add consolidation
        state2 = {
            "messages": [],
            "memory_consolidate": True,
        }
        new_state2 = node(state2)
        print(f"\nConsolidated memories: {new_state2.get('memory_consolidated', [])}")


def main():
    print("MemCtrl + LangGraph Integration Examples\n")
    demo_memory_manager()
    demo_memory_node()
    print("\n" + "=" * 60)
    print("Done! MemCtrl gives LangGraph agents persistent memory.")
    print("=" * 60)


if __name__ == "__main__":
    main()
