"""MemCtrl — Shared secret/PII redaction utilities.

Centralizes redaction logic so ALL paths that send data to external
services (LLM APIs, export, etc.) can sanitize before crossing the
process boundary.
"""

from __future__ import annotations

import re

# Secret patterns to redact/detect
_SECRET_PATTERNS = [
    # OpenAI / generic sk- keys (allow hyphens/underscores for Stripe-style keys)
    (r"\b(sk-[a-zA-Z0-9\-_]{20,})\b", "API_KEY"),
    # Generic API key assignments
    (r"\b(api[_-]?key\s*[=:]\s*\S+)", "API_KEY"),
    # Bearer / auth tokens
    (r"\b(bearer\s+\S+)", "TOKEN"),
    (r"\b(auth[_-]?token\s*[=:]\s*\S+)", "TOKEN"),
    (r"\b(access[_-]?token\s*[=:]\s*\S+)", "TOKEN"),
    (r"\b(refresh[_-]?token\s*[=:]\s*\S+)", "TOKEN"),
    # Base64-encoded tokens (40+ chars)
    (r"\b([A-Za-z0-9/+=]{40,})\b", "TOKEN"),
    # Passwords
    (r"\b(password\s*[=:]\s*\S+)", "PASSWORD"),
    # Generic secrets
    (r"\b(secret\s*[=:]\s*\S+)", "SECRET"),
    # AWS keys
    (r"\b(AKIA[0-9A-Z]{16})\b", "AWS_KEY"),
    # Private keys
    (r"-----BEGIN (RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----", "PRIVATE_KEY"),
]

_PII_PATTERNS = [
    (r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b", "EMAIL"),
    (r"\b\d{3}-\d{2}-\d{4}\b", "SSN"),
    (r"\b\d{3}-\d{3}-\d{4}\b", "PHONE"),
    (r"\b\d{10,12}\b", "PHONE_INTL"),
]

_ALL_PATTERNS = _SECRET_PATTERNS + _PII_PATTERNS


def sanitize_text(text: str) -> str:
    """Redact secrets and PII from text.

    This is the canonical redaction function used by:
    - MemoryExtractor (before storage)
    - MemoryTreeBuilder (before LLM clustering prompt)
    - MemoryRetriever (before LLM retrieval prompt)
    - ReflectionEngine (before LLM summary prompt)

    Args:
        text: Raw text that may contain secrets or PII.

    Returns:
        Text with sensitive patterns replaced by [REDACTED_<LABEL>].
    """
    for pattern, label in _ALL_PATTERNS:
        text = re.sub(pattern, f"[REDACTED_{label}]", text, flags=re.I)
    return text


def has_secrets(text: str) -> bool:
    """Check if text contains any secret or PII patterns."""
    for pattern, _ in _ALL_PATTERNS:
        if re.search(pattern, text, re.I):
            return True
    return False
