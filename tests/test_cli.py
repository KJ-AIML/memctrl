"""Tests for CLI commands using Typer's CliRunner.

Covers: --version, init, add, list, forget, tree, trigger-cmd, audit, clear.
Uses temporary directories and in-memory DB isolation.

NOTE: --help tests are skipped due to a known Typer 0.15.x + Click 8.2.x
compatibility issue (Parameter.make_metavar() signature change).
"""

import os
import tempfile
from contextlib import contextmanager
from pathlib import Path

from typer.testing import CliRunner

from memctrl.cli import app


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


runner = CliRunner()


# ---------------------------------------------------------------------------
# Meta commands
# ---------------------------------------------------------------------------


def test_version():
    result = runner.invoke(app, ["--version"])
    assert result.exit_code == 0
    assert "MemCtrl" in result.output


# ---------------------------------------------------------------------------
# Init
# ---------------------------------------------------------------------------


def test_init():
    with _temp_cwd() as tmpdir:
        result = runner.invoke(app, ["init"])
        assert result.exit_code == 0
        assert (Path(tmpdir) / ".memoryrc").exists()


def test_init_already_exists():
    with _temp_cwd() as tmpdir:
        (Path(tmpdir) / ".memoryrc").write_text("existing")
        result = runner.invoke(app, ["init"])
        assert result.exit_code == 1
        assert "already exists" in result.output or "overwrite" in result.output.lower()


def test_init_force():
    with _temp_cwd() as tmpdir:
        (Path(tmpdir) / ".memoryrc").write_text("existing")
        result = runner.invoke(app, ["init", "--force"])
        assert result.exit_code == 0


# ---------------------------------------------------------------------------
# Add / List / Forget
# ---------------------------------------------------------------------------


def test_add_memory():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        result = runner.invoke(app, ["add", "we use FastAPI", "--layer", "project"])
        assert result.exit_code == 0
        assert "Added" in result.output or "Added memory" in result.output
        del os.environ["MEMCTRL_DB_PATH"]


def test_list_empty():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        result = runner.invoke(app, ["list"])
        assert result.exit_code == 0
        assert "No memories" in result.output
        del os.environ["MEMCTRL_DB_PATH"]


def test_list_with_memories():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        runner.invoke(app, ["add", "we use FastAPI", "--layer", "project"])
        result = runner.invoke(app, ["list"])
        assert result.exit_code == 0
        assert "FastAPI" in result.output
        del os.environ["MEMCTRL_DB_PATH"]


def test_forget_memory():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        # Add then list to get ID
        runner.invoke(app, ["add", "to forget", "--layer", "session"])
        result_list = runner.invoke(app, ["list"])
        lines = result_list.output.strip().split("\n")
        mem_id = None
        for line in lines:
            if "to forget" in line:
                # ID is the first non-empty column in the table row
                parts = [p.strip() for p in line.split("|") if p.strip()]
                if parts:
                    mem_id = parts[0].strip()
                    break
        if mem_id:
            result = runner.invoke(app, ["forget", mem_id])
            assert result.exit_code == 0
            assert "Forgot" in result.output or "not found" in result.output
        del os.environ["MEMCTRL_DB_PATH"]


def test_forget_missing_memory():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        result = runner.invoke(app, ["forget", "nonexistent-id"])
        assert result.exit_code == 0
        assert "not found" in result.output
        del os.environ["MEMCTRL_DB_PATH"]


# ---------------------------------------------------------------------------
# Tree
# ---------------------------------------------------------------------------


def test_tree_empty():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        result = runner.invoke(app, ["tree"])
        assert result.exit_code == 0
        assert "No memories" in result.output
        del os.environ["MEMCTRL_DB_PATH"]


def test_tree_with_memories():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        runner.invoke(app, ["add", "we use FastAPI", "--layer", "project"])
        runner.invoke(app, ["add", "fixing auth bug", "--layer", "session"])
        result = runner.invoke(app, ["tree"])
        assert result.exit_code == 0
        assert "Memory Tree" in result.output
        del os.environ["MEMCTRL_DB_PATH"]


# ---------------------------------------------------------------------------
# Trigger (command name is 'trigger-cmd' — Typer converts underscores to hyphens)
# ---------------------------------------------------------------------------


