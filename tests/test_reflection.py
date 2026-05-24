"""Tests for ReflectionEngine — auto-detection of session end and consolidation.

Covers: explicit reflection, time-based detection, git commit detection,
heuristic non-triggering, result serialization, summary generation (LLM + fallback),
empty session handling, CLI commands (done, reflect).
"""

import os
import subprocess
import tempfile
from contextlib import contextmanager
from datetime import datetime, timedelta
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest
from typer.testing import CliRunner

from memctrl.cli import app
from memctrl.reflection import ReflectionEngine, ReflectionResult
from memctrl.rules import RuleEngine
from memctrl.store import MemoryStore


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
def tmp_store(tmp_path):
    """Provide an isolated MemoryStore in a temp directory."""
    os.chdir(tmp_path)
    db_path = tmp_path / "test.db"
    return MemoryStore(str(db_path))


@pytest.fixture
def engine(tmp_store):
    """Provide a RuleEngine pre-loaded with default rules."""
    return RuleEngine()


# ---------------------------------------------------------------------------
# ReflectionResult
# ---------------------------------------------------------------------------


def test_reflection_result_defaults():
    """ReflectionResult should have sensible defaults for all fields."""
    result = ReflectionResult(triggered=True)
    assert result.triggered is True
    assert result.event == ""
    assert result.consolidated_ids == []
    assert result.new_memories == []
    assert result.summary == ""
    assert isinstance(result.timestamp, datetime)


def test_reflection_result_to_dict():
    """to_dict should produce a JSON-serializable dict."""
    result = ReflectionResult(
        triggered=True,
        event="explicit",
        consolidated_ids=["id1", "id2"],
        new_memories=[{"id": "n1", "layer": "project"}],
        summary="test summary",
        timestamp=datetime(2024, 1, 1, 12, 0, 0),
    )
    d = result.to_dict()
    assert d["triggered"] is True
    assert d["event"] == "explicit"
    assert d["consolidated_ids"] == ["id1", "id2"]
    assert d["summary"] == "test summary"
    assert "2024-01-01T12:00:00" in d["timestamp"]


def test_reflection_result_to_dict_none_timestamp():
    """to_dict should handle None timestamp gracefully."""
    result = ReflectionResult(triggered=False, timestamp=None)
    d = result.to_dict()
    assert d["timestamp"] is None


# ---------------------------------------------------------------------------
# Explicit reflection (force=True)
# ---------------------------------------------------------------------------


def test_explicit_reflection_consolidates_session_memories(tmp_store, engine):
    """force=True should always consolidate session memories regardless of time."""
    tmp_store.insert_memory("session", "we decided to use FastAPI", "test")
    tmp_store.insert_memory("session", "fixed auth bug in middleware", "test")

    refl = ReflectionEngine(tmp_store, engine=engine)
    result = refl.check_and_reflect(force=True)

    assert result.triggered is True
    assert result.event == "explicit"
    assert len(result.consolidated_ids) == 2
    # Memories should be moved to project layer (+ 1 reflection memory)
    assert len(tmp_store.list_memories("session")) == 0
    assert len(tmp_store.list_memories("project")) == 3  # 2 consolidated + 1 reflection


def test_explicit_reflection_empty_session(tmp_store, engine):
    """force=True with no session memories should still mark as triggered but do nothing."""
    refl = ReflectionEngine(tmp_store, engine=engine)
    result = refl.check_and_reflect(force=True)

    assert result.triggered is True
    assert result.event == "explicit"
    assert result.consolidated_ids == []
    assert "No session memories" in result.summary


def test_explicit_reflection_creates_reflection_memory(tmp_store, engine):
    """Reflection should create a memory in project layer with source='reflection'."""
    tmp_store.insert_memory("session", "built the auth module", "test")

    refl = ReflectionEngine(tmp_store, engine=engine)
    result = refl.check_and_reflect(force=True)

    assert len(result.new_memories) == 1
    project_mems = tmp_store.list_memories("project")
    reflection_mems = [m for m in project_mems if m.source == "reflection"]
    assert len(reflection_mems) == 1
    assert "built the auth module" in reflection_mems[0].content


def test_explicit_reflection_logs_trigger(tmp_store, engine):
    """Reflection should log a trigger execution for audit."""
    tmp_store.insert_memory("session", "task 1", "test")

    refl = ReflectionEngine(tmp_store, engine=engine)
    refl.check_and_reflect(force=True)

    logs = tmp_store.get_trigger_log()
    assert len(logs) >= 1


