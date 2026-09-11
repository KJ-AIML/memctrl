"""MemCtrl — Read-Only Heli History Source Adapter & Distiller.

Provides read-only access to Heli-Harness completed task history and distills
candidate reusable lessons with explicit source lineage and counterexamples.

Invariants:
- Strictly read-only against Heli (.heli-harness is never mutated).
- Derived lessons become candidate knowledge in MemCtrl, not active Heli skills.
- Distinguishes similar symptoms with different causes via counterexamples.
"""

from __future__ import annotations

import json
import logging
import re
import uuid
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional

logger = logging.getLogger("memctrl.sources.heli")


@dataclass
class HeliTaskRecord:
    """Authoritative completed task record loaded from a Heli-Harness workspace."""

    task_id: str
    title: str
    status: str
    target_repo: str = ""
    risk_tier: str = "S1"
    completed_at: Optional[datetime] = None
    commit_sha: Optional[str] = None
    plan_steps: List[Dict[str, Any]] = field(default_factory=list)
    evidence_items: List[str] = field(default_factory=list)
    decisions: List[str] = field(default_factory=list)
    diagnosis: Optional[Dict[str, Any]] = None
    raw_content: str = ""

    def to_dict(self) -> Dict[str, Any]:
        return {
            "task_id": self.task_id,
            "title": self.title,
            "status": self.status,
            "target_repo": self.target_repo,
            "risk_tier": self.risk_tier,
            "completed_at": self.completed_at.isoformat() if self.completed_at else None,
            "commit_sha": self.commit_sha,
            "plan_steps": self.plan_steps,
            "evidence_items": self.evidence_items,
            "decisions": self.decisions,
            "diagnosis": self.diagnosis,
        }


@dataclass
class LessonProposal:
    """A proposed reusable lesson distilled from historical tasks."""

    proposal_id: str
    claim: str
    claim_type: str = "derived_lesson"
    source_tasks: List[str] = field(default_factory=list)
    source_revisions: List[str] = field(default_factory=list)
    supporting_evidence: List[str] = field(default_factory=list)
    contradictory_evidence: List[str] = field(default_factory=list)
    applicability_conditions: List[str] = field(default_factory=list)
    counterexamples: List[str] = field(default_factory=list)
    proposed_disposition: str = "candidate"

    def to_dict(self) -> Dict[str, Any]:
        return {
            "proposal_id": self.proposal_id,
            "claim": self.claim,
            "claim_type": self.claim_type,
            "source_tasks": self.source_tasks,
            "source_revisions": self.source_revisions,
            "supporting_evidence": self.supporting_evidence,
            "contradictory_evidence": self.contradictory_evidence,
            "applicability_conditions": self.applicability_conditions,
            "counterexamples": self.counterexamples,
            "proposed_disposition": self.proposed_disposition,
        }


