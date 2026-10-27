"""LLM provider package: the `LLMProvider` abstraction and its implementations."""

from __future__ import annotations

from typing import Dict

from ..config import BrainConfig
from .base import LLMProvider, LLMResult, ProviderError, ToolCall

__all__ = [
    "LLMProvider",
    "LLMResult",
    "ProviderError",
    "ToolCall",
    "create_provider",
    "available_providers",
]


def available_providers() -> Dict[str, str]:
    """Registry of providers — config keys map to factories (no vendor lock-in)."""
    return {
        "openai_compat": "OpenAI / OpenRouter / LM Studio / Ollama (any /v1 endpoint)",
        "test": "offline deterministic provider (no network, no key)",
    }


def create_provider(cfg: BrainConfig) -> LLMProvider:
    """Instantiate the configured provider.

    Unknown ids fail loudly at startup instead of silently degrading — the doctor
    command reports exactly this.
    """
    provider_id = cfg.llm.provider.strip().lower()
    if provider_id in ("openai_compat", "openai", "openrouter", "lmstudio", "ollama", "local"):
        from .openai_compat import OpenAICompatProvider

        return OpenAICompatProvider(cfg.llm, max_retries=cfg.agent.max_retries)
    if provider_id == "test":
        from .test_provider import TestProvider

        return TestProvider()
    raise ProviderError(
        "bad_config",
        f"unknown llm.provider `{cfg.llm.provider}` — expected one of "
        + ", ".join(available_providers()),
    )