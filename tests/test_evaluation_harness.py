"""Tests for Phase 6: Completed-Task Reuse Evaluation Harness."""

import pytest
from benchmarks.eval_heli_history import EvaluationHarness, HISTORICAL_TASKS_20, EVAL_BENCHMARK_QUERIES


@pytest.mark.asyncio
async def test_evaluation_harness_20_tasks(tmp_path):
    """The 20-task evaluation benchmark runs and satisfies the product gate."""
    db_file = tmp_path / "eval_harness.db"
    harness = EvaluationHarness(str(db_file))

    summary = await harness.run_benchmark()

    # 1. Corpus sanity check
    assert summary["total_tasks_evaluated"] == 20
    assert summary["total_queries"] == len(EVAL_BENCHMARK_QUERIES)

    # 2. Product gate verification (Section 31: >= 3 useful proposals, 0 misleading leads)
    assert summary["memctrl_useful_leads"] >= 3
    assert summary["memctrl_misleading_leads"] == 0
    assert summary["product_gate_passed"] is True

    # 3. Advantage over grep on semantic knowledge:
    # MemCtrl properly identifies refuted hypothesis in Q06 whereas raw grep surfaces it as truth
    q06_result = next(r for r in summary["query_results"] if r["query_id"] == "Q06")
    assert q06_result["memctrl_useful_lead"] is True
    assert q06_result["grep_misleading_lead"] is True


@pytest.mark.asyncio
async def test_holdout_queries_adversarial_check(tmp_path):
    """Adversarial holdout test with 4 queries not used during implementation tuning."""
    from memctrl.retriever import MemoryRetriever

    db_file = tmp_path / "eval_holdout.db"
    harness = EvaluationHarness(str(db_file))
    retriever = MemoryRetriever()

    memories = harness.store.list_memories(include_candidates=True, include_history=True)
    mem_lookup = {m.id: m.to_dict() for m in memories}
    tree = {
        "id": "root",
        "title": "Heli Knowledge Tree",
        "layer": "root",
        "summary": "root",
        "memory_ids": list(mem_lookup.keys()),
        "children": [],
    }

    # H1: Thai + English token clock skew
    res_h1 = await retriever.retrieve("มีปัญหาเรื่อง token clock skew หรือไม่", tree, memory_lookup=mem_lookup, include_candidates=True)
    assert len(res_h1.facts) > 0
    # Traceability check: must trace to task-002
    h1_tasks = [ev.source_id for mid in res_h1.memory_ids for ev in harness.store.get_memory_evidence(mid)]
    assert "heli-task-002" in h1_tasks

    # H2: Cold start 401 provider hydration (same symptom, distinct cause)
    res_h2 = await retriever.retrieve("repeated 401 unauthorized errors on cold start", tree, memory_lookup=mem_lookup, include_candidates=True)
    assert len(res_h2.facts) > 0
    h2_tasks = [ev.source_id for mid in res_h2.memory_ids for ev in harness.store.get_memory_evidence(mid)]
    assert any(t in h2_tasks for t in ["heli-task-001", "heli-task-016"])
    # Crucial: distractor task-002 (clock skew) must not be the primary answer
    assert "provider hydration" in " ".join(res_h2.facts).lower()

    # H3: Refuted hypothesis holdout (must be annotated [REFUTED] in history mode)
    res_h3 = await retriever.retrieve("network tunnel dropped WebSocket pings", tree, memory_lookup=mem_lookup, history=True)
    assert len(res_h3.facts) > 0
    assert any("[REFUTED]" in f for f in res_h3.facts)

    # H4: Concurrent SQLite lock resolution
    res_h4 = await retriever.retrieve("SQLite database is locked concurrent writers", tree, memory_lookup=mem_lookup, include_candidates=True)
    assert len(res_h4.facts) > 0
    h4_tasks = [ev.source_id for mid in res_h4.memory_ids for ev in harness.store.get_memory_evidence(mid)]
    assert "heli-task-006" in h4_tasks
