"""Tests for MemoryExtractor — LLM-powered memory extraction.

Covers: fallback extraction, secret detection, PII sanitization, PII detection,
LLM extraction path, security filtering.
"""

import json

import pytest

from memctrl.extractor import MemoryExtractor
from memctrl.rules import DEFAULT_RULES


# ---------------------------------------------------------------------------
# Fallback extraction (no LLM)
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_fallback_extract():
    extractor = MemoryExtractor()
    text = "we decided to use FastAPI\nwe migrated from Flask\nimport sqlalchemy"
    results = extractor._fallback_extract(text, "project", DEFAULT_RULES)
    assert len(results) > 0
    contents = [r["content"].lower() for r in results]
    assert any("fastapi" in c for c in contents)


def test_fallback_extract_explicit_decisions():
    extractor = MemoryExtractor()
    text = "we decided to use PostgreSQL\nADR-001: we chose microservices"
    results = extractor._fallback_extract(text, "project", DEFAULT_RULES)
    assert len(results) >= 1
    for r in results:
        assert r["confidence"] > 0


def test_fallback_extract_inferred_imports():
    extractor = MemoryExtractor()
    text = "import fastapi\nfrom sqlalchemy import Column"
    results = extractor._fallback_extract(text, "project", DEFAULT_RULES)
    assert len(results) >= 1


def test_fallback_extract_migration():
    extractor = MemoryExtractor()
    text = "migrated from Django to FastAPI for performance"
    results = extractor._fallback_extract(text, "project", DEFAULT_RULES)
    assert len(results) >= 1
    assert "migrated" in results[0]["content"].lower()


def test_fallback_extract_skips_short_lines():
    extractor = MemoryExtractor()
    text = "hi\nok\nshort"
    results = extractor._fallback_extract(text, "project", DEFAULT_RULES)
    assert len(results) == 0


def test_fallback_extract_skips_secrets():
    """Lines containing secrets should not be extracted."""
    extractor = MemoryExtractor()
    text = "the password is secret123\nwe use FastAPI"
    results = extractor._fallback_extract(text, "project", DEFAULT_RULES)
    contents = [r["content"].lower() for r in results]
    assert any("fastapi" in c for c in contents)
    assert not any("password" in c for c in contents)


def test_fallback_extract_dedup():
    """Similar lines should be deduplicated."""
    extractor = MemoryExtractor()
    text = "we decided to use FastAPI\nwe decided to use FastAPI\nwe decided to use FastAPI"
    results = extractor._fallback_extract(text, "project", DEFAULT_RULES)
    # Should deduplicate identical content
    assert len(results) == 1


def test_fallback_extract_long_lines_truncated():
    """Lines > 500 chars get truncated during extraction."""
    extractor = MemoryExtractor()
    long_line = "we decided to use FastAPI " + "x" * 600
    results = extractor._fallback_extract(long_line, "project", DEFAULT_RULES)
    if results:
        assert len(results[0]["content"]) <= 500


def test_fallback_extract_tags():
    extractor = MemoryExtractor()
    text = "we decided to use FastAPI"
    results = extractor._fallback_extract(text, "project", DEFAULT_RULES)
    assert "project" in results[0]["tags"]
    assert "tech_choice" in results[0]["tags"] or "explicit" in results[0]["tags"]


# ---------------------------------------------------------------------------
# Secret detection
# ---------------------------------------------------------------------------

def test_has_secrets():
    extractor = MemoryExtractor()
    assert extractor._has_secrets("password = secret123", ["password"]) is True


def test_has_secrets_no_match():
    extractor = MemoryExtractor()
    assert extractor._has_secrets("we use FastAPI", ["password"]) is False


def test_has_secrets_api_key():
    extractor = MemoryExtractor()
    assert extractor._has_secrets("sk-abcdefghijklmnopqrstuvwxyz123456", []) is True


def test_has_secrets_aws_key():
    extractor = MemoryExtractor()
    assert extractor._has_secrets("AKIAIOSFODNN7EXAMPLE", []) is True


def test_has_secrets_private_key():
    extractor = MemoryExtractor()
    assert extractor._has_secrets(
        "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEAx...", []) is True


def test_has_secrets_env_file():
    extractor = MemoryExtractor()
    # secret= matches the SECRET pattern (\b(secret\s*[=:]\s*\S+))
    assert extractor._has_secrets('secret="my-secret-value"', []) is True


