"""Health analysis for MemCtrl memory stores."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from memctrl.sanitize import has_secrets


TRUSTED_SOURCES = {
    "manual",
    "mcp",
    "cli",
    "test",
    "benchmark",
    "arch_decision",
    "schema",
    "postmortem",
    "incident",
    "reflection",
    "langgraph",
    "langgraph_conversation",
}


def analyze_store_health(
    store, low_confidence_threshold: float = 0.5
) -> dict[str, Any]:
    """Return a health report for a MemoryStore.

    The report is intentionally JSON-friendly so the CLI can render it and
    future integrations can export it without scraping terminal output.
    """
    memories = store.list_memories()
    stats = store.stats()
    now = datetime.now()

    expired = [
        mem for mem in memories if mem.expires_at is not None and mem.expires_at < now
    ]
    low_confidence = [
        mem for mem in memories if mem.confidence < low_confidence_threshold
    ]
    risky_sources = [
        mem for mem in memories if mem.source.lower() not in TRUSTED_SOURCES
    ]
    secret_findings = [mem for mem in memories if has_secrets(mem.content)]

    provenance_rows = store.get_provenance(limit=1000)
    covered_ids = set()
    low_confidence_retrievals = 0
    for row in provenance_rows:
        if row.get("avg_confidence", 0.0) < low_confidence_threshold:
            low_confidence_retrievals += 1
        for source in row.get("sources", []):
            mid = source.get("memory_id")
            if mid:
                covered_ids.add(mid)

    memory_ids = {mem.id for mem in memories}
    covered_memory_ids = memory_ids & covered_ids
    coverage = len(covered_memory_ids) / len(memory_ids) if memory_ids else 1.0

    spans = store.get_otel_spans(limit=1000)
    error_spans = [span for span in spans if span.get("status") == "error"]

    warnings = []
    if expired:
        warnings.append("expired")
    if low_confidence:
        warnings.append("low_confidence")
    if risky_sources:
        warnings.append("risky_sources")
    if secret_findings:
        warnings.append("secret_findings")
    if memories and not provenance_rows:
        warnings.append("missing_provenance")
    elif coverage < 0.5:
        warnings.append("low_provenance_coverage")
    if error_spans:
        warnings.append("otel_errors")

    return {
        "status": "warn" if warnings else "ok",
        "warnings": warnings,
        "memory_count": len(memories),
        "expired_count": len(expired),
        "low_confidence_count": len(low_confidence),
        "risky_source_count": len(risky_sources),
        "secret_finding_count": len(secret_findings),
        "low_confidence_threshold": low_confidence_threshold,
        "memory_samples": {
            "expired": [_memory_summary(mem) for mem in expired[:5]],
            "low_confidence": [_memory_summary(mem) for mem in low_confidence[:5]],
            "risky_sources": [_memory_summary(mem) for mem in risky_sources[:5]],
            "secret_findings": [_memory_summary(mem) for mem in secret_findings[:5]],
        },
        "provenance": {
            "records": len(provenance_rows),
            "covered_memories": len(covered_memory_ids),
            "coverage": coverage,
            "low_confidence_retrievals": low_confidence_retrievals,
        },
        "opentelemetry": {
            "spans": len(spans),
            "error_spans": len(error_spans),
        },
        "store": stats,
    }


def _memory_summary(memory) -> dict[str, Any]:
    return {
        "id": memory.id,
        "layer": memory.layer,
        "source": memory.source,
        "confidence": memory.confidence,
        "content": memory.content[:120],
    }
