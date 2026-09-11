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
