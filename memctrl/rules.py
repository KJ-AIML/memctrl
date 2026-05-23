"""MemCtrl — .memoryrc parser and rule engine.

Uses TOML for configuration (native tomllib in Python 3.11+).
Implements hot-reload via watchdog and trigger execution.

Research: TOML is the best format for .memoryrc — native Python support,
clean syntax for rules, no external parser needed on 3.11+.
"""

from __future__ import annotations

import copy
import os
import re
import sys
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Dict, List, Optional

# ---------------------------------------------------------------------------
# TOML parser compatibility
# ---------------------------------------------------------------------------

if sys.version_info >= (3, 11):
    import tomllib
else:
    try:
        import tomli as tomllib
    except ImportError:
        tomllib = None

# ---------------------------------------------------------------------------
# Data model
# ---------------------------------------------------------------------------

@dataclass
class Rules:
    """Normalized .memoryrc configuration."""

    layers: Dict[str, str] = field(default_factory=dict)
    triggers: Dict[str, str] = field(default_factory=dict)
    forget_never: List[str] = field(default_factory=list)
    forget_after_days: Dict[str, int] = field(default_factory=dict)
    confidence: Dict[str, float] = field(default_factory=dict)

    def get_ttl_days(self, layer: str) -> Optional[int]:
        return self.forget_after_days.get(layer)


# ---------------------------------------------------------------------------
# Default rules (used when .memoryrc does not exist)
# ---------------------------------------------------------------------------

DEFAULT_RULES = Rules(
    layers={
        "project": "architecture decisions, tech stack, ADRs, why we chose X",
        "session": "current task, WIP, what was done this session",
        "user": "preferences, working style, patterns, personal rules",
    },
    triggers={
        "on_commit": "consolidate session -> project",
        "on_session_end": "summarize session -> user",
        'on_file "docs/ADR-*.md"': "extract -> project",
        'on_file "*.md"': "extract -> project if contains decision",
    },
    forget_never=["passwords", "keys", "PII", "secrets", "api_key",
                   "token", "secret", "password"],
    forget_after_days={"session": 7, "user": 90},
    confidence={"explicit": 1.0, "inferred": 0.7, "mentioned": 0.5},
)


# ---------------------------------------------------------------------------
# Rule engine
# ---------------------------------------------------------------------------

