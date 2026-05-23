"""MemCtrl — LangGraph integration.

Provides checkpoint-style persistence and memory nodes for LangGraph agents.

Usage:
    from memctrl.integrations.langgraph import MemoryNode, MemCtrlMemory

    # As a LangGraph node
    workflow.add_node("memory", MemoryNode())
    workflow.add_edge("agent", "memory")

    # As a memory manager inside any node
    memory = MemCtrlMemory()
    memory.remember("user prefers dark mode", layer="user")
    facts = memory.recall("what does the user prefer?")
"""

from __future__ import annotations

import asyncio
import json
from typing import Any, Dict, List, Optional

from memctrl.store import MemoryStore
from memctrl.tree import MemoryTreeBuilder
from memctrl.retriever import MemoryRetriever
from memctrl.rules import RuleEngine

# Optional LangGraph import with graceful degradation
try:
    from langgraph.checkpoint.base import BaseCheckpointSaver
    from langgraph.types import StateSnapshot

    LANGGRAPH_AVAILABLE = True
except ImportError:
    BaseCheckpointSaver = object
    StateSnapshot = Any
    LANGGRAPH_AVAILABLE = False


class MemCtrlMemory:
    """High-level memory manager for LangGraph agents.

    Wraps MemoryStore with async-friendly methods designed for agent nodes.
    """

    def __init__(self, db_path: Optional[str] = None):
        self.store = MemoryStore(db_path)
        self.builder = MemoryTreeBuilder()
        self.retriever = MemoryRetriever()
        self.engine = RuleEngine()

    def remember(
        self,
        content: str,
        layer: str = "session",
        source: str = "langgraph",
        confidence: float = 1.0,
        tags: Optional[List[str]] = None,
    ) -> str:
        """Store a memory fact. Returns memory ID."""
        return self.store.insert_memory(
            layer=layer,
            content=content,
            source=source,
            confidence=confidence,
            tags=tags or [],
        )

    def recall(self, query: str, top_k: int = 5) -> List[str]:
        """Retrieve relevant memory facts with reasoning trace."""
        memories = [m.to_dict() for m in self.store.list_memories()]
        if not memories:
            return []

        tree = asyncio.run(self.builder.build_tree(memories))
        tree_dict = tree.to_dict() if tree else {}
        memory_lookup = {m["id"]: m for m in memories}

        result = asyncio.run(
            self.retriever.retrieve(
                query, tree_dict, top_k=top_k, memory_lookup=memory_lookup
            )
        )
        return result.facts

    def recall_with_trace(self, query: str, top_k: int = 5) -> Dict[str, Any]:
        """Retrieve memories with full trace and metadata."""
        memories = [m.to_dict() for m in self.store.list_memories()]
        if not memories:
            return {"facts": [], "trace": ["empty"], "confidence": 0.0}

        tree = asyncio.run(self.builder.build_tree(memories))
        tree_dict = tree.to_dict() if tree else {}
        memory_lookup = {m["id"]: m for m in memories}

        result = asyncio.run(
            self.retriever.retrieve(
                query, tree_dict, top_k=top_k, memory_lookup=memory_lookup
            )
        )
        return {
            "facts": result.facts,
            "trace": result.trace,
            "confidence": result.confidence,
        }

    def consolidate(
        self, event: str = "on_commit", context: Optional[Dict] = None
    ) -> List[str]:
        """Fire a trigger rule to consolidate memories."""
        return self.engine.fire_trigger(event, context or {}, self.store)

    def get_stats(self) -> Dict[str, Any]:
        """Get memory store statistics."""
        return self.store.stats()


