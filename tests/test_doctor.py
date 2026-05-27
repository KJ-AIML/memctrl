"""Tests for the MemCtrl doctor health report."""

import os
import sqlite3
import tempfile
from datetime import datetime, timedelta
from pathlib import Path

from typer.testing import CliRunner

from memctrl.cli import app
from memctrl.doctor import analyze_store_health
from memctrl.store import MemoryStore


def test_analyze_store_health_flags_memory_risks():
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    try:
        store = MemoryStore(db_path)
        covered_id = store.insert_memory("project", "we use FastAPI", "manual")
        store.insert_memory(
            "session",
            "old bug investigation",
            "external",
            confidence=0.35,
            expires_at=datetime.now() - timedelta(days=1),
        )

        # Simulate a pre-redaction legacy row that bypassed insert_memory().
        with sqlite3.connect(db_path) as conn:
            conn.execute(
                """INSERT INTO memories
                   (id, layer, content, source, confidence, created_at, expires_at, tags)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?)""",
                (
                    "legacy-secret",
                    "session",
                    "password=abc123",
                    "legacy",
                    0.8,
                    datetime.now().isoformat(),
                    None,
                    "[]",
                ),
            )
            conn.commit()

        store.save_provenance(
            {
                "query": "stack",
                "retrieval_method": "keyword",
                "total_memories_searched": 3,
                "avg_confidence": 1.0,
                "sources": [{"memory_id": covered_id, "confidence": 1.0}],
            }
        )

        report = analyze_store_health(store)

        assert report["status"] == "warn"
        assert report["memory_count"] == 3
        assert report["expired_count"] == 1
        assert report["low_confidence_count"] == 1
        assert report["risky_source_count"] == 2
        assert report["secret_finding_count"] == 1
        assert report["provenance"]["coverage"] == 1 / 3
        assert "low_confidence" in report["warnings"]
    finally:
        del store
        for suffix in ("", "-wal", "-shm"):
            path = db_path + suffix
            if os.path.exists(path):
                try:
                    os.unlink(path)
                except PermissionError:
                    pass


def test_doctor_cli_prints_health_sections():
    runner = CliRunner()
    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = Path(tmpdir) / "doctor.db"
        store = MemoryStore(str(db_path))
        store.insert_memory("project", "we use FastAPI", "manual")

        os.environ["MEMCTRL_DB_PATH"] = str(db_path)
        try:
            result = runner.invoke(app, ["doctor"])
        finally:
            del os.environ["MEMCTRL_DB_PATH"]

        assert result.exit_code == 0
        assert "Memory Doctor" in result.output
        assert "Memories" in result.output
        assert "Provenance" in result.output
        assert "OpenTelemetry" in result.output
