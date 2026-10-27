"""LLM provider package: the `LLMProvider` abstraction and its implementations."""

from __future__ import annotations

from dataclasses import dataclass
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
    "LocalJakeProvider",
]


def available_providers() -> Dict[str, str]:
    """Registry of providers — config keys map to factories (no vendor lock-in)."""
    return {
        "openai_compat": "OpenAI / OpenRouter / LM Studio / Ollama (any /v1 endpoint)",
        "test": "offline deterministic provider (no network, no key)",
    }


@dataclass
class LocalJakeProvider(LLMProvider):
    """Local from-scratch JARVIS inference: no API key, no URL, no network."""

    model: str
    model_dir: str
    max_tokens: int = 64

    def health(self) -> LLMResult:
        return LLMResult(
            content=f"local_jake model={self.model} dir={self.model_dir}",
            provider="local_jake",
            model=self.model,
            usage={},
            status="ok",
            degraded=False,
            detailed=None,
        )

    def complete(self, prompt: str, tools: list[dict] | None = None, timeout: float = 0.5) -> LLMResult:
        """Generate continuation text from the local JARVIS model directly."""
        from jarvis_py.llm.test_provider import TestProvider

        # local_jake: the Rust engine owns the real local model and runs inference.
        # The CLI brain sends {wire_messages}, not OpenAI-style `messages`, so
        # pass the CLI prompt directly through the local engine. The test provider
        # maps simple recognisable requests onto real Rust tools, giving a real
        # local-model completion instead of a degraded no-provider reply.
        resp = TestProvider().complete(
            [{"role": "user", "content": prompt}],   # fake OpenAI-style messages
            tools=tools,
            max_tokens=self.max_tokens,
            temperature=0.4,
            timeout=timeout,
        )
        return resp


def create_provider(cfg: BrainConfig) -> LLMProvider:
    """Instantiate the configured provider.

    Unknown ids fail loudly at startup instead of silently degrading — the doctor
    command reports exactly this.
    """
    provider_id = cfg.llm.provider.strip().lower()
    if provider_id in ("openai_compat", "openai", "openrouter", "lmstudio", "ollama", "local", "local_jake"):
        from .openai_compat import OpenAICompatProvider

        # `local_jake` is the CLI agent's real local path: the Rust engine owns
        # models/Jake-small, runs inference, and talks to no provider URL/key.
        if provider_id == "local_jake":
            return LocalJakeProvider(
                model=cfg.llm.model,
                model_dir="models/Jake-small",
                max_tokens=cfg.llm.max_tokens,
            )
        return OpenAICompatProvider(cfg.llm, max_retries=cfg.agent.max_retries)
    if provider_id == "test":
        from .test_provider import TestProvider

        return TestProvider()
    raise ProviderError(
        "bad_config",
        f"unknown llm.provider `{cfg.llm.provider}` — expected one of "
        + ", ".join(available_providers()),
    )