"""MemCtrl — LLM-powered memory extraction from text.

Extracts structured memories with confidence scoring:
  - Explicit (1.0): "we decided to use FastAPI"
  - Inferred (0.7): "import fastapi" ← inferred from code
  - Mentioned (0.5): "FastAPI was suggested" ← not yet decided

Security: NEVER extracts passwords, API keys, secrets, or PII.
Uses regex patterns for secret detection + redaction.
"""

from __future__ import annotations

import re
import uuid
from datetime import datetime
from typing import Any, Callable, Coroutine, List, Optional

# Type alias
LLMCallable = Callable[[str, bool], Coroutine[Any, Any, str]]

# Secret patterns to redact/detect
_SECRET_PATTERNS = [
    (r"\b(sk-[a-zA-Z0-9]{20,})\b", "API_KEY"),
    (r"\b([A-Za-z0-9/+=]{40,})\b", "TOKEN"),
    (r"\b(password\s*[=:]\s*\S+)", "PASSWORD"),
    (r"\b(secret\s*[=:]\s*\S+)", "SECRET"),
    (r"\b(AKIA[0-9A-Z]{16})\b", "AWS_KEY"),
    (r"-----BEGIN (RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----", "PRIVATE_KEY"),
]

_PII_PATTERNS = [
    (r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b", "EMAIL"),
    (r"\b\d{3}-\d{2}-\d{4}\b", "SSN"),
    (r"\b\d{3}-\d{3}-\d{4}\b", "PHONE"),
    (r"\b\d{10,12}\b", "PHONE_INTL"),
]


class MemoryExtractor:
    """Extract structured memories from text with confidence scoring.

    Distinguishes:
        - Explicit facts (confidence=1.0): "we use FastAPI"
        - Inferred facts (confidence=0.7): "import fastapi" ← from code
        - Mentioned (confidence=0.5): "FastAPI was suggested"

    NEVER extracts passwords, API keys, secrets, or PII.
    """

    def __init__(
        self,
        llm_client: Optional[LLMCallable] = None,
        rules: Optional[Any] = None,
    ):
        self.llm_client = llm_client
        self.rules = rules

    # --- Public API ---

    async def extract(
        self,
        text: str,
        layer: str,
        rules,
    ) -> List[dict]:
        """Extract structured memories from text.

        text: source text (commit message, chat, file content)
        layer: target layer (project/session/user)
        rules: Rules object with confidence levels and forget.never

        Returns list of dicts: {id, layer, content, source, confidence,
                                  created_at, expires_at, tags}
        """
        # 1. Security scan
        if self._has_secrets(text, rules.forget_never):
            text = self._sanitize_text(text)

        # 2. LLM extraction
        if self.llm_client:
            try:
                memories = await self._llm_extract(text, layer, rules)
                if memories:
                    return memories
            except Exception:
                pass  # Fallback to heuristic

        # 3. Fallback heuristic extraction
        return self._fallback_extract(text, layer, rules)

    # --- Security ---

    def _has_secrets(self, text: str, never_list: List[str]) -> bool:
        """Check if text contains forbidden patterns."""
        text_lower = text.lower()
        for pattern in never_list:
            if pattern.lower() in text_lower:
                return True
        for pattern, _ in _SECRET_PATTERNS:
            if re.search(pattern, text, re.I):
                return True
        return False

    def _sanitize_text(self, text: str) -> str:
        """Redact secrets and PII from text."""
        for pattern, label in _SECRET_PATTERNS + _PII_PATTERNS:
            text = re.sub(pattern, f"[REDACTED_{label}]", text, flags=re.I)
        return text

    def _detect_pii(self, text: str) -> List[str]:
        """Detect PII in text. Returns list of found PII types."""
        found = []
        for pattern, label in _PII_PATTERNS:
            if re.search(pattern, text):
                found.append(label)
        return found

    # --- LLM extraction ---

    async def _llm_extract(
        self,
        text: str,
        layer: str,
        rules,
    ) -> List[dict]:
        """Use LLM to extract memories with confidence scoring."""
        prompt = self._build_extraction_prompt(text, layer, rules)
        response = await self.llm_client(prompt, json_mode=True)

        try:
            data = {"memories": []}
            import json as _json

            data = _json.loads(response)
        except Exception:
            return []

        results = []
        for mem in data.get("memories", []):
            content = mem.get("content", "").strip()
            if not content or len(content) < 5:
                continue

            # Final secret check
            if any(p.lower() in content.lower() for p in rules.forget_never):
                continue
            if self._detect_pii(content):
                continue

            confidence = mem.get("confidence", 0.5)
            # Clamp to valid levels
            valid_levels = (
                list(rules.confidence.values()) if rules.confidence else [0.5, 0.7, 1.0]
            )
            if valid_levels and confidence not in valid_levels:
                confidence = min(valid_levels, key=lambda x: abs(x - confidence))

            results.append(
                {
                    "id": str(uuid.uuid4()),
                    "layer": layer,
                    "content": content,
                    "source": "llm_extract",
                    "confidence": confidence,
                    "created_at": datetime.now().isoformat(),
                    "expires_at": None,
                    "tags": mem.get("tags", [layer, "llm-extracted"]),
                }
            )

        return results

    def _build_extraction_prompt(self, text: str, layer: str, rules) -> str:
        """Build LLM prompt for memory extraction."""
        layer_desc = rules.layers.get(layer, layer)
        explicit_c = rules.confidence.get("explicit", 1.0)
        inferred_c = rules.confidence.get("inferred", 0.7)
        mentioned_c = rules.confidence.get("mentioned", 0.5)

        return (
            f"Extract memories from the following text for the '{layer}' layer.\n\n"
            f"Layer definition: {layer_desc}\n\n"
            f"Text:\n{text[:3000]}\n\n"  # Limit to 3K chars
            f"Confidence levels:\n"
            f"  {explicit_c} = explicit statement (e.g., 'we decided to use X')\n"
            f"  {inferred_c} = inferred from context (e.g., 'import X')\n"
            f"  {mentioned_c} = mentioned but not decided\n\n"
            f"NEVER extract: passwords, API keys, secrets, PII.\n\n"
            f"Return ONLY JSON:\n"
            f'{{"memories": [\n'
            f'  {{"content": "fact text", "confidence": {explicit_c}, '
            f'"tags": ["{layer}"]}}\n'
            f"]}}"
        )

    # --- Fallback extraction (no LLM) ---

    def _fallback_extract(self, text: str, layer: str, rules) -> List[dict]:
        """Non-LLM extraction using regex patterns."""
        results = []
        lines = text.split("\n")

        patterns = [
            # Explicit patterns (1.0)
            (
                r"(?i)(we\s+(?:use|use[d]|chose|decided|migrated|switched|implemented)\s+.+)",
                "explicit",
                "tech_choice",
            ),
            (r"(?i)(adr[-\s]?\d+\s*[:\-]?\s*.+)", "explicit", "adr"),
            (r"(?i)(decided\s+to\s+.+)", "explicit", "decision"),
            # Migration patterns
            (
                r"(?i)(migrated?\s+(?:from\s+)?\w+\s+to\s+\w+.+)",
                "explicit",
                "migration",
            ),
            # Inferred patterns (0.7)
            (r"(?i)^\s*(?:import|from)\s+(\w+).+", "inferred", "dependency"),
            (
                r"(?i)(?:built|written|developed)\s+(?:with|on|using)\s+(\w+).+",
                "inferred",
                "framework",
            ),
            # Preference patterns
            (r"(?i)(?:prefer|like|always|never)\s+.+", "explicit", "preference"),
        ]

        for line in lines:
            line = line.strip()
            if len(line) < 10:
                continue
            if len(line) > 500:
                line = line[:500]

            for pattern, level, tag in patterns:
                match = re.search(pattern, line)
                if match:
                    content = match.group(1) if match.groups() else match.group(0)
                    content = content.strip(". ;,\t")
                    if len(content) < 10:
                        continue

                    # Skip if contains secrets
                    if any(p.lower() in content.lower() for p in rules.forget_never):
                        continue
                    if self._detect_pii(content):
                        continue

                    confidence = rules.confidence.get(level, 0.5)
                    results.append(
                        {
                            "id": str(uuid.uuid4()),
                            "layer": layer,
                            "content": content,
                            "source": "heuristic_extract",
                            "confidence": confidence,
                            "created_at": datetime.now().isoformat(),
                            "expires_at": None,
                            "tags": [layer, tag, level],
                        }
                    )
                    break  # One match per line

        # Deduplicate by content similarity
        seen = set()
        deduped = []
        for mem in results:
            key = mem["content"][:50].lower()
            if key not in seen:
                seen.add(key)
                deduped.append(mem)

        return deduped