class HeliSourceAdapter:
    """Read-only adapter for extracting completed tasks from Heli-Harness workspace."""

    def __init__(self, workspace_root: Path | str):
        self.workspace_root = Path(workspace_root).resolve()
        self.harness_dir = self.workspace_root / ".heli-harness"

    def is_valid_workspace(self) -> bool:
        """Check if workspace contains a valid .heli-harness directory."""
        return self.harness_dir.exists() and self.harness_dir.is_dir()

    def load_tasks(
        self,
        limit: int = 20,
        status_filter: Optional[str] = "complete",
        repo_filter: Optional[str] = None,
    ) -> List[HeliTaskRecord]:
        """Load task records from .heli-harness/tasks/.

        Strictly read-only; never writes or locks any file.
        """
        if not self.is_valid_workspace():
            logger.warning("Workspace root %s does not contain .heli-harness", self.workspace_root)
            return []

        tasks_dir = self.harness_dir / "tasks"
        if not tasks_dir.exists() or not tasks_dir.is_dir():
            return []

        records: List[HeliTaskRecord] = []
        for task_path in sorted(tasks_dir.iterdir(), key=lambda p: p.stat().st_mtime, reverse=True):
            if not task_path.is_dir():
                continue

            record = self._load_single_task(task_path)
            if not record:
                continue

            if status_filter and record.status.lower() != status_filter.lower():
                continue

            if repo_filter and record.target_repo.lower() != repo_filter.lower():
                continue

            records.append(record)
            if len(records) >= limit:
                break

        return records

    def _load_single_task(self, task_dir: Path) -> Optional[HeliTaskRecord]:
        """Parse files in task_dir into a HeliTaskRecord."""
        task_id = task_dir.name
        title = task_id
        status = "unknown"
        target_repo = ""
        risk_tier = "S1"
        completed_at: Optional[datetime] = None
        commit_sha: Optional[str] = None
        plan_steps: List[Dict[str, Any]] = []
        evidence_items: List[str] = []
        decisions: List[str] = []
        diagnosis: Optional[Dict[str, Any]] = None
        raw_text = []

        # 1. Read task.json if present
        task_json_path = task_dir / "task.json"
        if task_json_path.exists():
            try:
                data = json.loads(task_json_path.read_text(encoding="utf-8"))
                title = data.get("title", title)
                status = data.get("status", status)
                target_repo = (
                    data.get("target", {}).get("repositoryId")
                    or data.get("repo", "")
                )
                risk_tier = data.get("riskTier", risk_tier)
                if data.get("completedAt"):
                    try:
                        completed_at = datetime.fromisoformat(data["completedAt"].replace("Z", "+00:00"))
                    except Exception:
                        pass
                commit_sha = data.get("commitSha")
            except Exception as e:
                logger.debug("Error reading task.json in %s: %e", task_dir, e)

        # 2. Read current-task.md
        current_task_path = task_dir / "current-task.md"
        if current_task_path.exists():
            try:
                ct_text = current_task_path.read_text(encoding="utf-8")
                raw_text.append(ct_text)
                for line in ct_text.splitlines():
                    if line.startswith("Current status:"):
                        status = line.split(":", 1)[1].strip()
                    elif line.startswith("Target repo:"):
                        repo_val = line.split(":", 1)[1].strip()
                        if repo_val:
                            target_repo = repo_val
                    elif line.startswith("Risk tier:"):
                        risk_tier = line.split(":", 1)[1].strip()
            except Exception as e:
                logger.debug("Error reading current-task.md in %s: %e", task_dir, e)

        # 3. Read plan.md
        plan_path = task_dir / "plan.md"
        if plan_path.exists():
            try:
                plan_text = plan_path.read_text(encoding="utf-8")
                raw_text.append(plan_text)
                # Parse step blocks
                steps = re.split(r"^##\s+Step\s+", plan_text, flags=re.MULTILINE)
                for step_block in steps[1:]:
                    lines = step_block.splitlines()
                    header = lines[0] if lines else "Step"
                    step_status = "pending"
                    evidence_line = ""
                    for l in lines[1:]:
                        if l.strip().startswith("Status:"):
                            step_status = l.split(":", 1)[1].strip()
                        elif l.strip().startswith("Evidence:"):
                            evidence_line = l.split(":", 1)[1].strip()
                    plan_steps.append({
                        "name": header,
                        "status": step_status,
                        "evidence": evidence_line,
                    })
                    if evidence_line:
                        evidence_items.append(evidence_line)
            except Exception as e:
                logger.debug("Error reading plan.md in %s: %e", task_dir, e)

        # 4. Read decisions.md
        decisions_path = task_dir / "decisions.md"
        if decisions_path.exists():
            try:
                dec_text = decisions_path.read_text(encoding="utf-8")
                raw_text.append(dec_text)
                for line in dec_text.splitlines():
                    if line.strip().startswith("- ") or line.strip().startswith("* "):
                        decisions.append(line.strip()[2:])
            except Exception as e:
                logger.debug("Error reading decisions.md in %s: %e", task_dir, e)

        # 5. Read diagnosis.json if present
        diag_path = task_dir / "diagnosis.json"
        if diag_path.exists():
            try:
                diagnosis = json.loads(diag_path.read_text(encoding="utf-8"))
            except Exception:
                pass

        return HeliTaskRecord(
            task_id=task_id,
            title=title,
            status=status,
            target_repo=target_repo,
            risk_tier=risk_tier,
            completed_at=completed_at,
            commit_sha=commit_sha,
            plan_steps=plan_steps,
            evidence_items=evidence_items,
            decisions=decisions,
            diagnosis=diagnosis,
            raw_content="\n\n".join(raw_text),
        )