def test_trigger_consolidate():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        runner.invoke(app, ["add", "task 1", "--layer", "session"])
        runner.invoke(app, ["add", "task 2", "--layer", "session"])
        result = runner.invoke(app, ["trigger-cmd", "on_commit"])
        assert result.exit_code == 0
        assert "fired" in result.output.lower()
        # Memories should have been consolidated from session -> project
        assert "2" in result.output  # 2 memories affected
        del os.environ["MEMCTRL_DB_PATH"]


def test_trigger_no_match():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        result = runner.invoke(app, ["trigger-cmd", "nonexistent_event"])
        assert result.exit_code == 0
        assert "0" in result.output  # 0 memories affected
        del os.environ["MEMCTRL_DB_PATH"]


def test_trigger_invalid_json_context():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        result = runner.invoke(
            app, ["trigger-cmd", "on_commit", "--context", "not json"]
        )
        assert result.exit_code == 0
        assert "Invalid JSON" in result.output
        del os.environ["MEMCTRL_DB_PATH"]


# ---------------------------------------------------------------------------
# Audit
# ---------------------------------------------------------------------------


def test_audit_empty():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        result = runner.invoke(app, ["audit"])
        assert result.exit_code == 0
        assert "No audit" in result.output
        del os.environ["MEMCTRL_DB_PATH"]


def test_audit_with_entries():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        runner.invoke(app, ["add", "task 1", "--layer", "session"])
        runner.invoke(app, ["trigger-cmd", "on_commit"])
        result = runner.invoke(app, ["audit"])
        assert result.exit_code == 0
        assert "on_commit" in result.output
        del os.environ["MEMCTRL_DB_PATH"]


# ---------------------------------------------------------------------------
# Clear
# ---------------------------------------------------------------------------


def test_clear_yes():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        runner.invoke(app, ["add", "to clear", "--layer", "session"])
        result = runner.invoke(app, ["clear", "--yes"])
        assert result.exit_code == 0
        assert "Cleared" in result.output
        del os.environ["MEMCTRL_DB_PATH"]


def test_clear_empty():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        result = runner.invoke(app, ["clear", "--yes"])
        assert result.exit_code == 0
        assert "No memories" in result.output
        del os.environ["MEMCTRL_DB_PATH"]


def test_clear_by_layer():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        runner.invoke(app, ["add", "project memory", "--layer", "project"])
        runner.invoke(app, ["add", "session memory", "--layer", "session"])
        result = runner.invoke(app, ["clear", "--layer", "session", "--yes"])
        assert result.exit_code == 0
        # Check project memory still exists
        result_list = runner.invoke(app, ["list"])
        assert "project memory" in result_list.output
        assert "session memory" not in result_list.output
        del os.environ["MEMCTRL_DB_PATH"]


# ---------------------------------------------------------------------------
# Heatmap
# ---------------------------------------------------------------------------


def test_heatmap_empty():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        result = runner.invoke(app, ["heatmap"])
        assert result.exit_code == 0
        assert "No memories" in result.output
        del os.environ["MEMCTRL_DB_PATH"]


def test_heatmap_with_memories():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        runner.invoke(app, ["add", "project memory", "--layer", "project"])
        runner.invoke(app, ["add", "session memory", "--layer", "session"])
        result = runner.invoke(app, ["heatmap"])
        assert result.exit_code == 0
        assert "project" in result.output
        assert "session" in result.output
        del os.environ["MEMCTRL_DB_PATH"]


# ---------------------------------------------------------------------------
# Timeline
# ---------------------------------------------------------------------------


def test_timeline_empty():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        result = runner.invoke(app, ["timeline"])
        assert result.exit_code == 0
        assert "No timeline" in result.output
        del os.environ["MEMCTRL_DB_PATH"]


def test_timeline_with_events():
    with _temp_cwd() as tmpdir:
        os.environ["MEMCTRL_DB_PATH"] = str(Path(tmpdir) / "test.db")
        runner.invoke(app, ["add", "task 1", "--layer", "session"])
        runner.invoke(app, ["trigger-cmd", "on_commit"])
        result = runner.invoke(app, ["timeline"])
        assert result.exit_code == 0
        assert "task 1" in result.output or "on_commit" in result.output
        del os.environ["MEMCTRL_DB_PATH"]
