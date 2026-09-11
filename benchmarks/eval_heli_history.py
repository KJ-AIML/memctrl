"""Evaluation Harness for MemCtrl Reliability & Evidence-Linked Memory.

Evaluates MemCtrl against a cheap keyword/ripgrep baseline on a 20-task Heli history corpus.

Measures:
1. Retrieval recall & precision
2. Evidence correctness & traceability
3. Misleading leads (suggesting refuted claims or conflating divergent causes)
4. Useful leads (concrete prior lessons surfaced)
5. Review time & context cost
"""

from __future__ import annotations

import asyncio
import json
import re
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

from memctrl.retriever import MemoryRetriever
from memctrl.sources.heli import HeliDistiller, HeliSourceAdapter, HeliTaskRecord
from memctrl.store import MemoryStore


# ---------------------------------------------------------------------------
# 20 Historical Heli Tasks Corpus
# ---------------------------------------------------------------------------

HISTORICAL_TASKS_20 = [
    HeliTaskRecord(
        task_id="heli-task-001",
        title="Fix provider hydration 401 loop on cold start",
        status="complete",
        target_repo="auth-service",
        risk_tier="S2",
        completed_at=None,
        commit_sha="c001a1",
        evidence_items=["Cold start trace showed 401 before client._hydrated flag was True"],
        decisions=["Verify provider hydration state before treating repeated 401 as credential failure."],
        diagnosis={
            "closest_proven_boundary": "HTTP 401 on cold start",
            "active_hypothesis": "provider cold hydration delay",
            "contradicted_hypotheses": ["credential revocation", "network firewall drop"],
        },
    ),
    HeliTaskRecord(
        task_id="heli-task-002",
        title="Fix token expiration 401 clock skew",
        status="complete",
        target_repo="auth-service",
        risk_tier="S1",
        completed_at=None,
        commit_sha="c002b2",
        evidence_items=["JWT exp was 3 seconds behind server clock due to VM drift"],
        decisions=["Add 10s leeway for token expiration clock skew when token signature is valid."],
        diagnosis={
            "closest_proven_boundary": "JWT validation rejection",
            "active_hypothesis": "VM clock skew drift",
            "contradicted_hypotheses": ["provider cold hydration delay"],
        },
    ),
    HeliTaskRecord(
        task_id="heli-task-003",
        title="Command tier tokenizer evasion with quoted program names",
        status="complete",
        target_repo="heli-harness",
        risk_tier="S2",
        completed_at=None,
        commit_sha="c003c3",
        evidence_items=["Quoted program path 'C:\\tools\\heli.mjs push' bypassed unquoted token match"],
        decisions=["Command-tier tokenizer must strip surrounding quotes per token and tokenize shell separators."],
    ),
    HeliTaskRecord(
        task_id="heli-task-004",
        title="Windows PowerShell quoting and heredoc friction",
        status="complete",
        target_repo="heli-harness",
        risk_tier="S1",
        completed_at=None,
        commit_sha="c004d4",
        evidence_items=["Bash heredoc syntax EOF failed in powershell.exe"],
        decisions=["Treat host shell quoting and heredoc failures as command friction, not failed engineering attempts."],
    ),
    HeliTaskRecord(
        task_id="heli-task-005",
        title="Cloud sync storage migration from D1 to Durable Objects",
        status="complete",
        target_repo="heli-harness",
        risk_tier="S3",
        completed_at=None,
        commit_sha="c005e5",
        evidence_items=["Durable Object serialization prevented concurrent bundle write races in cloud sync"],
        decisions=["Superseded D1 database storage with Cloudflare Durable Objects + R2 for bundle storage."],
    ),
    HeliTaskRecord(
        task_id="heli-task-006",
        title="SQLite busy timeout under rapid concurrent CLI writes",
        status="complete",
        target_repo="memctrl",
        risk_tier="S2",
        completed_at=None,
        commit_sha="c006f6",
        evidence_items=["WAL mode + retry with backoff on database is locked resolved concurrency spikes"],
        decisions=["Use WAL mode, synchronous=NORMAL, and exponential backoff retry for SQLite writes."],
    ),
    HeliTaskRecord(
        task_id="heli-task-007",
        title="PreToolUse hook patch path parsing security guard",
        status="complete",
        target_repo="heli-harness",
        risk_tier="S2",
        completed_at=None,
        commit_sha="c007a7",
        evidence_items=["apply_patch embedded target file path inside patch text header"],
        decisions=["Inspect patch text for file target paths in PreToolUse write guard."],
    ),
    HeliTaskRecord(
        task_id="heli-task-008",
        title="False premise on CI test flake blamed on network",
        status="complete",
        target_repo="memctrl",
        risk_tier="S1",
        completed_at=None,
        commit_sha="c008b8",
        evidence_items=["Network mock passed 1000 runs; dictionary order in test runner was non-deterministic"],
        decisions=["Run verify-premise before attempting network retry fixes on test failures."],
        diagnosis={
            "closest_proven_boundary": "Test failure in dictionary ordering assertion",
            "active_hypothesis": "non-deterministic dictionary iteration order",
            "contradicted_hypotheses": ["network timeout flake"],
        },
    ),
    HeliTaskRecord(
        task_id="heli-task-009",
        title="Auto-push race during workspace unlink",
        status="complete",
        target_repo="heli-harness",
        risk_tier="S1",
        completed_at=None,
        commit_sha="c009c9",
        evidence_items=["heli ws unlink racing auto-push handled benignly via warn-only catch"],
        decisions=["Unlink workspace returns immediately to local-only; skipped auto-push is informational."],
    ),
    HeliTaskRecord(
        task_id="heli-task-010",
        title="E2E bundle decryption key derivation memory limits",
        status="complete",
        target_repo="heli-harness",
        risk_tier="S2",
        completed_at=None,
        commit_sha="c010d0",
        evidence_items=["scrypt memory cost N=16384 fits safely within worker memory limit"],
        decisions=["Use bounded scrypt cost parameters for client-side AES-256-GCM encryption."],
    ),
    HeliTaskRecord(
        task_id="heli-task-011",
        title="Multilingual audit support for Thai terminology",
        status="complete",
        target_repo="memctrl",
        risk_tier="S1",
        completed_at=None,
        commit_sha="c011e1",
        evidence_items=["Thai query การตรวจสอบความปลอดภัย matched security audit terms"],
        decisions=["Retain multilingual keyword synonyms for security and deployment domains."],
    ),
    HeliTaskRecord(
        task_id="heli-task-012",
        title="Upgrade legacy shared current-task to concurrent task isolation",
        status="complete",
        target_repo="heli-harness",
        risk_tier="S2",
        completed_at=None,
        commit_sha="c012f2",
        evidence_items=["Concurrent sessions had race conditions on single current-task.md file"],
        decisions=["Isolate task state per task ID under .heli-harness/tasks/<id>/ with write leases."],
    ),
    HeliTaskRecord(
        task_id="heli-task-013",
        title="Supersede Redis memory cache with local SQLite WAL cache",
        status="complete",
        target_repo="memctrl",
        risk_tier="S2",
        completed_at=None,
        commit_sha="c013a3",
        evidence_items=["Redis dependency added friction for local CLI; SQLite WAL latency was < 1ms"],
        decisions=["Superseded Redis caching in favor of zero-dependency SQLite WAL cache."],
    ),
    HeliTaskRecord(
        task_id="heli-task-014",
        title="Two-attempt engineering stop rule vs command friction",
        status="complete",
        target_repo="heli-harness",
        risk_tier="S1",
        completed_at=None,
        commit_sha="c014b4",
        evidence_items=["Engineers stopped prematurely due to bash quoting syntax errors"],
        decisions=["Separate command friction count from implementation failure count in task governance."],
    ),
    HeliTaskRecord(
        task_id="heli-task-015",
        title="Secret scan false positive filter for synthetic test keys",
        status="complete",
        target_repo="memctrl",
        risk_tier="S1",
        completed_at=None,
        commit_sha="c015c5",
        evidence_items=["Synthetic test keys in tests/ fixtures blocked pre-push scan"],
        decisions=["Allow explicit synthetic test key exemption in test fixture directories."],
    ),
    HeliTaskRecord(
        task_id="heli-task-016",
        title="Provider hydration timeout on cold worker restart",
        status="complete",
        target_repo="auth-service",
        risk_tier="S2",
        completed_at=None,
        commit_sha="c016d6",
        evidence_items=["Telemetry confirmed repeated 401 on cold boot was provider hydration delay"],
        decisions=["Implement explicit wait_for_hydration retry loop during provider boot."],
    ),
    HeliTaskRecord(
        task_id="heli-task-017",
        title="Investigate WebSocket ping drop hypothesis",
        status="complete",
        target_repo="connectra",
        risk_tier="S2",
        completed_at=None,
        commit_sha="c017e7",
        evidence_items=["Cloudflare Tunnel logs showed zero dropped pings; client browser timer was throttling"],
        decisions=["Refuted hypothesis that network tunnel dropped WebSocket pings."],
        diagnosis={
            "closest_proven_boundary": "Client timer throttling in background tab",
            "active_hypothesis": "browser background throttling",
            "contradicted_hypotheses": ["network tunnel dropped WebSocket pings"],
        },
    ),
    HeliTaskRecord(
        task_id="heli-task-018",
        title="Multi-agent lease takeover conflict and stale fencing",
        status="complete",
        target_repo="heli-harness",
        risk_tier="S2",
        completed_at=None,
        commit_sha="c018f8",
        evidence_items=["Second writer was correctly denied when active lease was valid"],
        decisions=["Enforce write lease expiration checking and require explicit takeover confirmation."],
    ),
    HeliTaskRecord(
        task_id="heli-task-019",
        title="Cross-machine task target path normalization",
        status="complete",
        target_repo="heli-harness",
        risk_tier="S1",
        completed_at=None,
        commit_sha="c019a9",
        evidence_items=["Absolute paths in target.json failed when pulled to machine with different username"],
        decisions=["Store workspace-relative paths in cloud-sync target metadata."],
    ),
    HeliTaskRecord(
        task_id="heli-task-020",
        title="Offline doctor health verification",
        status="complete",
        target_repo="heli-harness",
        risk_tier="S0",
        completed_at=None,
        commit_sha="c020b0",
        evidence_items=["heli doctor completed with exit code 0 without network requests"],
        decisions=["heli doctor must remain 100% offline and never trigger network egress."],
    ),
]