class RuleEngine:
    """Parse .memoryrc (TOML), validate, and execute rules.

    Supports hot-reload via watchdog (optional dependency).
    """

    def __init__(self, rc_path: str = ".memoryrc"):
        self.rc_path = Path(rc_path)
        self.rules: Rules = copy.deepcopy(DEFAULT_RULES)
        self._watching = False

    # --- Loading ---

    def load(self) -> Rules:
        """Parse .memoryrc TOML. Return Rules (falls back to defaults)."""
        if not self.rc_path.exists():
            self.rules = copy.deepcopy(DEFAULT_RULES)
            return self.rules

        if tomllib is None:
            raise RuntimeError(
                "TOML parsing requires Python 3.11+ or 'tomli' package. "
                "Install: pip install tomli"
            )

        try:
            with open(self.rc_path, "rb") as f:
                data = tomllib.load(f)
        except Exception as exc:
            raise ValueError(f"Failed to parse {self.rc_path}: {exc}") from exc

        rules = copy.deepcopy(DEFAULT_RULES)

        # [layers]
        if "layers" in data:
            for layer_name, desc in data["layers"].items():
                rules.layers[layer_name] = desc

        # [triggers] — handle both formats:
        # compact: 'on_file "*.md"' = "extract -> project"
        # flat:    on_file = "*.md -> extract -> project"
        if "triggers" in data:
            raw_triggers: Dict[str, Any] = {}
            for key, val in data["triggers"].items():
                if isinstance(val, str):
                    raw_triggers[key] = val
                elif isinstance(val, dict):
                    for sub_key, sub_val in val.items():
                        raw_triggers[f'{key} "{sub_key}"'] = sub_val
            rules.triggers.update(raw_triggers)

        # [forget]
        if "forget" in data:
            forget = data["forget"]
            if "never" in forget:
                rules.forget_never = forget["never"]
            if "after_days" in forget:
                rules.forget_after_days = dict(forget["after_days"])

        # [extract]
        if "extract" in data:
            extract = data["extract"]
            if "confidence" in extract:
                rules.confidence = {k: float(v)
                                    for k, v in extract["confidence"].items()}

        self.rules = rules
        return rules

    def reload(self) -> Rules:
        """Reload rules from disk."""
        return self.load()

    # --- Hot reload ---

    def watch(self) -> None:
        """Start watchdog to auto-reload .memoryrc on change."""
        if self._watching:
            return
        try:
            from watchdog.observers import Observer
            from watchdog.events import FileSystemEventHandler, FileModifiedEvent
        except ImportError:
            return  # watchdog not installed — skip silently

        class _Handler(FileSystemEventHandler):
            def __init__(self, engine: RuleEngine):
                self.engine = engine

            def on_modified(self, event) -> None:
                if isinstance(event, FileModifiedEvent):
                    p = Path(event.src_path)
                    if p.name == self.engine.rc_path.name:
                        self.engine.reload()

        self._handler = _Handler(self)
        self._observer = Observer()
        watch_dir = self.rc_path.parent if self.rc_path.exists() else Path.cwd()
        self._observer.schedule(self._handler, str(watch_dir), recursive=False)
        self._observer.start()
        self._watching = True

    def stop_watch(self) -> None:
        if self._observer:
            self._observer.stop()
            self._observer.join()
            self._watching = False

    # --- Trigger execution ---

    def fire_trigger(self, event: str, context: dict, store) -> List[str]:
        """Execute matching trigger rule. Return affected memory IDs.

        Parse actions like 'consolidate session -> project'.
        """
        matched_ids: List[str] = []

        for pattern, action in self.rules.triggers.items():
            # Simple substring match for event name
            if event.lower() in pattern.lower():
                parsed = self._parse_action(action)
                if parsed:
                    ids = self._execute_action(parsed, context, store)
                    matched_ids.extend(ids)
                    # Log trigger execution
                    store.log_trigger(event, action, ids)

        return matched_ids

    def _parse_action(self, action: str) -> Optional[Dict[str, str]]:
        """Parse action string into structured dict."""
        action = action.strip()

        # consolidate X -> Y
        m = re.match(r"consolidate\s+(\w+)\s*[-]+>\s*(\w+)", action, re.I)
        if m:
            return {"verb": "consolidate", "from": m.group(1), "to": m.group(2)}

        # summarize X -> Y
        m = re.match(r"summarize\s+(\w+)\s*[-]+>\s*(\w+)", action, re.I)
        if m:
            return {"verb": "summarize", "from": m.group(1), "to": m.group(2)}

        # extract -> layer
        m = re.match(r"extract\s*[-]+>\s*(\w+)", action, re.I)
        if m:
            return {"verb": "extract", "to": m.group(1)}

        m = re.match(r"extract\s*[-]+>\s*(\w+)\s+if\s+(.+)", action, re.I)
        if m:
            return {"verb": "extract", "to": m.group(1), "condition": m.group(2)}

        return {"verb": "unknown", "raw": action}

    def _execute_action(self, parsed: dict, context: dict, store) -> List[str]:
        verb = parsed.get("verb", "")
        if verb == "consolidate":
            return store.consolidate(parsed["from"], parsed["to"])
        elif verb == "summarize":
            # For now: consolidate + mark as summarized
            return store.consolidate(parsed["from"], parsed["to"])
        elif verb == "extract":
            # Extract is handled by extractor module
            return []
        return []

    # --- Forget rules ---

    def should_forget(self, memory, rules: Optional[Rules] = None) -> bool:
        """Check if a memory should be forgotten based on rules."""
        r = rules or self.rules

        # Never forget items matching forget.never
        content_lower = memory.content.lower()
        for pattern in r.forget_never:
            if pattern.lower() in content_lower:
                return False

        # Check TTL
        ttl = r.get_ttl_days(memory.layer)
        if ttl is None:
            return False
        if memory.expires_at is None:
            return False

        return datetime.now() > memory.expires_at

    # --- Extraction helpers ---

    def extract_memories(self, text: str, layer: str, rules: Rules) -> List[dict]:
        """Baseline text extraction using heuristics.

        Returns list of dicts with content, confidence, source, tags.
        For full LLM extraction, use MemoryExtractor.
        """
        results = []
        lines = text.split("\n")
        for line in lines:
            line = line.strip()
            if len(line) < 10:
                continue

            confidence = self._heuristic_confidence(line, rules)
            if confidence >= 0.5:
                results.append({
                    "content": line,
                    "confidence": confidence,
                    "source": "heuristic",
                    "tags": [layer, "auto-extracted"],
                })

        return results

    def _heuristic_confidence(self, line: str, rules: Rules) -> float:
        """Score a line's confidence based on explicit indicators."""
        explicit_markers = [
            r"we\s+(use|use[d]|chose|decided|migrated|switched|implemented)",
            r"adr[-\s]?\d+",
            r"decided\s+to\s+",
            r"architecture\s+decision",
            r"tech\s+stack",
        ]
        for pattern in explicit_markers:
            if re.search(pattern, line, re.I):
                return rules.confidence.get("explicit", 1.0)

        inferred_markers = [
            r"import\s+\w+",
            r"from\s+\w+\s+import",
            r"uses?\s+\w+",
            r"built\s+(with|on)\s+",
        ]
        for pattern in inferred_markers:
            if re.search(pattern, line, re.I):
                return rules.confidence.get("inferred", 0.7)

        # Check for mention patterns
        mentioned_markers = [
            r"(consider|considering|evaluating|looking at|might|maybe)",
            r"(suggested|proposed|idea)",
        ]
        for pattern in mentioned_markers:
            if re.search(pattern, line, re.I):
                return rules.confidence.get("mentioned", 0.5)

        return 0.0

    def compute_expiry(self, layer: str) -> Optional[datetime]:
        """Compute expiry datetime for a given layer."""
        days = self.rules.get_ttl_days(layer)
        if days is None:
            return None
        return datetime.now() + timedelta(days=days)