class HeliDistiller:
    """Distills candidate reusable lessons from completed Heli tasks."""

    def __init__(self, adapter: HeliSourceAdapter):
        self.adapter = adapter

    def distill_lessons(
        self,
        tasks: Optional[List[HeliTaskRecord]] = None,
        limit: int = 20,
    ) -> List[LessonProposal]:
        """Analyze completed tasks to extract reusable candidate lessons.

        Identifies recurring failure modes, proven mitigations, and counterexamples.
        """
        task_records = tasks if tasks is not None else self.adapter.load_tasks(limit=limit)
        if not task_records:
            return []

        proposals: List[LessonProposal] = []

        # Analyze task records for diagnostic patterns and recurring themes
        # 1. Look for diagnosis boundaries / contradicted hypotheses
        for task in task_records:
            if task.diagnosis:
                hyp = task.diagnosis.get("active_hypothesis", "")
                contradictions = task.diagnosis.get("contradicted_hypotheses", [])
                closest_boundary = task.diagnosis.get("closest_proven_boundary", "")
                if hyp and closest_boundary:
                    p = LessonProposal(
                        proposal_id=f"prop-{task.task_id[:8]}-diag",
                        claim=f"When encountering {closest_boundary}, verify hypothesis '{hyp}' before costly retries.",
                        claim_type="derived_lesson",
                        source_tasks=[task.task_id],
                        source_revisions=[task.commit_sha] if task.commit_sha else [],
                        supporting_evidence=[f"Closest proven boundary: {closest_boundary}"],
                        contradictory_evidence=[f"Contradicted hypothesis: {c}" for c in contradictions],
                        applicability_conditions=[f"Subsystem matches {task.target_repo}"],
                        counterexamples=[],
                        proposed_disposition="candidate",
                    )
                    proposals.append(p)

            # 2. Extract lessons from durable decisions
            for i, dec in enumerate(task.decisions):
                if len(dec) > 15:
                    p = LessonProposal(
                        proposal_id=f"prop-{task.task_id[:8]}-dec-{i}",
                        claim=dec,
                        claim_type="derived_lesson",
                        source_tasks=[task.task_id],
                        source_revisions=[task.commit_sha] if task.commit_sha else [],
                        supporting_evidence=task.evidence_items[:2],
                        contradictory_evidence=[],
                        applicability_conditions=[f"Repo context: {task.target_repo}"],
                        counterexamples=[],
                        proposed_disposition="candidate",
                    )
                    proposals.append(p)

        # 3. Detect counterexamples across similar symptoms/claims
        self._link_counterexamples(proposals)

        return proposals

    def _link_counterexamples(self, proposals: List[LessonProposal]) -> None:
        """Find lessons that address similar symptoms with different causes."""
        # Simple clustering by common words in claim
        for i, p1 in enumerate(proposals):
            words1 = set(re.findall(r"\w{4,}", p1.claim.lower()))
            for j, p2 in enumerate(proposals):
                if i >= j or p1.source_tasks == p2.source_tasks:
                    continue
                words2 = set(re.findall(r"\w{4,}", p2.claim.lower()))
                overlap = words1 & words2
                if len(overlap) >= 3:
                    # Mark each other as potential counterexamples/divergent causes
                    c1 = f"Task {p2.source_tasks[0]} addressed related symptom with different resolution: '{p2.claim[:80]}'"
                    c2 = f"Task {p1.source_tasks[0]} addressed related symptom with different resolution: '{p1.claim[:80]}'"
                    if c1 not in p1.counterexamples:
                        p1.counterexamples.append(c1)
                    if c2 not in p2.counterexamples:
                        p2.counterexamples.append(c2)

    def persist_proposals_to_store(
        self,
        proposals: List[LessonProposal],
        store: Any,
    ) -> List[str]:
        """Persist proposals into MemCtrl store as candidate knowledge.

        Strictly read-only towards Heli. Writes ONLY to MemCtrl store.
        """
        created_ids: List[str] = []
        for prop in proposals:
            # Build memory content with claim and counterexamples noted
            content = prop.claim
            if prop.counterexamples:
                content += f"\nCounterexamples:\n- " + "\n- ".join(prop.counterexamples)

            mid = store.insert_memory(
                layer="project",
                content=content,
                source="heli-distillation",
                confidence=0.7,
                claim_type="derived_lesson",
                lifecycle_state="candidate",
                verification_state="unverified",
                tags=["distilled", "heli-history", "candidate"],
            )

            # Store external evidence references linking to Heli tasks
            for task_id in prop.source_tasks:
                sha = prop.source_revisions[0] if prop.source_revisions else None
                store.add_memory_evidence(
                    memory_id=mid,
                    source_system="heli",
                    source_id=task_id,
                    source_revision=sha,
                    relation="derived_from",
                    metadata={
                        "proposal_id": prop.proposal_id,
                        "applicability": prop.applicability_conditions,
                    },
                )

            created_ids.append(mid)

        return created_ids