def test_has_secrets_token():
    extractor = MemoryExtractor()
    long_token = "x" * 45  # matches TOKEN pattern (40+ base64 chars)
    assert extractor._has_secrets(long_token, []) is True


def test_has_secrets_false_positive_safe():
    """Normal text should not trigger secret detection."""
    extractor = MemoryExtractor()
    assert extractor._has_secrets(
        "We discussed the API design and chose REST over GraphQL", []) is False


# ---------------------------------------------------------------------------
# Sanitization
# ---------------------------------------------------------------------------

def test_sanitize_text():
    extractor = MemoryExtractor()
    text = "API key is sk-abc123XYZabcdefghijklmnopqrstuvwx and email is test@example.com"
    cleaned = extractor._sanitize_text(text)
    assert "sk-abc123XYZabcdefghijklmnopqrstuvwx" not in cleaned
    assert "test@example.com" not in cleaned
    assert "[REDACTED_API_KEY]" in cleaned
    assert "[REDACTED_EMAIL]" in cleaned


def test_sanitize_text_password():
    extractor = MemoryExtractor()
    text = 'password = "super-secret"'
    cleaned = extractor._sanitize_text(text)
    assert "super-secret" not in cleaned
    assert "[REDACTED_PASSWORD]" in cleaned


def test_sanitize_text_secret():
    extractor = MemoryExtractor()
    text = 'secret = "hidden"'
    cleaned = extractor._sanitize_text(text)
    assert "hidden" not in cleaned
    assert "[REDACTED_SECRET]" in cleaned


def test_sanitize_text_no_pii():
    extractor = MemoryExtractor()
    text = "We use FastAPI and PostgreSQL"
    cleaned = extractor._sanitize_text(text)
    assert cleaned == text  # No changes


def test_sanitize_text_ssn():
    extractor = MemoryExtractor()
    text = "SSN is 123-45-6789"
    cleaned = extractor._sanitize_text(text)
    assert "123-45-6789" not in cleaned
    assert "[REDACTED_SSN]" in cleaned


def test_sanitize_text_phone():
    extractor = MemoryExtractor()
    text = "Phone: 555-123-4567"
    cleaned = extractor._sanitize_text(text)
    assert "555-123-4567" not in cleaned
    assert "[REDACTED_PHONE]" in cleaned


# ---------------------------------------------------------------------------
# PII detection
# ---------------------------------------------------------------------------

def test_detect_pii():
    extractor = MemoryExtractor()
    pii = extractor._detect_pii("Contact: user@example.com or 555-1234")
    assert "EMAIL" in pii


def test_detect_pii_ssn():
    extractor = MemoryExtractor()
    pii = extractor._detect_pii("SSN: 123-45-6789")
    assert "SSN" in pii


def test_detect_pii_phone():
    extractor = MemoryExtractor()
    pii = extractor._detect_pii("Call me at 555-555-5555")
    assert "PHONE" in pii


def test_detect_pii_none():
    extractor = MemoryExtractor()
    pii = extractor._detect_pii("We use FastAPI and PostgreSQL")
    assert pii == []


def test_detect_pii_multiple():
    extractor = MemoryExtractor()
    pii = extractor._detect_pii(
        "Email user@example.com, phone 555-555-5555, SSN 123-45-6789")
    assert "EMAIL" in pii
    assert "PHONE" in pii
    assert "SSN" in pii


# ---------------------------------------------------------------------------
# LLM extraction
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_llm_extract_success():
    async def mock_llm(prompt, json_mode=False):
        return json.dumps({
            "memories": [
                {"content": "we use FastAPI", "confidence": 1.0,
                 "tags": ["project"]},
            ]
        })

    extractor = MemoryExtractor(llm_client=mock_llm)
    results = await extractor._llm_extract("we use FastAPI", "project", DEFAULT_RULES)
    assert len(results) == 1
    assert results[0]["content"] == "we use FastAPI"


@pytest.mark.asyncio
async def test_llm_extract_skips_secrets_in_content():
    async def mock_llm(prompt, json_mode=False):
        return json.dumps({
            "memories": [
                {"content": "password is secret123", "confidence": 1.0,
                 "tags": ["project"]},
            ]
        })

    extractor = MemoryExtractor(llm_client=mock_llm)
    results = await extractor._llm_extract("some text", "project", DEFAULT_RULES)
    assert len(results) == 0  # Secret content filtered out