# ---------------------------------------------------------------------------
# Time-based detection
# ---------------------------------------------------------------------------


def test_time_based_triggers_when_inactive(tmp_store, engine):
    """Reflection should trigger when last activity is older than inactivity_hours."""
    # Insert an old session memory (3 hours ago)
    old_time = datetime.now() - timedelta(hours=3)
    tmp_store.insert_memory(
        layer="session",
        content="old task from previous session",
        source="test",
    )
    # Patch the memory's created_at to be old
    # We need to update the DB directly since insert_memory uses now()
    with tmp_store._connect() as conn:
        conn.execute(
            "UPDATE memories SET created_at = ? WHERE layer = 'session'",
            (old_time.isoformat(),),
        )
        conn.commit()

    refl = ReflectionEngine(tmp_store, engine=engine, inactivity_hours=2.0)
    result = refl.check_and_reflect(force=False)

    assert result.triggered is True
    assert result.event == "on_session_end"


def test_time_based_does_not_trigger_when_active(tmp_store, engine):
    """Reflection should NOT trigger when there is recent activity."""
    tmp_store.insert_memory("session", "just did this task", "test")

    refl = ReflectionEngine(tmp_store, engine=engine, inactivity_hours=2.0)
    result = refl.check_and_reflect(force=False)

    assert result.triggered is False


def test_time_based_no_memories(tmp_store, engine):
    """Time-based detection should not trigger when no memories exist."""
    refl = ReflectionEngine(tmp_store, engine=engine, inactivity_hours=2.0)
    result = refl.check_and_reflect(force=False)

    assert result.triggered is False


def test_get_last_activity_returns_latest_memory_time(tmp_store):
    """get_last_activity should return the most recent memory timestamp."""
    tmp_store.insert_memory("session", "task 1", "test")
    tmp_store.insert_memory("project", "task 2", "test")

    refl = ReflectionEngine(tmp_store)
    last = refl.get_last_activity()

    assert last is not None
    # Should be within last minute (just created)
    assert datetime.now() - last < timedelta(minutes=1)


def test_get_last_activity_no_memories(tmp_store):
    """get_last_activity should return None when no memories exist."""
    refl = ReflectionEngine(tmp_store)
    assert refl.get_last_activity() is None


def test_get_last_activity_includes_trigger_log(tmp_store, engine):
    """get_last_activity should consider trigger log timestamps too."""
    tmp_store.insert_memory("session", "task 1", "test")
    engine.load()
    engine.fire_trigger("on_commit", {}, tmp_store)

    refl = ReflectionEngine(tmp_store)
    last = refl.get_last_activity()

    assert last is not None


# ---------------------------------------------------------------------------
# Git-based detection
# ---------------------------------------------------------------------------


def test_git_commit_detection_fires(tmp_store, engine):
    """Git heuristic should trigger when git log returns a recent commit."""
    refl = ReflectionEngine(tmp_store, engine=engine, inactivity_hours=2.0)

    with patch(
        "memctrl.reflection.subprocess.run",
        return_value=MagicMock(returncode=0, stdout="abc123 initial commit"),
    ):
        result = refl.check_and_reflect(force=False)

    assert result.triggered is True
    assert result.event == "on_commit"


def test_git_commit_detection_no_commit(tmp_store, engine):
    """Git heuristic should NOT trigger when no recent commits exist."""
    tmp_store.insert_memory("session", "some task", "test")
    # Make it old enough for time-based to not fire either
    old_time = datetime.now() - timedelta(minutes=30)
    with tmp_store._connect() as conn:
        conn.execute(
            "UPDATE memories SET created_at = ? WHERE layer = 'session'",
            (old_time.isoformat(),),
        )
        conn.commit()

    refl = ReflectionEngine(tmp_store, engine=engine, inactivity_hours=2.0)

    with patch(
        "memctrl.reflection.subprocess.run",
        return_value=MagicMock(returncode=0, stdout=""),
    ):
        result = refl.check_and_reflect(force=False)

    assert result.triggered is False


def test_git_commit_detection_not_git_repo(tmp_store, engine):
    """Git heuristic should handle non-git directories gracefully."""
    tmp_store.insert_memory("session", "some task", "test")
    old_time = datetime.now() - timedelta(minutes=30)
    with tmp_store._connect() as conn:
        conn.execute(
            "UPDATE memories SET created_at = ? WHERE layer = 'session'",
            (old_time.isoformat(),),
        )
        conn.commit()

    refl = ReflectionEngine(tmp_store, engine=engine, inactivity_hours=2.0)

    with patch(
        "memctrl.reflection.subprocess.run",
        return_value=MagicMock(returncode=128, stderr="fatal: not a git repo"),
    ):
        result = refl.check_and_reflect(force=False)

    assert result.triggered is False


