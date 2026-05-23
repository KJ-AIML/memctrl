"""MemCtrl — Rule-governed memory layer for AI coding assistants.

Inspired by PageIndex's tree-based retrieval and Graphify's install pattern.
Uses hierarchical reasoning (not vectors) for explainable memory retrieval.
"""

__version__ = "1.1.0"

from memctrl.store import Memory, MemoryStore, TriggerLog, TreeNode
from memctrl.retriever import RetrievalResult

__all__ = [
    "__version__",
    "Memory",
    "MemoryStore",
    "TreeNode",
    "TriggerLog",
    "RetrievalResult",
]
