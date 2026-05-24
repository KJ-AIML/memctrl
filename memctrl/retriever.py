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
from typing import Any, Callable, Coroutine, Dict, List, Optional, Tuple

from memctrl.provenance import ProvenanceTracker, RetrievalProvenance

# Type alias
LLMCallable = Callable[[str, bool], Coroutine[Any, Any, str]]


@dataclass
class RetrievalResult:
    """Result of a memory retrieval with reasoning trace."""

    facts: List[str] = field(default_factory=list)
    trace: List[str] = field(default_factory=list)
    confidence: float = 0.0
    sources: List[str] = field(default_factory=list)
    provenance: Optional[RetrievalProvenance] = None  # populated when tracker is active

    def to_dict(self) -> dict:
        result = {
            "facts": self.facts,
            "trace": self.trace,
            "confidence": self.confidence,
            "sources": self.sources,
        }
        if self.provenance is not None:
            result["provenance"] = self.provenance.to_dict()
        return result


class MemoryRetriever:
    """PageIndex-style tree traversal for memory retrieval.

    Algorithm:
        1. Strip leaf memory text from tree (keep structure only)
        2. LLM reads tree titles/summaries, decides which branches relevant
        3. Traverse into selected branches
        4. Collect memory facts from leaves
        5. Return facts + trace showing path taken
    """

    def __init__(
        self,
        llm_client: Optional[LLMCallable] = None,
        provenance_tracker: Optional[ProvenanceTracker] = None,
    ):
        self.llm_client = llm_client
        self.provenance_tracker = provenance_tracker

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
        If a ProvenanceTracker was provided at init, the result will also
        contain a provenance record.
        """
        if not tree or not memory_lookup:
            return RetrievalResult(facts=[], trace=["empty_tree"], confidence=0.0)

        if self.llm_client:
            result, matched_memories = await self._llm_retrieve_with_sources(
                query, tree, memory_lookup, top_k
            )
        else:
            result, matched_memories = self._keyword_retrieve_with_sources(
                query, tree, memory_lookup, top_k
            )

        # Record provenance if a tracker is configured.
        # WHY: Every retrieval decision should be auditable. We capture
        # the full context of what was retrieved and why immediately
        # after the retrieval completes.
        if self.provenance_tracker is not None:
            provenance = self.provenance_tracker.record_retrieval(
                query=query,
                results=matched_memories,
                method="llm" if self.llm_client else "keyword",
                tree_version=0,
                total_memories_searched=len(memory_lookup),
                trace_paths={m.get("id", ""): result.trace for m in matched_memories},
                match_reasons={
                    m.get(
                        "id", ""
                    ): f"matched via {result.trace[-1] if result.trace else 'unknown'}"
                    for m in matched_memories
                },
            )
            result.provenance = provenance

        return result

    # --- LLM-based retrieval ---

    async def _llm_retrieve(
        self,
        query: str,
        tree: dict,
        memory_lookup: Dict[str, dict],
        top_k: int,
    ) -> RetrievalResult:
        """Public-facing wrapper for LLM retrieval (returns only result)."""
        result, _ = await self._llm_retrieve_with_sources(
            query, tree, memory_lookup, top_k
        )
        return result

    async def _llm_retrieve_with_sources(
        self,
        query: str,
        tree: dict,
        memory_lookup: Dict[str, dict],
        top_k: int,
    ) -> Tuple[RetrievalResult, List[dict]]:
        """LLM retrieval that returns both the result and matched memory dicts.

        WHY: Provenance tracking needs the full memory dicts (with id,
        layer, source, confidence) — not just the content strings. This
        internal method returns both so the public retrieve() can record
        provenance without re-looking-up memories.
        """
        # 1. Strip leaf content, keep structure
        stripped = self._strip_leaves(tree)

        # 2. Build prompt and ask LLM
        prompt = self._build_retrieval_prompt(query, stripped)

        try:
            response = await self.llm_client(prompt, json_mode=True)
            parsed = json.loads(response)
        except Exception:
            # Fall back to keyword search on any error
            return self._keyword_retrieve_with_sources(
                query, tree, memory_lookup, top_k
            )

        relevant_node_ids = parsed.get("relevant_nodes", [])
        confidence = parsed.get("confidence", 0.8)

        if not relevant_node_ids:
            return self._keyword_retrieve_with_sources(
                query, tree, memory_lookup, top_k
            )

        # 3. Collect memories from selected nodes
        facts, sources, matched_memories = self._collect_from_nodes_with_memories(
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
        matched_memories = matched_memories[:top_k]

        return (
            RetrievalResult(
                facts=facts,
                trace=trace,
                confidence=confidence,
                sources=sources,
            ),
            matched_memories,
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
            "{\n"
            '  "thinking": "reason about which branches are relevant",\n'
            '  "relevant_nodes": ["node_id_1", "node_id_2"],\n'
            '  "confidence": 0.9\n'
            "}"
        )

    def _collect_from_nodes(
        self,
        node_ids: List[str],
        tree: dict,
        memory_lookup: Dict[str, dict],
    ) -> tuple[List[str], List[str]]:
        """Collect facts and sources from specified tree nodes."""
        facts, sources, _memories = self._collect_from_nodes_with_memories(
            node_ids, tree, memory_lookup
        )
        return facts, sources

    def _collect_from_nodes_with_memories(
        self,
        node_ids: List[str],
        tree: dict,
        memory_lookup: Dict[str, dict],
    ) -> Tuple[List[str], List[str], List[dict]]:
        """Collect facts, sources, AND full memory dicts from tree nodes.

        WHY: Provenance tracking requires the full memory dicts (with id,
        layer, source, confidence), not just content strings. This method
        is identical to _collect_from_nodes but also returns the memory
        dicts so provenance can be recorded without re-looking them up.
        """
        facts: List[str] = []
        sources: List[str] = []
        memories: List[dict] = []

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
                    memories.append(mem)

            # Also check children recursively
            child_facts, child_sources, child_memories = (
                self._collect_from_nodes_with_memories(
                    [c["id"] for c in node.get("children", [])],
                    node,
                    memory_lookup,
                )
            )
            facts.extend(child_facts)
            sources.extend(child_sources)
            memories.extend(child_memories)

        return facts, sources, memories

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
        """Public-facing wrapper for keyword retrieval (returns only result)."""
        result, _ = self._keyword_retrieve_with_sources(
            query, tree, memory_lookup, top_k
        )
        return result

    def _keyword_retrieve_with_sources(
        self,
        query: str,
        tree: dict,
        memory_lookup: Dict[str, dict],
        top_k: int,
    ) -> Tuple[RetrievalResult, List[dict]]:
        """Keyword retrieval that returns both the result and matched memory dicts.

        WHY: Provenance tracking needs the full memory dicts (with id,
        layer, source, confidence) -- not just the content strings. This
        internal method returns both so the public retrieve() can record
        provenance without re-looking-up memories.
        """
        query_words = set(re.findall(r"\b\w{3,}\b", query.lower()))
        if not query_words:
            return RetrievalResult(facts=[], trace=["no_keywords"], confidence=0.0), []

        scored_memories: Dict[
            str, Tuple[float, str, str, dict]
        ] = {}  # mem_id -> (score, content, source, mem_dict)

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
                total = (
                    title_score + summary_score + content_score + (1.0 / (depth + 1))
                )
                if total > 0:
                    existing = scored_memories.get(mid, (0, "", "", {}))
                    if total > existing[0]:
                        scored_memories[mid] = (
                            total,
                            mem["content"],
                            mem.get("source", ""),
                            mem,
                        )

            for child in node.get("children", []):
                score_node(child, depth + 1)

        score_node(tree)

        sorted_mems = sorted(scored_memories.values(), key=lambda x: x[0], reverse=True)
        top = sorted_mems[:top_k]

        if not top:
            return (
                RetrievalResult(facts=[], trace=["root", "no_match"], confidence=0.0),
                [],
            )

        facts = [s[1] for s in top]
        sources = [s[2] for s in top]
        matched_memories = [s[3] for s in top]
        avg_score = sum(s[0] for s in top) / len(top)
        confidence = min(avg_score / 10, 1.0)  # Normalize

        # Build simple trace from matched content
        trace = ["root", "keyword_search"]
        if facts:
            trace.append(facts[0][:30])

        return (
            RetrievalResult(
                facts=facts,
                trace=trace,
                confidence=round(confidence, 2),
                sources=sources,
            ),
            matched_memories,
        )
