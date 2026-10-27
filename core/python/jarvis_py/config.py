"""Typed brain configuration, loaded from the `Jake_BRAIN_CONFIG` environment variable.

The Rust runtime owns configuration; the brain receives an already-resolved slice of
it on every launch. Environment variables are the fallback for standalone use
(`python -m Jake_py.main` during development).
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass, field
from typing import Any, Dict


@dataclass
class LlmConfig:
    provider: str = "openai_compat"
    base_url: str = "https://api.openai.com/v1"
    model: str = "gpt-4o-mini"
    api_key: str = ""
    timeout_ms: int = 60_000
    max_tokens: int = 2048
    temperature: float = 0.4

    @property
    def timeout_s(self) -> float:
        return max(1.0, self.timeout_ms / 1000.0)


@dataclass
class AgentConfig:
    max_steps: int = 8
    max_retries: int = 2


@dataclass
class PersonaConfig:
    name: str = "Jake"
    style: str = "calm, professional, concise, occasionally witty, never annoying"
    language: str = "en"
    wake_word: str = "Jake"


@dataclass
class PrivacyConfig:
    privacy_mode: bool = False
    allow_screenshots: bool = False
    allow_cloud_llm: bool = True

    @property
    def cloud_allowed(self) -> bool:
        return not self.privacy_mode and self.allow_cloud_llm


@dataclass
class BrainConfig:
    llm: LlmConfig = field(default_factory=LlmConfig)
    agent: AgentConfig = field(default_factory=AgentConfig)
    persona: PersonaConfig = field(default_factory=PersonaConfig)
    privacy: PrivacyConfig = field(default_factory=PrivacyConfig)
    project_root: str = "."

    @classmethod
    def load(cls) -> "BrainConfig":
        raw = os.environ.get("Jake_BRAIN_CONFIG", "").strip()
        data: Dict[str, Any] = {}
        if raw:
            try:
                parsed = json.loads(raw)
                if isinstance(parsed, dict):
                    data = parsed
            except json.JSONDecodeError:
                data = {}
        return cls.from_dict(data)

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "BrainConfig":
        llm = data.get("llm") or {}
        agent = data.get("agent") or {}
        persona = data.get("persona") or {}
        privacy = data.get("privacy") or {}

        cfg = cls(
            llm=LlmConfig(
                provider=str(llm.get("provider", "local_jake")),
                base_url=str(llm.get("base_url", "")).rstrip("/") if str(llm.get("provider", "local_jake")) != "local_jake" else "",
                model=str(llm.get("model", "Jake-small")),
                api_key=str(llm.get("api_key", "") or ""),
                timeout_ms=int(llm.get("timeout_ms", 500)),
                max_tokens=int(llm.get("max_tokens", 64)),
                temperature=float(llm.get("temperature", 0.4)),
            ),
            agent=AgentConfig(
                max_steps=int(agent.get("max_steps", 8)),
                max_retries=int(agent.get("max_retries", 2)),
            ),
            persona=PersonaConfig(
                name=str(persona.get("name", "Jake")),
                style=str(persona.get("style", "calm, professional, concise")),
                language=str(persona.get("language", "en")),
                wake_word=str(persona.get("wake_word", "Jake")),
            ),
            privacy=PrivacyConfig(
                privacy_mode=bool(privacy.get("privacy_mode", False)),
                allow_screenshots=bool(privacy.get("allow_screenshots", False)),
                allow_cloud_llm=bool(privacy.get("allow_cloud_llm", True)),
            ),
            project_root=str(data.get("project_root", ".")),
        )
        # Secrets never come from argv, only from the environment/config slice.
        if not cfg.llm.api_key:
            cfg.llm.api_key = os.environ.get("Jake_LLM_API_KEY", "")
        return cfg

    def describe(self, redact_fn=None) -> Dict[str, Any]:
        """Safe-to-log summary. Never includes the raw key."""
        key = self.llm.api_key
        masked = "set" if key else "unset"
        if redact_fn is not None and key:
            masked = "set"
        return {
            "provider": self.llm.provider,
            "base_url": self.llm.base_url,
            "model": self.llm.model,
            "api_key": masked,
            "max_steps": self.agent.max_steps,
            "autonomy_cloud": self.privacy.cloud_allowed,
            "privacy_mode": self.privacy.privacy_mode,
            "project_root": self.project_root,
        }