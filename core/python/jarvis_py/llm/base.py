"""Provider abstraction: `LLMProvider` with interchangeable implementations.

The brain never imports a vendor SDK — it talks to this interface only, so the
assistant can be pointed at OpenAI, OpenRouter, LM Studio, Ollama, vLLM or a local
test double without touching orchestration code.
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional


class ProviderError(Exception):
    """A provider-level failure the runtime must report honestly (never swallow).

    `code` mirrors the JSON-RPC codes so the Rust side can classify it:
    `no_api_key`, `auth`, `rate_limited`, `network`, `timeout`, `bad_response`,
    `offline`.
    """

    def __init__(self, code: str, message: str, retryable: bool = False):
        super().__init__(message)
        self.code = code
        self.message = message
        self.retryable = retryable


@dataclass
class ToolCall:
    id: str
    name: str
    arguments: Dict[str, Any]

    def to_wire(self) -> Dict[str, Any]:
        return {"id": self.id, "name": self.name, "arguments": self.arguments}


@dataclass
class LLMResult:
    content: str = ""
    tool_calls: List[ToolCall] = field(default_factory=list)
    model: str = ""
    provider: str = ""
    usage: Dict[str, Any] = field(default_factory=dict)
    finish_reason: str = ""


class LLMProvider(ABC):
    """One language-model backend."""

    #: stable id used in config (`llm.provider`) and logs
    id: str = "abstract"

    @abstractmethod
    def complete(
        self,
        messages: List[Dict[str, Any]],
        tools: Optional[List[Dict[str, Any]]] = None,
        *,
        max_tokens: int = 2048,
        temperature: float = 0.4,
        timeout: float = 60.0,
    ) -> LLMResult:
        """Run one non-streaming completion. Raises `ProviderError` on failure."""

    @abstractmethod
    def health(self) -> Dict[str, Any]:
        """Cheap liveness probe for `jarvis doctor`: {ok, detail, model, provider}."""

    def close(self) -> None:  # pragma: no cover - default no-op
        return None

    def __repr__(self) -> str:  # never leak the key
        return f"<{type(self).__name__} id={self.id}>"