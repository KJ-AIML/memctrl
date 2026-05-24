"""MemCtrl — Rule-governed memory layer for AI coding assistants.

Inspired by PageIndex's tree-based retrieval and Graphify's install pattern.
Uses hierarchical reasoning (not vectors) for explainable memory retrieval.
"""

__version__ = "1.2.0"

from memctrl.store import Memory, MemoryStore, TriggerLog, TreeNode
from memctrl.retriever import RetrievalResult
from memctrl.decay import ConfidenceDecay, DECAY_RULES
from memctrl.reflection import ReflectionEngine, ReflectionResult
from memctrl.provenance import MemorySource, ProvenanceTracker, RetrievalProvenance
from memctrl.span import MemoryOperation, MemorySpan, SpanTracker
from memctrl.otel_exporter import MemoryOTelExporter

__all__ = [
    "__version__",
    "Memory",
    "MemoryStore",
    "TreeNode",
    "TriggerLog",
    "RetrievalResult",
    "ConfidenceDecay",
    "DECAY_RULES",
    "ReflectionEngine",
    "ReflectionResult",
    "ProvenanceTracker",
    "RetrievalProvenance",
    "MemorySource",
    "MemoryOperation",
    "MemorySpan",
    "SpanTracker",
    "MemoryOTelExporter",
]