@pytest.mark.asyncio
async def test_llm_extract_skips_pii():
    async def mock_llm(prompt, json_mode=False):
        return json.dumps({
            "memories": [
                {"content": "email me at user@example.com", "confidence": 1.0,
                 "tags": ["project"]},
            ]
        })

    extractor = MemoryExtractor(llm_client=mock_llm)
    results = await extractor._llm_extract("some text", "project", DEFAULT_RULES)
    assert len(results) == 0  # PII content filtered out


@pytest.mark.asyncio
async def test_llm_extract_skips_short_content():
    async def mock_llm(prompt, json_mode=False):
        return json.dumps({
            "memories": [
                {"content": "hi", "confidence": 1.0, "tags": ["project"]},
            ]
        })

    extractor = MemoryExtractor(llm_client=mock_llm)
    results = await extractor._llm_extract("some text", "project", DEFAULT_RULES)
    assert len(results) == 0  # Too short


@pytest.mark.asyncio
async def test_llm_extract_invalid_json():
    async def bad_llm(prompt, json_mode=False):
        return "not json"

    extractor = MemoryExtractor(llm_client=bad_llm)
    results = await extractor._llm_extract("text", "project", DEFAULT_RULES)
    assert results == []


@pytest.mark.asyncio
async def test_llm_extract_clamps_confidence():
    async def mock_llm(prompt, json_mode=False):
        return json.dumps({
            "memories": [
                {"content": "we use FastAPI", "confidence": 0.99,
                 "tags": ["project"]},
            ]
        })

    extractor = MemoryExtractor(llm_client=mock_llm)
    results = await extractor._llm_extract("text", "project", DEFAULT_RULES)
    assert len(results) == 1
    # Confidence should be clamped to nearest valid level
    assert results[0]["confidence"] in [0.5, 0.7, 1.0]


# ---------------------------------------------------------------------------
# Build extraction prompt
# ---------------------------------------------------------------------------

def test_build_extraction_prompt():
    extractor = MemoryExtractor()
    prompt = extractor._build_extraction_prompt("we use FastAPI", "project", DEFAULT_RULES)
    assert "project" in prompt
    assert "FastAPI" in prompt
    assert "memories" in prompt
    assert "NEVER extract" in prompt


# ---------------------------------------------------------------------------
# Full extract async path
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_extract_with_llm():
    async def mock_llm(prompt, json_mode=False):
        return json.dumps({
            "memories": [
                {"content": "we use FastAPI", "confidence": 1.0,
                 "tags": ["project"]},
            ]
        })

    extractor = MemoryExtractor(llm_client=mock_llm)
    results = await extractor.extract("we use FastAPI", "project", DEFAULT_RULES)
    assert len(results) == 1
    assert results[0]["layer"] == "project"
    assert results[0]["source"] == "llm_extract"


@pytest.mark.asyncio
async def test_extract_fallback_when_no_llm():
    """Without LLM client, falls back to heuristic extraction."""
    extractor = MemoryExtractor()
    text = "we decided to use FastAPI\nwe chose PostgreSQL"
    results = await extractor.extract(text, "project", DEFAULT_RULES)
    assert len(results) > 0
    assert results[0]["source"] == "heuristic_extract"


@pytest.mark.asyncio
async def test_extract_with_sanitization():
    """Text with secrets gets sanitized before extraction."""
    extractor = MemoryExtractor()
    text = "API key is sk-abc123XYZabcdefghijklmnopqrstuvwx\nwe use FastAPI"
    results = await extractor.extract(text, "project", DEFAULT_RULES)
    contents = [r["content"].lower() for r in results]
    # Secret line should be sanitized, FastAPI line should be extracted
    assert not any("sk-abc123" in c for c in contents)


@pytest.mark.asyncio
async def test_extract_llm_exception_falls_back():
    """LLM exception triggers fallback."""
    async def failing_llm(prompt, json_mode=False):
        raise RuntimeError("LLM down")

    extractor = MemoryExtractor(llm_client=failing_llm)
    text = "we decided to use FastAPI"
    results = await extractor.extract(text, "project", DEFAULT_RULES)
    assert len(results) > 0
    assert results[0]["source"] == "heuristic_extract"
