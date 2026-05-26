"""MemCtrl — Lightweight LLM client wrapper for CLI integration.

Supports OpenAI-compatible APIs via LiteLLM (if installed) or
falls back to direct httpx requests. This enables LLM-powered
tree building and retrieval in the CLI without requiring
the Python API.

Providers: openai, anthropic, gemini, cohere, azure, etc.
"""

from __future__ import annotations

import json
import os
from typing import Any, Callable, Coroutine, Optional


async def _llm_via_litellm(
    prompt: str,
    json_mode: bool = False,
    model: str = "gpt-4o-mini",
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
) -> str:
    """Call LLM via LiteLLM (unified interface for many providers)."""
    try:
        import litellm

        litellm.suppress_debug_info = True
        messages = [{"role": "user", "content": prompt}]
        kwargs: dict = {
            "model": model,
            "messages": messages,
            "temperature": 0.2,
        }
        if api_key:
            kwargs["api_key"] = api_key
        if base_url:
            kwargs["api_base"] = base_url
        if json_mode:
            kwargs["response_format"] = {"type": "json_object"}

        response = await litellm.acompletion(**kwargs)
        content = response.choices[0].message.content
        return content or "{}"
    except Exception as exc:
        raise RuntimeError(f"LiteLLM call failed: {exc}") from exc


async def _llm_via_httpx(
    prompt: str,
    json_mode: bool = False,
    model: str = "gpt-4o-mini",
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
) -> str:
    """Fallback: call OpenAI-compatible API directly via httpx."""
    import httpx

    api_key = api_key or os.environ.get("OPENAI_API_KEY")
    if not api_key:
        raise RuntimeError("No API key provided. Set --llm-api-key or OPENAI_API_KEY.")

    base_url = base_url or os.environ.get("OPENAI_BASE_URL", "https://api.openai.com/v1")

    headers = {
        "Authorization": f"Bearer {api_key}",
        "Content-Type": "application/json",
    }

    payload = {
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.2,
    }
    if json_mode:
        payload["response_format"] = {"type": "json_object"}

    async with httpx.AsyncClient(timeout=60.0) as client:
        resp = await client.post(
            f"{base_url}/chat/completions",
            headers=headers,
            json=payload,
        )
        resp.raise_for_status()
        data = resp.json()
        content = data["choices"][0]["message"]["content"]
        return content or "{}"


def create_llm_client(
    provider: Optional[str] = None,
    model: Optional[str] = None,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
) -> Optional[Callable[[str, bool], Coroutine[Any, Any, str]]]:
    """Create an LLM callable for MemCtrl components.

    Args:
        provider: LLM provider (openai, anthropic, etc.). 'openai' is default.
        model: Model name (e.g., gpt-4o-mini, claude-3-haiku-20240307).
        api_key: API key. Falls back to env vars per provider.
        base_url: Custom API base URL.

    Returns:
        Async callable (prompt, json_mode) -> response string,
        or None if no provider configured.
    """
    if not provider and not model and not api_key:
        # Check env vars for implicit configuration
        if os.environ.get("OPENAI_API_KEY"):
            provider = provider or "openai"
            model = model or os.environ.get("OPENAI_MODEL", "gpt-4o-mini")
        elif os.environ.get("ANTHROPIC_API_KEY"):
            provider = provider or "anthropic"
            model = model or os.environ.get("ANTHROPIC_MODEL", "claude-3-haiku-20240307")
        else:
            return None

    provider = (provider or "openai").lower()
    model = model or "gpt-4o-mini"

    # Resolve API key from env if not provided
    if not api_key:
        env_map = {
            "openai": "OPENAI_API_KEY",
            "anthropic": "ANTHROPIC_API_KEY",
            "gemini": "GEMINI_API_KEY",
            "cohere": "COHERE_API_KEY",
            "azure": "AZURE_OPENAI_API_KEY",
        }
        api_key = os.environ.get(env_map.get(provider, ""))

    # Build model string for LiteLLM
    litellm_model = f"{provider}/{model}" if provider != "openai" else model

    async def client(prompt: str, json_mode: bool = False) -> str:
        try:
            return await _llm_via_litellm(
                prompt, json_mode, model=litellm_model, api_key=api_key, base_url=base_url
            )
        except ImportError:
            # LiteLLM not installed, fall back to direct httpx
            if provider == "openai":
                return await _llm_via_httpx(
                    prompt, json_mode, model=model, api_key=api_key, base_url=base_url
                )
            raise RuntimeError(
                f"LiteLLM not installed. Install with: pip install litellm\n"
                f"Required for provider: {provider}"
            )

    return client