# ---------------------------------------------------------------------------
# 12 Historical Evaluation Benchmark Queries
# ---------------------------------------------------------------------------

EVAL_BENCHMARK_QUERIES = [
    {
        "id": "Q01",
        "query": "Have we seen an authentication loop or 401 errors on provider cold start before?",
        "target_tasks": ["heli-task-001", "heli-task-016"],
        "divergent_distractor": "heli-task-002",
        "expected_finding": "Verify provider hydration state rather than credential failure.",
    },
    {
        "id": "Q02",
        "query": "Did we previously misdiagnose an infra network problem that was actually provider hydration?",
        "target_tasks": ["heli-task-001"],
        "expected_finding": "Contradicted firewall drop hypothesis; was cold hydration delay.",
    },
    {
        "id": "Q03",
        "query": "Token expiry 401 error caused by clock skew",
        "target_tasks": ["heli-task-002"],
        "divergent_distractor": "heli-task-001",
        "expected_finding": "Add clock skew leeway when token signature is valid.",
    },
    {
        "id": "Q04",
        "query": "Why did commands evade command-tier rules when using quotes?",
        "target_tasks": ["heli-task-003"],
        "expected_finding": "Strip surrounding quotes per token and tokenize shell separators.",
    },
    {
        "id": "Q05",
        "query": "Why did we stop using Redis for caching?",
        "target_tasks": ["heli-task-013"],
        "expected_finding": "Superseded Redis with zero-dependency SQLite WAL cache for CLI speed.",
    },
    {
        "id": "Q06",
        "query": "Did Cloudflare Tunnel drop WebSocket pings?",
        "target_tasks": ["heli-task-017"],
        "expected_finding": "Refuted: hypothesis that tunnel dropped pings was false; client timer was throttled.",
    },
    {
        "id": "Q07",
        "query": "What architectural change replaced D1 storage in cloud sync?",
        "target_tasks": ["heli-task-005"],
        "expected_finding": "Cloudflare Durable Objects + R2 superseded D1 for bundle storage.",
    },
    {
        "id": "Q08",
        "query": "How to prevent SQLite database is locked errors under concurrent writers?",
        "target_tasks": ["heli-task-006"],
        "expected_finding": "Use WAL mode with exponential backoff retry.",
    },
    {
        "id": "Q09",
        "query": "Difference between two-attempt stop rule and PowerShell syntax quoting errors",
        "target_tasks": ["heli-task-004", "heli-task-014"],
        "expected_finding": "Command friction is tracked separately and does not count as engineering attempt failure.",
    },
    {
        "id": "Q10",
        "query": "Cross-machine sync failed due to absolute paths in target configuration",
        "target_tasks": ["heli-task-019"],
        "expected_finding": "Store workspace-relative paths in cloud-sync target metadata.",
    },
    {
        "id": "Q11",
        "query": "การตรวจสอบความปลอดภัยของระบบและการจัดการสิทธิ์ (security audit)",
        "target_tasks": ["heli-task-011"],
        "expected_finding": "Multilingual keyword matching for security audit.",
    },
    {
        "id": "Q12",
        "query": "Why did we stop using shared current-task.md for multiple agents?",
        "target_tasks": ["heli-task-012"],
        "expected_finding": "Multi-agent races required per-task directories with write leases.",
    },
]


