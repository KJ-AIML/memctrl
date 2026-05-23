"""MemCtrl — PageIndex-style reasoning-based retrieval.

Uses LLM to traverse memory tree (titles + summaries only) rather than
vector similarity. Returns facts WITH reasoning trace.

Research: PageIndex (VectifyAI) achieves 98.7% accuracy on FinanceBench
by replacing vector search with LLM tree traversal. Each retrieval:
  1. Scan tree structure (titles + summaries)
  2. LLM reasons which branches are relevant
  3. Traverse selected branches
  4. Return facts + full trace
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Callable, Coroutine, Dict, List, Optional

# Type alias
LLMCallable = Callable[[str, bool], Coroutine[Any, Any, str]]


@dataclass
class RetrievalResult:
    """Result of a memory retrieval with reasoning trace."""

    facts: List[str] = field(default_factory=list)
    trace: List[str] = field(default_factory=list)
    confidence: float = 0.0
    sources: List[str] = field(default_factory=list)

    def to_dict(self) -> dict:
        return {
            "facts": self.facts,
            "trace": self.trace,
            "confidence": self.confidence,
            "sources": self.sources,
        }


class MemoryRetriever:
    """PageIndex-style tree traversal for memory retrieval.

    Algorithm:
        1. Strip leaf memory text from tree (keep structure only)
        2. LLM reads tree titles/summaries, decides which branches relevant
        3. Traverse into selected branches
        4. Collect memory facts from leaves
        5. Return facts + trace showing path taken
    """

    def __init__(self, llm_client: Optional[LLMCallable] = None):
        self.llm_client = llm_client

    # --- Public API ---

    async def retrieve(
        self,
        query: str,
        tree: dict,
        top_k: int = 5,
        memory_lookup: Optional[Dict[str, dict]] = None,
    ) -> RetrievalResult:
        """Retrieve relevant memories with reasoning trace.

        query: natural language question
        tree: TreeNode serialized as dict (from MemoryTreeBuilder.to_dict)
        memory_lookup: dict of memory_id -> memory dict for content lookup
        top_k: maximum number of facts to return

        Returns RetrievalResult with facts, trace, confidence, sources.
        """
        if not tree or not memory_lookup:
            return RetrievalResult(facts=[], trace=["empty_tree"], confidence=0.0)

        if self.llm_client:
            return await self._llm_retrieve(query, tree, memory_lookup, top_k)
        return self._keyword_retrieve(query, tree, memory_lookup, top_k)

    # --- LLM-based retrieval ---

    async def _llm_retrieve(
        self,
        query: str,
        tree: dict,
        memory_lookup: Dict[str, dict],
        top_k: int,
    ) -> RetrievalResult:
        # 1. Strip leaf content, keep structure
        stripped = self._strip_leaves(tree)

        # 2. Build prompt and ask LLM
        prompt = self._build_retrieval_prompt(query, stripped)

        try:
            response = await self.llm_client(prompt, json_mode=True)
            parsed = json.loads(response)
        except Exception:
            # Fall back to keyword search on any error
            return self._keyword_retrieve(query, tree, memory_lookup, top_k)

        relevant_node_ids = parsed.get("relevant_nodes", [])
        thinking = parsed.get("thinking", "")
        confidence = parsed.get("confidence", 0.8)

        if not relevant_node_ids:
            return self._keyword_retrieve(query, tree, memory_lookup, top_k)

        # 3. Collect memories from selected nodes
        facts, sources = self._collect_from_nodes(
            relevant_node_ids, tree, memory_lookup
        )

        # 4. Build trace
        trace = ["root"]
        for nid in relevant_node_ids[:3]:  # Top 3 nodes in trace
            node = self._find_node(tree, nid)
            if node:
                trace.append(node.get("title", nid))

        # Limit to top_k
        facts = facts[:top_k]
        sources = sources[:top_k]

        return RetrievalResult(
            facts=facts,
            trace=trace,
            confidence=confidence,
            sources=sources,
        )

    def _strip_leaves(self, tree: dict) -> dict:
        """Remove full memory content, keep structure for LLM."""
        result = {
            "id": tree.get("id", ""),
            "title": tree.get("title", ""),
            "layer": tree.get("layer", ""),
            "summary": tree.get("summary", ""),
            "memory_count": len(tree.get("memory_ids", [])),
            "children": [self._strip_leaves(c) for c in tree.get("children", [])],
        }
        return result

    def _build_retrieval_prompt(self, query: str, stripped_tree: dict) -> str:
        """Build LLM prompt for tree-based retrieval."""
        tree_json = json.dumps(stripped_tree, indent=2)
        return (
            "You are a memory retrieval expert. Given a user query and a "
            "hierarchical memory tree, identify which tree nodes are most "
            "likely to contain relevant information.\n\n"
            f"Query: {query}\n\n"
            "Memory Tree Structure:\n"
            f"{tree_json}\n\n"
            "Return ONLY JSON in this exact format:\n"
            '{\n'
            '  "thinking": "reason about which branches are relevant",\n'
            '  "relevant_nodes": ["node_id_1", "node_id_2"],\n'
            '  "confidence": 0.9\n'
            '}'
        )

    def _collect_from_nodes(
        self,
        node_ids: List[str],
        tree: dict,
        memory_lookup: Dict[str, dict],
    ) -> tuple[List[str], List[str]]:
        """Collect facts and sources from specified tree nodes."""
        facts: List[str] = []
        sources: List[str] = []

        for nid in node_ids:
            node = self._find_node(tree, nid)
            if not node:
                continue

            # If node has direct memory_ids, look them up
            for mid in node.get("memory_ids", []):
                mem = memory_lookup.get(mid)
                if mem and mem.get("content"):
                    facts.append(mem["content"])
                    sources.append(mem.get("source", "unknown"))

            # Also check children recursively
            child_facts, child_sources = self._collect_from_nodes(
                [c["id"] for c in node.get("children", [])],
                node, memory_lookup,
            )
            facts.extend(child_facts)
            sources.extend(child_sources)

        return facts, sources

    def _find_node(self, tree: dict, node_id: str) -> Optional[dict]:
        if tree.get("id") == node_id:
            return tree
        for child in tree.get("children", []):
            found = self._find_node(child, node_id)
            if found:
                return found
        return None

    # --- Keyword fallback (no LLM) ---

    def _keyword_retrieve(
        self,
        query: str,
        tree: dict,
        memory_lookup: Dict[str, dict],
        top_k: int,
    ) -> RetrievalResult:
        """Fallback: score nodes by keyword matching."""
        query_words = set(re.findall(r"\b\w{3,}\b", query.lower()))
        if not query_words:
            return RetrievalResult(facts=[], trace=["no_keywords"], confidence=0.0)

        scored_memories: Dict[str, tuple[float, str, str]] = {}  # mem_id -> (score, content, source)

        def score_node(node: dict, depth: int = 0):
            node_title = node.get("title", "").lower()
            node_summary = node.get("summary", "").lower()

            title_score = sum(1 for w in query_words if w in node_title) * 3
            summary_score = sum(1 for w in query_words if w in node_summary) * 2

            for mid in node.get("memory_ids", []):
                mem = memory_lookup.get(mid)
                if not mem:
                    continue
                content = mem.get("content", "").lower()
                content_score = sum(1 for w in query_words if w in content)
                total = title_score + summary_score + content_score + (1.0 / (depth + 1))
                if total > 0:
                    existing = scored_memories.get(mid, (0, "", ""))
                    if total > existing[0]:
                        scored_memories[mid] = (total, mem["content"], mem.get("source", ""))

            for child in node.get("children", []):
                score_node(child, depth + 1)

        score_node(tree)

        sorted_mems = sorted(scored_memories.values(), reverse=True)
        top = sorted_mems[:top_k]

        if not top:
            return RetrievalResult(
                facts=[], trace=["root", "no_match"], confidence=0.0
            )

        facts = [s[1] for s in top]
        sources = [s[2] for s in top]
        avg_score = sum(s[0] for s in top) / len(top)
        confidence = min(avg_score / 10, 1.0)  # Normalize

        # Build simple trace from matched content
        trace = ["root", "keyword_search"]
        if facts:
            trace.append(facts[0][:30])

        return RetrievalResult(
            facts=facts, trace=trace, confidence=round(confidence, 2),
            sources=sources,
        )
