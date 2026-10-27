"""Brain entry point: the JSON-RPC stdio server the Rust runtime supervises.

Launched as `python .../Jake_py/main.py`; configuration arrives in the
`Jake_BRAIN_CONFIG` environment variable. stderr is inherited (visible in logs),
stdout is reserved for the protocol.
"""

from __future__ import annotations

import sys
import traceback
from typing import Any, Dict

from .brain import Brain
from .config import BrainConfig
from .llm import available_providers
from .rpc import METHOD_NOT_FOUND, RpcError, RpcServer
from .voice import create_stt, create_tts

SERVER_VERSION = "0.1.0"

MAX_SPEECH_CHARS = 5000


def build_server(cfg: BrainConfig | None = None, server: RpcServer | None = None) -> RpcServer:
    cfg = cfg or BrainConfig.load()
    srv = server or RpcServer()
    brain = Brain(cfg)

    # ------------------------------------------------------------- brain.* API
    def chat(params: Any) -> Dict[str, Any]:
        if not isinstance(params, dict):
            raise RpcError(-32602, "brain.chat params must be an object")
        try:
            return brain.chat(params)
        except Exception as exc:  # noqa: BLE001 - the peer must always get an answer
            traceback.print_exc(file=sys.stderr)
            return {
                "content": (
                    "I hit an internal error while thinking, so I did not act on "
                    "anything. Nothing was changed."
                ),
                "tool_calls": [],
                "provider": cfg.llm.provider,
                "model": cfg.llm.model,
                "usage": {},
                "error": f"internal: {type(exc).__name__}: {exc}",
            }

    def plan(params: Any) -> Dict[str, Any]:
        if not isinstance(params, dict):
            raise RpcError(-32602, "brain.plan params must be an object")
        return brain.plan(params)

    def health(params: Any) -> Dict[str, Any]:
        out = brain.health()
        out["version"] = SERVER_VERSION
        out["protocol"] = "1.0"
        out["providers"] = available_providers()
        return out

    def cancel(params: Any) -> None:
        # Phase 2 will hook this into streaming generation; for now generation is
        # request-scoped and the runtime enforces its own timeouts.
        srv.log("info", "brain.cancel received (no streaming in-flight)")

    # ------------------------------------------------------------- voice.* API
    # Engines are created lazily: probing spawns PowerShell, and a config-only
    # startup should never pay for that.
    engines: dict[str, Any] = {}

    def tts_engine():
        if "tts" not in engines:
            engines["tts"] = create_tts()
        return engines["tts"]

    def stt_engine():
        if "stt" not in engines:
            engines["stt"] = create_stt()
        return engines["stt"]

    def _speech_text(params: Any) -> str:
        if not isinstance(params, dict) or not isinstance(params.get("text"), str):
            raise RpcError(-32602, "voice.* params must include a string `text`")
        text = params["text"].strip()
        if not text:
            raise RpcError(-32602, "`text` must not be empty")
        if len(text) > MAX_SPEECH_CHARS:
            raise RpcError(-32602, f"`text` longer than {MAX_SPEECH_CHARS} characters")
        return text

    def speak(params: Any) -> Dict[str, Any]:
        text = _speech_text(params)
        rate = int(params.get("rate", 0))
        volume = int(params.get("volume", 100))
        return tts_engine().speak(text, rate=rate, volume=volume)

    def render(params: Any) -> Dict[str, Any]:
        text = _speech_text(params)
        path = params.get("path") if isinstance(params, dict) else None
        if not isinstance(path, str) or not path:
            raise RpcError(-32602, "voice.render requires `path`")
        rate = int(params.get("rate", 0))
        volume = int(params.get("volume", 100))
        return tts_engine().speak_to_wav(text, path, rate=rate, volume=volume)

    def tts_probe(_params: Any) -> Dict[str, Any]:
        return tts_engine().available()

    def stt_probe(_params: Any) -> Dict[str, Any]:
        return stt_engine().available()

    def dictate(params: Any) -> Dict[str, Any]:
        timeout_s = float(params.get("timeout_s", 8.0)) if isinstance(params, dict) else 8.0
        timeout_s = max(2.0, min(60.0, timeout_s))
        return stt_engine().dictate(timeout_s=timeout_s)

    def utterance(params: Any) -> Dict[str, Any]:
        """One voice capture: `mode` = "wav" (offline file) or "mic" (live).

        Returns {ok, woke, command, transcript} or an honest failure with `stage`
        telling the runtime where it broke (input/vad/wake/stt).
        """
        if not isinstance(params, dict):
            raise RpcError(-32602, "voice.utterance params must be an object")
        from .voice.pipeline import PipelineConfig, VoicePipeline

        mode = str(params.get("mode", "mic"))
        wake_words = params.get("wake_words") or []
        if isinstance(wake_words, str):
            wake_words = [w.strip() for w in wake_words.split(",") if w.strip()]
        config = PipelineConfig(
            wake_words=[str(w) for w in wake_words],
            utterance_timeout_s=float(params.get("timeout_s", 8.0)),
            skip_wake=bool(params.get("skip_wake", False)),
            min_wake_confidence=float(params.get("min_wake_confidence", 0.3)),
        )
        pipeline = VoicePipeline(config, stt=create_stt())
        if mode == "wav":
            path = params.get("path")
            if not isinstance(path, str) or not path:
                raise RpcError(-32602, "voice.utterance mode=wav requires `path`")
            return pipeline.process_wav(path)
        if mode == "mic":
            return pipeline.process_mic(timeout_s=config.utterance_timeout_s)
        raise RpcError(-32602, f"unknown voice.utterance mode `{mode}`")

    def voice_demo_wav(params: Any) -> Dict[str, Any]:
        """Render speech to a WAV (offline demo/verification of the voice loop)."""
        from .voice.pipeline import build_demo_wav

        if not isinstance(params, dict) or not isinstance(params.get("text"), str):
            raise RpcError(-32602, "voice.demo_wav requires `text`")
        if not params["text"].strip():
            raise RpcError(-32602, "`text` must not be empty")
        path = params.get("path") if isinstance(params.get("path"), str) else None
        return build_demo_wav(params["text"], path)

    def voice_stop(_params: Any) -> Dict[str, Any]:
        """Barge-in / emergency stop: silence the TTS immediately."""
        return tts_engine().stop()

    srv.handle("brain.chat", chat)
    srv.handle("brain.plan", plan)
    srv.handle("brain.health", health)
    srv.handle_notification("brain.cancel", cancel)
    srv.handle("voice.speak", speak)
    srv.handle("voice.render", render)
    srv.handle("voice.tts_probe", tts_probe)
    srv.handle("voice.stt_probe", stt_probe)
    srv.handle("voice.dictate", dictate)
    srv.handle("voice.utterance", utterance)
    srv.handle("voice.demo_wav", voice_demo_wav)
    srv.handle("voice.stop", voice_stop)
    return srv


def main() -> int:
    server = build_server()
    try:
        server.serve_forever()
    except KeyboardInterrupt:  # pragma: no cover
        pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