def test_git_commit_detection_git_not_found(tmp_store, engine):
    """Git heuristic should handle git not being installed."""
    tmp_store.insert_memory("session", "some task", "test")
    old_time = datetime.now() - timedelta(minutes=30)
    with tmp_store._connect() as conn:
        conn.execute(
            "UPDATE memories SET created_at = ? WHERE layer = 'session'",
            (old_time.isoformat(),),
        )
        conn.commit()

    refl = ReflectionEngine(tmp_store, engine=engine, inactivity_hours=2.0)

    with patch(
        "memctrl.reflection.subprocess.run",
        side_effect=FileNotFoundError("git not found"),
    ):
        result = refl.check_and_reflect(force=False)

    assert result.triggered is False


def test_git_commit_detection_timeout(tmp_store, engine):
    """Git heuristic should handle subprocess timeout gracefully."""
    tmp_store.insert_memory("session", "some task", "test")
    old_time = datetime.now() - timedelta(minutes=30)
    with tmp_store._connect() as conn:
        conn.execute(
            "UPDATE memories SET created_at = ? WHERE layer = 'session'",
            (old_time.isoformat(),),
        )
        conn.commit()

    refl = ReflectionEngine(tmp_store, engine=engine, inactivity_hours=2.0)

    with patch(
        "memctrl.reflection.subprocess.run",
        side_effect=subprocess.TimeoutExpired("git", 5),
    ):
        result = refl.check_and_reflect(force=False)

    assert result.triggered is False


def test_check_git_commit_uses_correct_hours():
    """_check_git_commit should use inactivity_hours in the git command."""
    store = MemoryStore(str(Path(tempfile.mkdtemp()) / "test.db"))
    refl = ReflectionEngine(store, inactivity_hours=4.0)

    with patch("memctrl.reflection.subprocess.run") as mock_run:
        mock_run.return_value = MagicMock(returncode=0, stdout="")
        refl._check_git_commit()

        call_args = mock_run.call_args[0][0]
        assert "--since=4 hours ago" in call_args


# ---------------------------------------------------------------------------
# Summary generation
# ---------------------------------------------------------------------------


def test_generate_summary_with_llm(tmp_store):
    """LLM client should be used for summary when available."""
    mock_llm = MagicMock(return_value="We built the auth module using FastAPI.")
    refl = ReflectionEngine(tmp_store, llm_client=mock_llm)

    memories = [
        {"content": "built auth module"},
        {"content": "used FastAPI for routing"},
    ]
    summary = refl._generate_summary(memories)

    assert summary == "We built the auth module using FastAPI."
    mock_llm.assert_called_once()
    call_prompt = mock_llm.call_args[0][0]
    assert "built auth module" in call_prompt


def test_generate_summary_llm_failure_fallback(tmp_store):
    """LLM failure should fall back to heuristic summary."""
    mock_llm = MagicMock(side_effect=Exception("API error"))
    refl = ReflectionEngine(tmp_store, llm_client=mock_llm)

    memories = [
        {"content": "task one"},
        {"content": "task two"},
    ]
    summary = refl._generate_summary(memories)

    assert "task one" in summary
    assert "task two" in summary


def test_generate_summary_heuristic_join(tmp_store):
    """Without LLM, summary should join memory contents."""
    refl = ReflectionEngine(tmp_store)

    memories = [
        {"content": "decided on PostgreSQL"},
        {"content": "set up Docker compose"},
    ]
    summary = refl._generate_summary(memories)

    assert "decided on PostgreSQL" in summary
    assert "set up Docker compose" in summary


def test_generate_summary_single_memory(tmp_store):
    """Single memory should return its content directly."""
    refl = ReflectionEngine(tmp_store)

    memories = [{"content": "only task"}]
    summary = refl._generate_summary(memories)

    assert summary == "only task"


def test_generate_summary_empty_memories(tmp_store):
    """Empty memory list should return empty string."""
    refl = ReflectionEngine(tmp_store)
    assert refl._generate_summary([]) == ""


def test_generate_summary_llm_returns_none(tmp_store):
    """LLM returning None should fall back to heuristic."""
    mock_llm = MagicMock(return_value=None)
    refl = ReflectionEngine(tmp_store, llm_client=mock_llm)

    memories = [{"content": "fallback task"}]
    summary = refl._generate_summary(memories)

    assert summary == "fallback task"