class MemoryNode:
    """LangGraph node that adds persistent memory capabilities.

    Expects state dict with at least:
        - "messages": list of message dicts (optional, for auto-extraction)
        - "memory_query": str (optional, for explicit recall)
        - "memory_facts": list (output, populated by this node)

    Usage:
        workflow.add_node("memory", MemoryNode())
        workflow.add_edge("agent", "memory")
        workflow.add_edge("memory", END)
    """

    def __init__(self, db_path: Optional[str] = None, auto_extract: bool = True):
        self.memory = MemCtrlMemory(db_path)
        self.auto_extract = auto_extract

    def __call__(self, state: Dict[str, Any]) -> Dict[str, Any]:
        """Process state: extract memories, answer queries, return enriched state."""
        new_state = dict(state)

        # Auto-extract from latest message if enabled
        if self.auto_extract and "messages" in state:
            messages = state["messages"]
            if messages:
                latest = messages[-1]
                content = (
                    latest.get("content", "")
                    if isinstance(latest, dict)
                    else str(latest)
                )
                if len(content) > 20:
                    self.memory.remember(
                        content=content[:500],
                        layer="session",
                        source="langgraph_conversation",
                        confidence=0.7,
                    )

        # Handle explicit memory queries
        query = state.get("memory_query", "")
        if query:
            result = self.memory.recall_with_trace(query)
            new_state["memory_facts"] = result["facts"]
            new_state["memory_trace"] = result["trace"]
            new_state["memory_confidence"] = result["confidence"]
        else:
            new_state.setdefault("memory_facts", [])
            new_state.setdefault("memory_trace", [])
            new_state.setdefault("memory_confidence", 0.0)

        # Run consolidation if requested
        if state.get("memory_consolidate"):
            affected = self.memory.consolidate()
            new_state["memory_consolidated"] = affected

        return new_state


class MemCtrlSaver(BaseCheckpointSaver):
    """LangGraph checkpoint saver backed by MemCtrl.

    Uses MemoryStore to persist agent state across runs.
    Provides hierarchical memory + traceability for every checkpoint.

    Usage:
        from langgraph.graph import StateGraph
        from memctrl.integrations.langgraph import MemCtrlSaver

        checkpointer = MemCtrlSaver()
        app = workflow.compile(checkpointer=checkpointer)
    """

    def __init__(self, db_path: Optional[str] = None):
        if not LANGGRAPH_AVAILABLE:
            raise ImportError(
                "LangGraph is required for MemCtrlSaver. "
                'Install with: pip install "memctrl[langgraph]"'
            )
        super().__init__()
        self.store = MemoryStore(db_path)

    def get_tuple(self, config: Dict[str, Any]) -> Optional[StateSnapshot]:
        """Retrieve checkpoint by thread ID."""
        thread_id = config.get("configurable", {}).get("thread_id", "default")
        mem = self.store.get_memory(f"checkpoint:{thread_id}")
        if not mem:
            return None
        try:
            data = json.loads(mem.content)
            return StateSnapshot(
                values=data.get("values", {}),
                next=data.get("next", []),
                config=config,
                metadata=data.get("metadata", {}),
                created_at=mem.created_at,
                parent_config=data.get("parent_config"),
                tasks=data.get("tasks", []),
            )
        except Exception:
            return None

    def put(
        self,
        config: Dict[str, Any],
        checkpoint: Dict[str, Any],
        metadata: Dict[str, Any],
        new_versions: Any,
    ) -> Dict[str, Any]:
        """Store checkpoint."""
        thread_id = config.get("configurable", {}).get("thread_id", "default")
        data = {
            "values": checkpoint.get("values", {}),
            "next": checkpoint.get("next", []),
            "metadata": metadata,
            "parent_config": checkpoint.get("parent_config"),
            "tasks": checkpoint.get("tasks", []),
        }
        # Upsert: delete old then insert
        self.store.delete_memory(f"checkpoint:{thread_id}")
        self.store.insert_memory(
            layer="session",
            content=json.dumps(data),
            source=f"checkpoint:{thread_id}",
            confidence=1.0,
            tags=["langgraph", "checkpoint", thread_id],
        )
        return config

    def list(
        self,
        config: Optional[Dict[str, Any]],
        *,
        before: Optional[Dict[str, Any]] = None,
        limit: Optional[int] = None,
        filter: Optional[Dict[str, Any]] = None,
    ) -> List[StateSnapshot]:
        """List checkpoints (returns session-layer checkpoints)."""
        memories = self.store.list_memories("session")
        results = []
        for mem in memories:
            if not mem.source.startswith("checkpoint:"):
                continue
            try:
                data = json.loads(mem.content)
                results.append(
                    StateSnapshot(
                        values=data.get("values", {}),
                        next=data.get("next", []),
                        config=config or {},
                        metadata=data.get("metadata", {}),
                        created_at=mem.created_at,
                        parent_config=data.get("parent_config"),
                        tasks=data.get("tasks", []),
                    )
                )
            except Exception:
                continue
        if limit:
            results = results[:limit]
        return results
