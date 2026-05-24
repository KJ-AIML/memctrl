"""MemCtrl — Reflection Engine.

Auto-detects session end and triggers consolidation without manual command.

Detection heuristics (implement ALL, use whichever fires first):
- Time-based: no memory activity for > 2 hours → session likely ended
- Git-based: git commit detected → fire on_commit + on_session_end
- Explicit: memctrl done shorthand → immediate consolidation

When reflection fires:
1. Summarize session layer memories using LLM (if available) or heuristic
2. Create new project/user layer memories with source='reflection'
3. Mark session memories as consolidated
4. Log trigger execution
"""

from __future__ import annotations

import subprocess
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from typing import Callable, List, Optional

from memctrl.store import MemoryStore
from memctrl.rules import RuleEngine


@dataclass
class ReflectionResult:
    """Result of a reflection operation.

    Tracks what triggered the reflection, which memories were consolidated,
    and what new memories were created. Used by CLI commands to show
    users a summary of what happened during auto-consolidation.
    """

    triggered: bool
    event: str = ""
    consolidated_ids: List[str] = field(default_factory=list)
    new_memories: List[dict] = field(default_factory=list)
    summary: str = ""
    timestamp: datetime = field(default_factory=datetime.now)

    def to_dict(self) -> dict:
        """Serialize result for logging or API responses.

        Converts datetime to ISO format so the result can be JSON-encoded.
        """
        return {
            "triggered": self.triggered,
            "event": self.event,
            "consolidated_ids": self.consolidated_ids,
            "new_memories": self.new_memories,
            "summary": self.summary,
            "timestamp": self.timestamp.isoformat() if self.timestamp else None,
        }