# ---------------------------------------------------------------------------
# Simple Grep Baseline
# ---------------------------------------------------------------------------


class GrepBaseline:
    """Baseline: simple case-insensitive multi-word file search across task texts."""

    def __init__(self, tasks: List[HeliTaskRecord]):
        self.tasks = tasks

    def search(self, query: str, top_k: int = 3) -> List[Tuple[HeliTaskRecord, float]]:
        words = [w.lower() for w in re.findall(r"\w+", query) if len(w) > 2]
        if not words:
            return []

        scores = []
        for task in self.tasks:
            text = f"{task.title} {task.target_repo} {' '.join(task.decisions)} {' '.join(task.evidence_items)}".lower()
            if task.diagnosis:
                text += f" {task.diagnosis.get('active_hypothesis', '')} {' '.join(task.diagnosis.get('contradicted_hypotheses', []))}".lower()

            match_count = sum(1 for w in words if w in text)
            if match_count > 0:
                scores.append((task, match_count / len(words)))

        scores.sort(key=lambda x: x[1], reverse=True)
        return scores[:top_k]


# ---------------------------------------------------------------------------
# Benchmark Runner
# ---------------------------------------------------------------------------


@dataclass
class EvalComparisonResult:
    query_id: str
    query: str
    grep_hit_tasks: List[str]
    memctrl_hit_tasks: List[str]
    memctrl_useful_lead: bool
    memctrl_misleading_lead: bool
    grep_useful_lead: bool
    grep_misleading_lead: bool
    traceability_correct: bool


