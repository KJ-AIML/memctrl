"""MemCtrl — PageIndex-style hierarchical tree builder.

Adapts PageIndex (VectifyAI) tree architecture for memory:
  - Nodes: {node_id, title, layer, summary, memory_ids, children}
  - LLM clusters flat memories into semantic groups per layer
  - Produces explainable, inspectable hierarchy (no vectors)

Research: PageIndex uses {node_id, title, start_index, end_index, summary,
sub_nodes[]}. We replace page references with memory metadata.
"""

from __future__ import annotations

import json
import logging
import uuid
from typing import Any, Callable, Coroutine, Dict, List, Optional

from memctrl.sanitize import sanitize_text
from memctrl.store import TreeNode

logger = logging.getLogger("memctrl.tree")

# Type alias for async LLM callable
LLMCallable = Callable[[str, bool], Coroutine[Any, Any, str]]


class MemoryTreeBuilder:
    """PageIndex-inspired hierarchical tree builder for memories.

    Algorithm:
        1. Group memories by layer (project / session / user)
        2. Within each layer, use LLM to cluster memories into semantic groups
        3. Each cluster becomes a tree node with LLM-generated summary
        4. Leaf nodes contain individual memory facts

    Incremental rebuild:
        - Cache built layer nodes to avoid full rebuilds
        - Rebuild only the affected layer when memories change
        - O(branch_size) not O(total_memories) per change
    """

    def __init__(self, llm_client: Optional[LLMCallable] = None):
        self.llm_client = llm_client
        # Cache for incremental rebuilds: layer_name -> TreeNode
        self._layer_cache: Dict[str, TreeNode] = {}
        # Track memory count per layer for cache invalidation
        self._layer_counts: Dict[str, int] = {}

    # --- Public API ---

    async def build_tree(self, memories: List[dict]) -> TreeNode:
        """Build tree from flat list of memory dicts.

        memories: list of dicts with keys: id, layer, content, confidence
        Returns root TreeNode with layer children.

        Also populates the layer cache for subsequent incremental rebuilds.
        """
        if not memories:
            self._layer_cache.clear()
            self._layer_counts.clear()
            return TreeNode(
                id="root",
                title="Memory Tree",
                layer="root",
                summary="Empty memory store",
            )

        # 1. Group by layer
        by_layer = self._group_by_layer(memories)

        # 2. Build layer nodes (with LLM clustering inside each)
        # Also cache for incremental rebuilds
        self._layer_cache.clear()
        self._layer_counts.clear()
        layer_nodes: List[TreeNode] = []
        for layer_name, mems in by_layer.items():
            if self.llm_client:
                node = await self._cluster_with_llm(layer_name, mems)
            else:
                node = self._cluster_fallback(layer_name, mems)
            self._layer_cache[layer_name] = node
            self._layer_counts[layer_name] = len(mems)
            layer_nodes.append(node)

        # 3. Build root
        root = TreeNode(
            id="root",
            title="Memory Tree",
            layer="root",
            summary=f"Root node with {len(layer_nodes)} layers, "
            f"{len(memories)} total memories",
            children=layer_nodes,
        )
        return root

    # --- Incremental rebuild ---

    async def build_tree_incremental(
        self,
        memories: List[dict],
        changed_layer: Optional[str] = None,
    ) -> TreeNode:
        """Build tree, reusing cached layer nodes when possible.

        If changed_layer is specified, only that layer's branch is rebuilt.
        Other layers use cached nodes from the previous build.

        This reduces cost from O(total_memories) to O(branch_size) when
        only a single layer changes (the common case).

        Args:
            memories: Full list of memory dicts
            changed_layer: Which layer changed (project/session/user)
                           If None, performs full rebuild.

        Returns:
            Root TreeNode with layer children
        """
        if not memories:
            self._layer_cache.clear()
            self._layer_counts.clear()
            return TreeNode(
                id="root",
                title="Memory Tree",
                layer="root",
                summary="Empty memory store",
            )

        by_layer = self._group_by_layer(memories)

        # If no specific layer changed, rebuild all with caching
        if changed_layer is None or changed_layer not in by_layer:
            return await self.build_tree(memories)

        # Rebuild only the changed layer
        layer_nodes: List[TreeNode] = []
        for layer_name, mems in by_layer.items():
            if layer_name == changed_layer:
                # Rebuild this layer
                if self.llm_client:
                    node = await self._cluster_with_llm(layer_name, mems)
                else:
                    node = self._cluster_fallback(layer_name, mems)
                self._layer_cache[layer_name] = node
                self._layer_counts[layer_name] = len(mems)
            elif layer_name in self._layer_cache:
                # Check if count changed (defensive)
                cached_count = self._layer_counts.get(layer_name, 0)
                if cached_count != len(mems):
                    # Count mismatch — rebuild this layer too
                    if self.llm_client:
                        node = await self._cluster_with_llm(layer_name, mems)
                    else:
                        node = self._cluster_fallback(layer_name, mems)
                    self._layer_cache[layer_name] = node
                    self._layer_counts[layer_name] = len(mems)
                else:
                    node = self._layer_cache[layer_name]
            else:
                # New layer — build and cache
                if self.llm_client:
                    node = await self._cluster_with_llm(layer_name, mems)
                else:
                    node = self._cluster_fallback(layer_name, mems)
                self._layer_cache[layer_name] = node
                self._layer_counts[layer_name] = len(mems)
            layer_nodes.append(node)

        root = TreeNode(
            id="root",
            title="Memory Tree",
            layer="root",
            summary=f"Root node with {len(layer_nodes)} layers, "
            f"{len(memories)} total memories (incremental: {changed_layer} rebuilt)",
            children=layer_nodes,
        )
        return root

    def invalidate_cache(self) -> None:
        """Clear the layer cache. Call this when tree structure may be stale."""
        self._layer_cache.clear()
        self._layer_counts.clear()

    # --- Grouping ---

    def _group_by_layer(self, memories: List[dict]) -> Dict[str, List[dict]]:
        result: Dict[str, List[dict]] = {}
        for mem in memories:
            layer = mem.get("layer", "session")
            result.setdefault(layer, []).append(mem)
        return result

    # --- LLM clustering ---

    async def _cluster_with_llm(self, layer: str, memories: List[dict]) -> TreeNode:
        """Use LLM to cluster memories into semantic groups.

        If there are more than 20 memories, splits into batches to keep
        LLM prompt size bounded. Each batch is clustered independently
        and results are merged under the layer node.
        """
        batch_size = 20
        if len(memories) <= batch_size:
            return await self._cluster_single_batch(layer, memories)

        # Split into batches and cluster each one
        batches = [
            memories[i : i + batch_size] for i in range(0, len(memories), batch_size)
        ]
        batch_nodes: List[TreeNode] = []
        for i, batch in enumerate(batches):
            node = await self._cluster_single_batch(layer, batch, batch_index=i)
            batch_nodes.append(node)

        return TreeNode(
            id=f"layer_{layer}",
            title=layer.capitalize(),
            layer=layer,
            summary=f"{len(memories)} memories in layer '{layer}' (batched)",
            memory_ids=[m["id"] for m in memories],
            children=batch_nodes,
            confidence=self._avg_confidence(
                [m["id"] for m in memories], {m["id"]: m for m in memories}
            ),
        )

    async def _cluster_single_batch(self, layer: str, memories: List[dict], batch_index: int = 0) -> TreeNode:
        """Cluster a single batch of memories with the LLM."""
        prompt = self._build_cluster_prompt(layer, memories)

        try:
            response = await self.llm_client(prompt, json_mode=True)
            clusters = self._parse_clusters(response)
        except Exception as exc:
            logger.warning("LLM clustering failed for layer '%s': %s", layer, exc)
            clusters = []

        if not clusters:
            logger.info(
                "Using keyword fallback for layer '%s' batch (%d memories)",
                layer,
                len(memories),
            )
            return self._cluster_fallback(layer, memories)

        # Build cluster nodes
        children: List[TreeNode] = []
        mem_by_id = {m["id"]: m for m in memories}

        for cluster in clusters:
            cluster_mem_ids = [
                mid for mid in cluster.get("memory_ids", []) if mid in mem_by_id
            ]
            if not cluster_mem_ids:
                continue

            # Build leaf nodes for each memory
            leaf_nodes = []
            for mid in cluster_mem_ids:
                mem = mem_by_id[mid]
                leaf = TreeNode(
                    id=f"mem_{mid}",
                    title=mem["content"][:60],
                    layer=layer,
                    summary=mem["content"],
                    memory_ids=[mid],
                    confidence=mem.get("confidence", 1.0),
                )
                leaf_nodes.append(leaf)

            cluster_node = TreeNode(
                id=f"cluster_{uuid.uuid4().hex[:8]}",
                title=cluster.get("title", "cluster"),
                layer=layer,
                summary=cluster.get("summary", ""),
                memory_ids=cluster_mem_ids,
                children=leaf_nodes,
                confidence=self._avg_confidence(cluster_mem_ids, mem_by_id),
            )
            children.append(cluster_node)

        node_id = f"layer_{layer}_batch_{batch_index}" if batch_index > 0 else f"layer_{layer}"
        return TreeNode(
            id=node_id,
            title=layer.capitalize(),
            layer=layer,
            summary=f"{len(memories)} memories in layer '{layer}'",
            memory_ids=[m["id"] for m in memories],
            children=children,
            confidence=self._avg_confidence([m["id"] for m in memories], mem_by_id),
        )

    def _build_cluster_prompt(self, layer: str, memories: List[dict]) -> str:
        """Build LLM prompt for clustering memories."""
        mem_lines = "\n".join(
            f"  [{i}] id={m['id']} | {sanitize_text(m['content'])[:200]}"
            for i, m in enumerate(memories)
        )
        return (
            f"You are a memory organization expert. Group the following "
            f"memories from the '{layer}' layer into 3-8 semantic clusters.\n\n"
            f"Memories:\n{mem_lines}\n\n"
            f"Return ONLY JSON in this exact format:\n"
            f'{{"clusters": [\n'
            f'  {{"title": "short_name", "summary": "what this group covers", '
            f'"memory_ids": ["id1", "id2"]}}\n'
            f"]}}"
        )

    def _parse_clusters(self, response: str) -> List[dict]:
        """Parse LLM JSON response into cluster list."""
        try:
            data = json.loads(response)
            clusters = data.get("clusters", [])
            return clusters if isinstance(clusters, list) else []
        except json.JSONDecodeError:
            # Try to extract JSON from markdown code block
            if "```json" in response:
                try:
                    json_text = response.split("```json")[1].split("```")[0]
                    data = json.loads(json_text)
                    clusters = data.get("clusters", [])
                    return clusters if isinstance(clusters, list) else []
                except (json.JSONDecodeError, IndexError):
                    return []
            return []

    # --- Fallback clustering (no LLM) ---

    def _cluster_fallback(self, layer: str, memories: List[dict]) -> TreeNode:
        """Simple fallback: group by keyword matching."""
        keyword_groups = {
            "tech_stack": [
                "use",
                "using",
                "framework",
                "library",
                "database",
                "backend",
                "frontend",
                "api",
                "service",
            ],
            "decisions": [
                "decided",
                "adr",
                "choice",
                "chose",
                "migrate",
                "switch",
                "architecture",
            ],
            "preferences": [
                "prefer",
                "like",
                "style",
                "pattern",
                "convention",
                "always",
                "never",
            ],
            "tasks": [
                "implement",
                "building",
                "working",
                "task",
                "wip",
                "feature",
                "fix",
            ],
            "team": ["team", "meeting", "review", "standup", "sprint"],
        }

        groups: Dict[str, List[dict]] = {name: [] for name in keyword_groups}
        groups["other"] = []

        for mem in memories:
            content_lower = mem["content"].lower()
            matched = False
            for group_name, keywords in keyword_groups.items():
                if any(kw in content_lower for kw in keywords):
                    groups[group_name].append(mem)
                    matched = True
                    break
            if not matched:
                groups["other"].append(mem)

        mem_by_id = {m["id"]: m for m in memories}
        children: List[TreeNode] = []

        for group_name, group_mems in groups.items():
            if not group_mems:
                continue
            leaf_nodes = [
                TreeNode(
                    id=f"mem_{m['id']}",
                    title=m["content"][:60],
                    layer=layer,
                    summary=m["content"],
                    memory_ids=[m["id"]],
                    confidence=m.get("confidence", 1.0),
                )
                for m in group_mems
            ]
            cluster = TreeNode(
                id=f"cluster_{group_name}",
                title=group_name.replace("_", " ").title(),
                layer=layer,
                summary=f"{len(group_mems)} memories about {group_name}",
                memory_ids=[m["id"] for m in group_mems],
                children=leaf_nodes,
                confidence=self._avg_confidence(
                    [m["id"] for m in group_mems], mem_by_id
                ),
            )
            children.append(cluster)

        return TreeNode(
            id=f"layer_{layer}",
            title=layer.capitalize(),
            layer=layer,
            summary=f"{len(memories)} memories in layer '{layer}'",
            memory_ids=[m["id"] for m in memories],
            children=children,
            confidence=self._avg_confidence([m["id"] for m in memories], mem_by_id),
        )

    # --- Helpers ---

    @staticmethod
    def _avg_confidence(mem_ids: List[str], mem_by_id: Dict[str, dict]) -> float:
        if not mem_ids:
            return 1.0
        total = sum(mem_by_id.get(mid, {}).get("confidence", 1.0) for mid in mem_ids)
        return round(total / len(mem_ids), 2)


async def get_or_build_tree(store, mem_dicts, builder):
    """Load a persisted tree from the store, or build and persist a new one.

    This avoids rebuilding the tree from scratch on every CLI/MCP invocation
    when the memory set hasn't changed.

    Args:
        store: MemoryStore instance with build_tree_from_nodes() and
               rebuild_tree_atomic() methods.
        mem_dicts: List of memory dicts to build the tree from.
        builder: MemoryTreeBuilder instance.

    Returns:
        TreeNode root (loaded or freshly built).
    """
    loaded = store.build_tree_from_nodes()
    if loaded is not None:
        if len(set(loaded.all_memory_ids())) == len(mem_dicts):
            return loaded
    tree = await builder.build_tree(mem_dicts)
    store.rebuild_tree_atomic([tree])
    return tree