def test_generate_summary_llm_returns_non_string(tmp_store):
    """LLM returning non-string should fall back to heuristic."""
    mock_llm = MagicMock(return_value=42)
    refl = ReflectionEngine(tmp_store, llm_client=mock_llm)

    memories = [{"content": "fallback task"}]
    summary = refl._generate_summary(memories)

    assert summary == "fallback task"


def test_generate_summary_deduplicates(tmp_store):
    """Duplicate memory contents should be deduplicated in summary."""
    refl = ReflectionEngine(tmp_store)

    memories = [
        {"content": "same task"},
        {"content": "same task"},
        {"content": "different task"},
    ]
    summary = refl._generate_summary(memories)

    # Should only appear once
    assert summary.count("same task") == 1


# ---------------------------------------------------------------------------
# Multiple layer consolidation
# ---------------------------------------------------------------------------


def test_reflection_consolidates_to_project(tmp_store, engine):
    """on_session_end trigger should consolidate session -> project."""
    tmp_store.insert_memory("session", "task A", "test")
    tmp_store.insert_memory("session", "task B", "test")

    refl = ReflectionEngine(tmp_store, engine=engine)
    _ = refl.check_and_reflect(force=True)

    project_mems = tmp_store.list_memories("project")
    assert len(project_mems) > 0
    # Session should be empty after consolidation
    assert len(tmp_store.list_memories("session")) == 0


# ---------------------------------------------------------------------------
# Constructor defaults
# ---------------------------------------------------------------------------


def test_engine_defaults(tmp_store):
    """ReflectionEngine should create a RuleEngine if none is provided."""
    refl = ReflectionEngine(tmp_store)
    assert refl.engine is not None
    assert isinstance(refl.engine, RuleEngine)


def test_inactivity_hours_default(tmp_store):
    """Default inactivity_hours should be 2.0."""
    refl = ReflectionEngine(tmp_store)
    assert refl.inactivity_hours == 2.0


def test_custom_inactivity_hours(tmp_store):
    """Custom inactivity_hours should be respected."""
    refl = ReflectionEngine(tmp_store, inactivity_hours=4.5)
    assert refl.inactivity_hours == 4.5


# ---------------------------------------------------------------------------
# Heuristic ordering (explicit wins over git wins over time)
# ---------------------------------------------------------------------------


def test_explicit_wins_over_git_and_time(tmp_store, engine):
    """force=True should trigger even when git and time heuristics don't fire."""
    # No memories, no git, no time issue
    refl = ReflectionEngine(tmp_store, engine=engine)
    result = refl.check_and_reflect(force=True)

    assert result.triggered is True
    assert result.event == "explicit"


def test_git_wins_over_time(tmp_store, engine):
    """When both git and time heuristics fire, git should win (checked first)."""
    # Make an old memory for time-based to fire
    tmp_store.insert_memory("session", "old task", "test")
    old_time = datetime.now() - timedelta(hours=5)
    with tmp_store._connect() as conn:
        conn.execute(
            "UPDATE memories SET created_at = ? WHERE layer = 'session'",
            (old_time.isoformat(),),
        )
        conn.commit()

    refl = ReflectionEngine(tmp_store, engine=engine, inactivity_hours=2.0)

    with patch(
        "memctrl.reflection.subprocess.run",
        return_value=MagicMock(returncode=0, stdout="abc123 recent commit"),
    ):
        result = refl.check_and_reflect(force=False)

    assert result.triggered is True
    assert result.event == "on_commit"


# ---------------------------------------------------------------------------
# Edge cases
# ---------------------------------------------------------------------------


def test_consolidate_with_no_matching_trigger_rule(tmp_store):
    """When trigger rule doesn't match, fallback consolidation should still work."""
    tmp_store.insert_memory("session", "orphan task", "test")

    # Use an engine with no matching triggers
    engine = RuleEngine()
    engine.rules.triggers = {}  # empty triggers

    refl = ReflectionEngine(tmp_store, engine=engine)
    result = refl.check_and_reflect(force=True)

    assert result.triggered is True
    assert len(result.consolidated_ids) == 1
    assert len(tmp_store.list_memories("session")) == 0


def test_reflection_result_str_summary():
    """Summary should be a string even for complex sessions."""
    result = ReflectionResult(triggered=True, summary="mixed: content; here")
    assert isinstance(result.summary, str)
    assert "mixed" in result.summary