class ReflectionEngine:
    """Auto-detects session end and triggers consolidation.

    The reflection engine solves the problem of users forgetting to run
    ``memctrl trigger on_session_end`` by detecting session-end conditions
    automatically. When a session ends, valuable context (what was built,
    decisions made, problems solved) should be preserved in project/user
    layers instead of expiring with the session layer.

    Detection heuristics (checked in order of confidence):
    1. Explicit: ``memctrl done`` command called — highest confidence
    2. Git-based: git commit detected in current directory — strong signal
    3. Time-based: no activity for N hours (default: 2) — weakest but catches all

    Usage:
        engine = ReflectionEngine(store)
        result = engine.check_and_reflect()
        if result.triggered:
            print(f"Consolidated {len(result.consolidated_ids)} memories")
    """

    def __init__(
        self,
        store: MemoryStore,
        engine: Optional[RuleEngine] = None,
        inactivity_hours: float = 2.0,
        llm_client: Optional[Callable] = None,
    ):
        """Initialize ReflectionEngine.

        Args:
            store: MemoryStore instance for reading/writing memories
            engine: RuleEngine instance (created with defaults if None)
            inactivity_hours: Hours of inactivity before auto-reflection triggers
            llm_client: Optional callable for generating summaries.
                Called as llm_client(prompt: str) -> str
        """
        self.store = store
        self.engine = engine or RuleEngine()
        self.inactivity_hours = inactivity_hours
        self.llm_client = llm_client

    def check_and_reflect(self, force: bool = False) -> ReflectionResult:
        """Check if reflection should trigger and execute if so.

        Checks heuristics in order of confidence (explicit > git > time).
        The first heuristic that fires wins — we don't run consolidation
        multiple times for the same session.

        Args:
            force: If True, trigger reflection regardless of heuristics.
                Used by the ``memctrl done`` command for explicit consolidation.

        Returns:
            ReflectionResult with details of what happened. If no heuristic
            fired and force is False, returns triggered=False.
        """
        if force:
            return self._consolidate("explicit")

        if self._check_git_commit():
            return self._consolidate("on_commit")

        if self._check_time_based():
            return self._consolidate("on_session_end")

        return ReflectionResult(triggered=False)

    def _check_time_based(self) -> bool:
        """Check if enough time has passed since last activity.

        Looks at the most recent memory's created_at timestamp across all
        layers. If no memories exist, returns False (no session to consolidate).

        Returns True if inactivity_hours have passed since last memory.
        """
        last_activity = self.get_last_activity()
        if last_activity is None:
            return False

        threshold = datetime.now() - timedelta(hours=self.inactivity_hours)
        return last_activity < threshold

    def _check_git_commit(self) -> bool:
        """Check if a git commit has been made recently.

        Runs ``git log -1 --since="{inactivity_hours} hours ago"`` in the
        current directory. A commit is a strong signal that the user has
        checkpointed their work and may be ending the session.

        Returns True if a commit was found within the inactivity window.
        Returns False if not in a git repo or no recent commits.
        """
        try:
            since = f"{int(self.inactivity_hours)} hours ago"
            result = subprocess.run(
                ["git", "log", "-1", f"--since={since}", "--oneline"],
                capture_output=True,
                text=True,
                timeout=5,
            )
            return result.returncode == 0 and result.stdout.strip() != ""
        except (subprocess.TimeoutExpired, FileNotFoundError, OSError):
            return False

    def _consolidate(self, event: str) -> ReflectionResult:
        """Execute consolidation for the given event.

        This is the core reflection workflow:
        1. Get all session-layer memories
        2. Generate a summary of what happened this session
        3. Fire the matching trigger rule (e.g., on_session_end, on_commit)
        4. Create reflection-sourced memories in target layers
        5. Log the trigger execution for audit

        Args:
            event: The trigger event name (e.g., "explicit", "on_commit",
                "on_session_end")

        Returns:
            ReflectionResult with consolidated IDs, new memories, and summary.
        """
        session_memories = self.store.list_memories(layer="session")

        if not session_memories:
            return ReflectionResult(
                triggered=True,
                event=event,
                summary="No session memories to consolidate",
            )

        mem_dicts = [m.to_dict() for m in session_memories]
        summary = self._generate_summary(mem_dicts)

        # Ensure rules are loaded before firing triggers
        self.engine.load()

        # Fire trigger rule — this performs the actual consolidation
        consolidated_ids = self.engine.fire_trigger(
            event, {"summary": summary}, self.store
        )

        # If the trigger didn't match any rule patterns, fall back to
        # default session -> project consolidation so reflection always
        # does something useful.
        if not consolidated_ids:
            consolidated_ids = self.store.consolidate("session", "project")

        # Create a reflection memory in the project layer with the summary
        new_memories: List[dict] = []
        if summary:
            rid = self.store.insert_memory(
                layer="project",
                content=f"Session reflection ({event}): {summary}",
                source="reflection",
                confidence=0.9,
                tags=["reflection", event, "auto-consolidated"],
            )
            new_memories.append(
                {
                    "id": rid,
                    "layer": "project",
                    "content": summary,
                    "source": "reflection",
                }
            )

        # Log trigger execution for audit trail
        self.store.log_trigger(event, "reflection_consolidate", consolidated_ids)

        return ReflectionResult(
            triggered=True,
            event=event,
            consolidated_ids=consolidated_ids,
            new_memories=new_memories,
            summary=summary,
        )

    def _generate_summary(self, memories: List[dict]) -> str:
        """Generate a summary of session memories.

        Uses the LLM client if available (produces higher-quality summaries
        that capture context and relationships between memories). Falls back
        to a simple heuristic join that concatenates memory contents with
        bullet points.

        Args:
            memories: List of memory dicts (from Memory.to_dict())

        Returns:
            A summary string describing the session.
        """
        if not memories:
            return ""

        if self.llm_client is not None:
            try:
                lines = [f"- {m.get('content', '')}" for m in memories]
                prompt = (
                    "Summarize the following session memories into a concise "
                    "paragraph (2-3 sentences) capturing what was accomplished:\n\n"
                    + "\n".join(lines)
                )
                summary = self.llm_client(prompt)
                if summary and isinstance(summary, str):
                    return summary.strip()
            except Exception:
                # LLM failed — fall through to heuristic
                pass

        # Heuristic fallback: join distinct memory contents
        contents: List[str] = []
        seen: set = set()
        for m in memories:
            content = m.get("content", "")
            if content and content not in seen:
                contents.append(content)
                seen.add(content)

        if len(contents) == 1:
            return contents[0]

        return "; ".join(contents)

    def get_last_activity(self) -> Optional[datetime]:
        """Get timestamp of most recent memory or trigger activity.

        Looks across all memories and trigger logs to find the most recent
        activity. This is used by the time-based heuristic to decide if
        enough idle time has passed.

        Returns:
            datetime of most recent activity, or None if no activity exists.
        """
        latest: Optional[datetime] = None

        # Check memory timestamps
        all_memories = self.store.list_memories()
        for mem in all_memories:
            if mem.created_at:
                if latest is None or mem.created_at > latest:
                    latest = mem.created_at

        # Check trigger log timestamps
        try:
            logs = self.store.get_trigger_log(limit=1)
            for log in logs:
                if log.timestamp:
                    if latest is None or log.timestamp > latest:
                        latest = log.timestamp
        except Exception:
            pass

        return latest
