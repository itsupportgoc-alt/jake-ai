import json

from Jake_py.config import BrainConfig


def test_defaults_without_env(monkeypatch):
    monkeypatch.delenv("Jake_BRAIN_CONFIG", raising=False)
    monkeypatch.delenv("Jake_LLM_API_KEY", raising=False)
    cfg = BrainConfig.load()
    assert cfg.llm.provider == "openai_compat"
    assert cfg.agent.max_steps == 8
    assert cfg.persona.name == "Jake"
    assert cfg.llm.api_key == ""


def test_config_from_runtime_slice(monkeypatch):
    payload = {
        "llm": {
            "provider": "openai_compat",
            "base_url": "http://localhost:11434/v1/",
            "model": "llama3",
            "api_key": "",
            "timeout_ms": 5000,
            "max_tokens": 100,
            "temperature": 0.1,
        },
        "agent": {"max_steps": 3, "max_retries": 0},
        "persona": {"name": "J", "style": "x", "language": "fil", "wake_word": "tol"},
        "privacy": {"privacy_mode": True, "allow_screenshots": False, "allow_cloud_llm": True},
        "project_root": "C:/proj",
    }
    monkeypatch.setenv("Jake_BRAIN_CONFIG", json.dumps(payload))
    cfg = BrainConfig.load()
    # trailing slash is normalised so url joining cannot double up
    assert cfg.llm.base_url == "http://localhost:11434/v1"
    assert cfg.llm.timeout_s == 5.0
    assert cfg.agent.max_steps == 3
    assert cfg.persona.wake_word == "tol"
    assert cfg.persona.language == "fil"
    assert cfg.privacy.privacy_mode is True
    assert cfg.privacy.cloud_allowed is False  # privacy mode kills cloud
    assert cfg.project_root == "C:/proj"


def test_api_key_falls_back_to_environment(monkeypatch):
    monkeypatch.delenv("Jake_BRAIN_CONFIG", raising=False)
    monkeypatch.setenv("Jake_LLM_API_KEY", "sk-env-key-12345678901234567890")
    cfg = BrainConfig.load()
    assert cfg.llm.api_key.startswith("sk-env-key")


def test_describe_never_leaks_the_key(monkeypatch):
    monkeypatch.setenv("Jake_BRAIN_CONFIG", json.dumps({"llm": {"api_key": "sk-secret-42"}}))
    cfg = BrainConfig.load()
    described = json.dumps(cfg.describe())
    assert "sk-secret-42" not in described
    assert cfg.describe()["api_key"] == "set"


def test_malformed_env_json_falls_back_to_defaults(monkeypatch):
    monkeypatch.setenv("Jake_BRAIN_CONFIG", "{not json")
    cfg = BrainConfig.load()
    assert cfg.llm.model == "gpt-4o-mini"