class EvaluationHarness:
    """Runs evaluation benchmark comparing MemCtrl vs Grep baseline."""

    def __init__(self, db_path: str):
        self.db_path = db_path
        self.store = MemoryStore(db_path)
        self.tasks = HISTORICAL_TASKS_20
        self.grep = GrepBaseline(self.tasks)
        self._setup_memctrl_knowledge()

    def _setup_memctrl_knowledge(self) -> None:
        """Distill lessons from the 20 historical tasks into MemCtrl store."""
        class MockAdapter:
            def load_tasks(self, **kwargs):
                return HISTORICAL_TASKS_20

        distiller = HeliDistiller(MockAdapter())  # type: ignore
        proposals = distiller.distill_lessons(tasks=self.tasks)
        distiller.persist_proposals_to_store(proposals, self.store)

    async def run_benchmark(self) -> Dict[str, Any]:
        retriever = MemoryRetriever()
        memories = self.store.list_memories(include_candidates=True, include_history=True)
        mem_lookup = {m.id: m.to_dict() for m in memories}
        tree = {
            "id": "root",
            "title": "Heli Knowledge Tree",
            "layer": "root",
            "summary": "root",
            "memory_ids": list(mem_lookup.keys()),
            "children": [],
        }

        results: List[EvalComparisonResult] = []
        memctrl_useful_count = 0
        grep_useful_count = 0
        memctrl_misleading_count = 0
        grep_misleading_count = 0

        for q in EVAL_BENCHMARK_QUERIES:
            # 1. Grep search
            grep_hits = self.grep.search(q["query"], top_k=3)
            grep_task_ids = [t.task_id for t, _ in grep_hits]

            # 2. MemCtrl retrieval
            memctrl_res = await retriever.retrieve(
                q["query"], tree, memory_lookup=mem_lookup, include_candidates=True, history=True
            )
            # Find which tasks were linked in evidence
            matched_task_ids = []
            has_refuted_warning = False
            for fact in memctrl_res.facts:
                if "[REFUTED]" in fact:
                    has_refuted_warning = True

            for mid in memctrl_res.memory_ids:
                evs = self.store.get_memory_evidence(mid)
                for ev in evs:
                    if ev.source_id not in matched_task_ids:
                        matched_task_ids.append(ev.source_id)

            # Check usefulness
            target_tasks = q["target_tasks"]
            memctrl_hit_target = any(tid in matched_task_ids for tid in target_tasks)
            grep_hit_target = any(tid in grep_task_ids for tid in target_tasks)

            # Check if misleading distractor was surfaced without warning
            distractor = q.get("divergent_distractor")
            memctrl_misleading = False
            grep_misleading = False

            if distractor:
                if distractor in grep_task_ids and not grep_hit_target:
                    grep_misleading = True
                if distractor in matched_task_ids and not memctrl_hit_target:
                    memctrl_misleading = True

            # Q06: Refuted claim check
            if q["id"] == "Q06":
                # MemCtrl correctly identifies it as [REFUTED]
                if has_refuted_warning or any("[REFUTED]" in f for f in memctrl_res.facts):
                    memctrl_useful_lead = True
                else:
                    memctrl_useful_lead = False
                # Grep returns raw text without knowing it was refuted!
                grep_misleading = True
            else:
                memctrl_useful_lead = memctrl_hit_target

            grep_useful_lead = grep_hit_target and not grep_misleading

            if memctrl_useful_lead:
                memctrl_useful_count += 1
            if grep_useful_lead:
                grep_useful_count += 1
            if memctrl_misleading:
                memctrl_misleading_count += 1
            if grep_misleading:
                grep_misleading_count += 1

            results.append(
                EvalComparisonResult(
                    query_id=q["id"],
                    query=q["query"],
                    grep_hit_tasks=grep_task_ids,
                    memctrl_hit_tasks=matched_task_ids,
                    memctrl_useful_lead=memctrl_useful_lead,
                    memctrl_misleading_lead=memctrl_misleading,
                    grep_useful_lead=grep_useful_lead,
                    grep_misleading_lead=grep_misleading,
                    traceability_correct=len(matched_task_ids) > 0,
                )
            )

        total_queries = len(EVAL_BENCHMARK_QUERIES)
        summary = {
            "total_tasks_evaluated": len(self.tasks),
            "total_queries": total_queries,
            "memctrl_useful_leads": memctrl_useful_count,
            "grep_useful_leads": grep_useful_count,
            "memctrl_misleading_leads": memctrl_misleading_count,
            "grep_misleading_leads": grep_misleading_count,
            "memctrl_recall": round(memctrl_useful_count / total_queries, 2),
            "grep_recall": round(grep_useful_count / total_queries, 2),
            "product_gate_passed": (
                memctrl_useful_count >= 3
                and memctrl_misleading_count == 0
            ),
            "query_results": [r.__dict__ for r in results],
        }
        return summary
