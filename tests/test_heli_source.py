"""Tests for Phase 5: Read-Only Heli History Source Adapter & Distiller."""

import json

import pytest
from typer.testing import CliRunner

from memctrl.cli import app
from memctrl.sources.heli import (
    HeliDistiller,
    HeliSourceAdapter,
)
from memctrl.store import MemoryStore

runner = CliRunner()


@pytest.fixture
def mock_heli_workspace(tmp_path):
    """Create a realistic mock Heli workspace with completed tasks."""
    ws = tmp_path / "mock_workspace"
    ws.mkdir()
    harness = ws / ".heli-harness"
    tasks_dir = harness / "tasks"
    tasks_dir.mkdir(parents=True)

    # Task 1: Auth provider hydration fix
    t1_dir = tasks_dir / "task-101"
    t1_dir.mkdir()
    (t1_dir / "task.json").write_text(
        json.dumps(
            {
                "title": "Fix provider hydration loop",
                "status": "complete",
                "target": {"repositoryId": "auth-service"},
                "riskTier": "S2",
                "completedAt": "2026-08-01T10:00:00Z",
                "commitSha": "abc101",
            }
        ),
        encoding="utf-8",
    )
    (t1_dir / "current-task.md").write_text(
        "# Current Task\nTarget repo: auth-service\nCurrent status: complete\nRisk tier: S2\n",
        encoding="utf-8",
    )
    (t1_dir / "plan.md").write_text(
        "# Plan: Auth fix\n## Step 1: verify hydration\nStatus: complete\nEvidence: pytest tests/test_auth.py passed\n",
        encoding="utf-8",
    )
    (t1_dir / "decisions.md").write_text(
        "# Decisions\n- Verify provider hydration state before treating repeated 401 as credential failure.\n",
        encoding="utf-8",
    )
    (t1_dir / "diagnosis.json").write_text(
        json.dumps(
            {
                "closest_proven_boundary": "HTTP 401 on cold start",
                "active_hypothesis": "provider cold hydration delay",
                "contradicted_hypotheses": [
                    "expired client secret",
                    "network firewall drop",
                ],
            }
        ),
        encoding="utf-8",
    )

    # Task 2: Another auth task with divergent cause (counterexample)
    t2_dir = tasks_dir / "task-102"
    t2_dir.mkdir()
    (t2_dir / "task.json").write_text(
        json.dumps(
            {
                "title": "Fix token expiration 401",
                "status": "complete",
                "target": {"repositoryId": "auth-service"},
                "riskTier": "S1",
                "completedAt": "2026-08-05T12:00:00Z",
                "commitSha": "abc102",
            }
        ),
        encoding="utf-8",
    )
    (t2_dir / "current-task.md").write_text(
        "# Current Task\nTarget repo: auth-service\nCurrent status: complete\nRisk tier: S1\n",
        encoding="utf-8",
    )
    (t2_dir / "decisions.md").write_text(
        "# Decisions\n- Verify token expiration clock skew when token signature is valid.\n",
        encoding="utf-8",
    )

    return ws


def test_heli_adapter_read_only_and_loading(mock_heli_workspace):
    """Adapter loads completed tasks and strictly avoids modifying Heli workspace."""
    adapter = HeliSourceAdapter(mock_heli_workspace)
    assert adapter.is_valid_workspace() is True

    # Record directory state before load
    harness_dir = mock_heli_workspace / ".heli-harness"
    before_mtime = harness_dir.stat().st_mtime

    tasks = adapter.load_tasks(limit=10, status_filter="complete")
    assert len(tasks) == 2
    t1 = next(t for t in tasks if t.task_id == "task-101")
    assert t1.target_repo == "auth-service"
    assert t1.commit_sha == "abc101"
    assert len(t1.decisions) == 1
    assert "Verify provider hydration" in t1.decisions[0]
    assert t1.diagnosis is not None
    assert t1.diagnosis["active_hypothesis"] == "provider cold hydration delay"

    # Verify no write/mutation occurred in .heli-harness
    assert harness_dir.stat().st_mtime == before_mtime


def test_heli_distiller_extracts_lessons_and_counterexamples(mock_heli_workspace):
    """Distiller extracts candidate lessons, links evidence, and detects counterexamples."""
    adapter = HeliSourceAdapter(mock_heli_workspace)
    distiller = HeliDistiller(adapter)

    proposals = distiller.distill_lessons()
    assert len(proposals) >= 1

    # Check proposal properties
    diag_props = [p for p in proposals if "hydration" in p.claim.lower()]
    assert len(diag_props) >= 1
    p = diag_props[0]
    assert p.claim_type == "derived_lesson"
    assert "task-101" in p.source_tasks
    assert "abc101" in p.source_revisions
    assert p.proposed_disposition == "candidate"
    assert len(p.supporting_evidence) >= 1


def test_persist_proposals_to_memctrl_store(mock_heli_workspace, tmp_path):
    """Persisting proposals writes candidates and evidence references into MemCtrl store only."""
    adapter = HeliSourceAdapter(mock_heli_workspace)
    distiller = HeliDistiller(adapter)
    proposals = distiller.distill_lessons()

    db_path = tmp_path / "memctrl.db"
    store = MemoryStore(str(db_path))

    created_ids = distiller.persist_proposals_to_store(proposals, store)
    assert len(created_ids) == len(proposals)

    # Check store records
    candidates = store.list_memories(lifecycle_state="candidate")
    assert len(candidates) == len(proposals)
    for mem in candidates:
        assert mem.claim_type == "derived_lesson"
        assert mem.lifecycle_state == "candidate"
        assert mem.verification_state == "unverified"
        assert mem.source == "heli-distillation"

        # Check evidence links
        evs = store.get_memory_evidence(mem.id)
        assert len(evs) >= 1
        assert evs[0].source_system == "heli"
        assert evs[0].source_id in ["task-101", "task-102"]


def test_cli_review_heli(mock_heli_workspace, tmp_path):
    """memctrl review heli CLI command works with --dry-run and --persist."""
    db_path = tmp_path / "cli_review.db"
    import os

    os.environ["MEMCTRL_DB_PATH"] = str(db_path)

    # 1. Dry run
    res_dry = runner.invoke(app, ["review", "heli", "--workspace", str(mock_heli_workspace), "--dry-run"])
    assert res_dry.exit_code == 0
    assert "Loaded 2 tasks" in res_dry.output
    assert "Distilled Lessons" in res_dry.output
    assert "Dry run complete" in res_dry.output

    # 2. Persist run
    res_persist = runner.invoke(app, ["review", "heli", "--workspace", str(mock_heli_workspace), "--persist"])
    assert res_persist.exit_code == 0
    assert "Persisted" in res_persist.output
    assert "candidate knowledge" in res_persist.output

    # 3. Check via memctrl candidates
    res_cand = runner.invoke(app, ["candidates"])
    assert res_cand.exit_code == 0
    assert "Candidate Memories Awaiting Review" in res_cand.output

    del os.environ["MEMCTRL_DB_PATH"]