def test_reflection_preserves_project_memories(tmp_store, engine):
    """Reflection should only move session memories, not touch project ones."""
    tmp_store.insert_memory("project", "existing project memory", "test")
    tmp_store.insert_memory("session", "session task", "test")

    refl = ReflectionEngine(tmp_store, engine=engine)
    refl.check_and_reflect(force=True)

    project_mems = tmp_store.list_memories("project")
    contents = [m.content for m in project_mems]
    assert "existing project memory" in contents


# ---------------------------------------------------------------------------
# CLI commands
# ---------------------------------------------------------------------------


runner = CliRunner()


@contextmanager
def _temp_cwd():
    """Change to a temp dir and restore on exit."""
    original = os.getcwd()
    with tempfile.TemporaryDirectory() as tmpdir:
        os.chdir(tmpdir)
        try:
            yield tmpdir
        finally:
            os.chdir(original)


def test_cli_done():
    """memctrl done should trigger reflection and consolidate session memories."""
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        # Add session memories
        runner.invoke(app, ["add", "task 1", "--layer", "session"])
        runner.invoke(app, ["add", "task 2", "--layer", "session"])

        result = runner.invoke(app, ["done"])

        assert result.exit_code == 0
        assert (
            "consolidated" in result.output.lower()
            or "Session consolidated" in result.output
        )
        # 2 memories should have been moved
        assert "2" in result.output

        # Session should be empty
        list_result = runner.invoke(app, ["list", "--layer", "session"])
        assert "No memories" in list_result.output

        del os.environ["MEMCTRL_DB_PATH"]


def test_cli_done_empty_session():
    """memctrl done with no session memories should report 0 memories moved."""
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")

        result = runner.invoke(app, ["done"])

        assert result.exit_code == 0
        assert (
            "0 memories moved" in result.output
            or "No session memories" in result.output
        )

        del os.environ["MEMCTRL_DB_PATH"]


def test_cli_reflect_no_trigger():
    """memctrl reflect should not trigger when heuristics don't fire."""
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        # Add a recent memory so time-based doesn't fire
        runner.invoke(app, ["add", "recent task", "--layer", "session"])

        # Mock git to return nothing
        with patch(
            "memctrl.reflection.subprocess.run",
            return_value=MagicMock(returncode=0, stdout=""),
        ):
            result = runner.invoke(app, ["reflect"])

        assert result.exit_code == 0
        assert "No reflection triggered" in result.output
        assert "2h" in result.output or "inactivity" in result.output

        del os.environ["MEMCTRL_DB_PATH"]


def test_cli_reflect_triggers_on_git():
    """memctrl reflect should trigger when git heuristic fires."""
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        # Add a not-so-recent memory
        runner.invoke(app, ["add", "some task", "--layer", "session"])
        old_time = datetime.now() - timedelta(minutes=30)
        from memctrl.store import MemoryStore

        store = MemoryStore(str(Path(tmpdir) / "test.db"))
        with store._connect() as conn:
            conn.execute(
                "UPDATE memories SET created_at = ? WHERE layer = 'session'",
                (old_time.isoformat(),),
            )
            conn.commit()

        with patch(
            "memctrl.reflection.subprocess.run",
            return_value=MagicMock(returncode=0, stdout="abc123 recent commit"),
        ):
            result = runner.invoke(app, ["reflect"])

        assert result.exit_code == 0
        assert "Reflection triggered" in result.output
        assert "on_commit" in result.output

        del os.environ["MEMCTRL_DB_PATH"]


def test_cli_reflect_triggers_on_time():
    """memctrl reflect should trigger when time-based heuristic fires."""
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        runner.invoke(app, ["add", "old task", "--layer", "session"])

        # Make the memory 3 hours old
        store = MemoryStore(str(Path(tmpdir) / "test.db"))
        old_time = datetime.now() - timedelta(hours=3)
        with store._connect() as conn:
            conn.execute(
                "UPDATE memories SET created_at = ? WHERE layer = 'session'",
                (old_time.isoformat(),),
            )
            conn.commit()

        # Mock git to return nothing (so git heuristic doesn't fire)
        with patch(
            "memctrl.reflection.subprocess.run",
            return_value=MagicMock(returncode=0, stdout=""),
        ):
            result = runner.invoke(app, ["reflect"])

        assert result.exit_code == 0
        assert "Reflection triggered" in result.output
        assert "on_session_end" in result.output

        del os.environ["MEMCTRL_DB_PATH"]
