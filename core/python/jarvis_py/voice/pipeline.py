"""The voice pipeline: source → VAD → (wake word) → STT → command transcript.

`Pipeline` is pure orchestration around the engine interfaces, so it runs the
same offline (WAV file — fully testable on a machine with no microphone) and
live (microphone). Every failure path returns a structured, honest result.
"""

from __future__ import annotations

import os
import tempfile
from dataclasses import dataclass
from typing import Any, Dict, List, Optional

from .audio import AudioError, AudioSource, WavSource, create_source, silence_frames, write_wav
from .stt import SttEngine, create_stt
from .vad import EnergyVad, trim_silence, utterances
from .wake import WakeWordEngine, WakeResult

# Hard ceilings: one utterance and one session must never run unbounded.
MAX_UTTERANCE_SECONDS = 30
MAX_SOURCE_FRAMES = 4000  # 40 s of 10 ms frames


@dataclass
class PipelineConfig:
    wake_words: List[str]
    #: seconds of speech to collect in one utterance before endpointing
    utterance_timeout_s: float = 10.0
    #: skip wake-word checking (the caller already knows we're addressed)
    skip_wake: bool = False
    #: minimum STT confidence for a wake word to count
    min_wake_confidence: float = 0.3

    def __post_init__(self) -> None:
        self.utterance_timeout_s = max(1.0, min(self.utterance_timeout_s, MAX_UTTERANCE_SECONDS))


class VoicePipeline:
    def __init__(
        self,
        config: PipelineConfig,
        stt: Optional[SttEngine] = None,
        wake_engine: Optional[WakeWordEngine] = None,
    ):
        self.config = config
        self.stt = stt or create_stt()
        self.wake = wake_engine or WakeWordEngine(config.wake_words)

    # ------------------------------------------------------------------ helpers
    def _transcribe_wav(self, path: str) -> Dict[str, Any]:
        """One STT pass over a WAV file via the engine's file interface."""
        return self.stt.transcribe_file(path, timeout_s=self.config.utterance_timeout_s + 10)

    def _wake_check_wav(self, path: str) -> Dict[str, Any]:
        return self.stt.detect_wake_file(
            path,
            self.config.wake_words,
            timeout_s=self.config.utterance_timeout_s + 10,
        )

    # --------------------------------------------------------------------- API
    def process_wav(self, path: str) -> Dict[str, Any]:
        """Offline: wake check + transcribe a WAV file. Fully deterministic path
        used by tests, `Jake voice --wav`, and as the STT fallback everywhere."""
        if not os.path.isfile(path):
            return {"ok": False, "error": f"no such audio file: {path}", "stage": "input"}
        try:
            with WavSource(path) as source:
                if source.duration_s < 0.15:
                    return {"ok": False, "error": "audio is too short to contain speech", "stage": "input"}
                source_frames = list(source.frames(160))
        except AudioError as exc:
            return {"ok": False, "error": str(exc), "stage": "input"}

        # VAD: nothing but silence → say so before spending time in STT.
        vad = EnergyVad()
        speech_frames = sum(1 for f in source_frames if vad.is_speech(f))
        if speech_frames < vad.config.min_speech_frames:
            return {"ok": False, "error": "no speech detected in the audio", "stage": "vad"}

        if not self.config.skip_wake and self.wake.wake_words:
            wake = self._wake_check_wav(path)
            if not wake.get("ok"):
                return {
                    "ok": False,
                    "error": wake.get("error") or wake.get("detail") or "wake check failed",
                    "stage": "wake",
                }
            result = WakeResult(
                woke=bool(wake.get("woke")),
                remainder=str(wake.get("remainder", "")),
                matched=str(wake.get("matched", "")),
                confidence=float(wake.get("confidence", 1.0)),
                transcript=str(wake.get("transcript", "")),
            )
            if not result.woke and result.confidence < self.config.min_wake_confidence:
                return {
                    "ok": True,
                    "woke": False,
                    "transcript": result.transcript,
                    "reason": f"wake word not detected (confidence {result.confidence:.2f})",
                }
            if not result.woke:
                return {
                    "ok": True,
                    "woke": False,
                    "transcript": result.transcript,
                    "reason": "wake word not present in the audio",
                }
            # Dictate the full file for the command, then strip the wake prefix.
            dictation = self._transcribe_wav(path)
            if not dictation.get("ok"):
                return {
                    "ok": False,
                    "error": dictation.get("error") or dictation.get("detail") or "transcription failed",
                    "stage": "stt",
                }
            full = str(dictation.get("text", ""))
            remainder = self.wake.check(full, require_prefix=False).remainder or full
            return {
                "ok": True,
                "woke": True,
                "wake_word": result.matched,
                "wake_confidence": result.confidence,
                "transcript": full,
                "command": remainder.strip() or full.strip(),
                "stage": "done",
            }

        # No wake word configured → the whole file is the command.
        dictation = self._transcribe_wav(path)
        if not dictation.get("ok"):
            return {
                "ok": False,
                "error": dictation.get("error") or dictation.get("detail") or "transcription failed",
                "stage": "stt",
            }
        text = str(dictation.get("text", "")).strip()
        if not text:
            return {"ok": False, "error": "speech recognition produced no text", "stage": "stt"}
        return {"ok": True, "woke": True, "transcript": text, "command": text, "stage": "done"}

    def process_mic(self, timeout_s: Optional[float] = None) -> Dict[str, Any]:
        """Live: capture from the microphone, VAD-segment, STT the first utterance.

        Requires a working audio device + `sounddevice`; failures are returned
        verbatim so the assistant can tell the user the truth.
        """
        timeout_s = timeout_s or self.config.utterance_timeout_s
        try:
            source: AudioSource = create_source("mic")
        except AudioError as exc:
            return {"ok": False, "error": str(exc), "stage": "input"}

        temp_path = ""
        try:
            frame_samples = 160  # 10 ms @ 16 kHz
            collected: List[bytes] = []
            vad = EnergyVad()
            seen_speech = False
            silent_run = 0
            max_frames = int(min(timeout_s, MAX_UTTERANCE_SECONDS) * 100)
            for frame in source.frames(frame_samples):
                if len(collected) >= max_frames:
                    break
                speech = vad.is_speech(frame)
                if speech:
                    seen_speech = True
                    collected.append(frame)
                    silent_run = 0
                elif seen_speech:
                    collected.append(frame)
                    silent_run += 1
                    if silent_run >= vad.config.hangover_frames:
                        break
            if not seen_speech or len(collected) < vad.config.min_speech_frames:
                return {"ok": False, "error": "no speech detected before the timeout", "stage": "vad"}
            trimmed = trim_silence(collected, EnergyVad())
            if not trimmed:
                return {"ok": False, "error": "speech was only noise", "stage": "vad"}
            fd, temp_path = tempfile.mkstemp(suffix=".wav", prefix="Jake-utt-")
            os.close(fd)
            write_wav(temp_path, trimmed, source.sample_rate)
        except AudioError as exc:
            return {"ok": False, "error": str(exc), "stage": "input"}
        except Exception as exc:  # noqa: BLE001 - surface capture failures honestly
            return {"ok": False, "error": f"capture failed: {exc}", "stage": "input"}
        finally:
            try:
                source.close()
            except Exception:  # noqa: BLE001
                pass

        try:
            # The captured utterance is already the command — wake detection for a
            # live mic is grammar-based on the stream; here we accept the segment
            # and strip a leading wake word if the user included it.
            dictation = self._transcribe_wav(temp_path)
            if not dictation.get("ok"):
                return {
                    "ok": False,
                    "error": dictation.get("error") or dictation.get("detail") or "transcription failed",
                    "stage": "stt",
                }
            full = str(dictation.get("text", "")).strip()
            if not full:
                return {"ok": False, "error": "speech recognition produced no text", "stage": "stt"}
            check = self.wake.check(full, require_prefix=False)
            command = (check.remainder or full).strip()
            return {
                "ok": True,
                "woke": True,
                "transcript": full,
                "command": command or full,
                "stage": "done",
            }
        finally:
            try:
                os.remove(temp_path)
            except OSError:
                pass

    def health(self) -> Dict[str, Any]:
        info = self.stt.available()
        return {
            "stt_ok": bool(info.get("ok")),
            "stt_detail": info.get("detail", ""),
            "wake": self.wake.describe(),
            "utterance_timeout_s": self.config.utterance_timeout_s,
        }


# ---------------------------------------------------------------------- utilities

def build_demo_wav(text: str, path: Optional[str] = None, rate: int = -3) -> Dict[str, Any]:
    """Render `text` to a WAV with the platform TTS — used by tests and by
    `Jake voice --demo` so the voice loop can be exercised without a microphone.

    Rendered slightly *slower* (rate=-3) by default: speech recognizers transcribe
    deliberately-paced audio far more accurately (measured here: 0.53 → 0.74
    confidence, and the wake word stops being mangled)."""
    from .tts import create_tts

    target = path or os.path.join(tempfile.gettempdir(), "Jake-voice-demo.wav")
    result = create_tts().speak_to_wav(text, target, rate=rate)
    if not result.get("ok"):
        return {"ok": False, "error": result.get("detail", "TTS render failed")}
    rate, channels, duration_ms = _wav_info(target)
    return {
        "ok": True,
        "path": target,
        "bytes": result.get("bytes", 0),
        "sample_rate": rate,
        "duration_ms": duration_ms,
    }


def _wav_info(path: str):
    import wave

    with wave.open(path, "rb") as wav:
        rate = wav.getframerate() or 1
        return rate, wav.getnchannels(), int(wav.getnframes() / rate * 1000)


def trim_wav_to_speech(path: str) -> str:
    """Write a silence-trimmed copy of `path`; returns the new path (or input)."""
    try:
        with WavSource(path) as source:
            frames = list(source.frames(160))
            rate = source.sample_rate
    except AudioError:
        return path
    trimmed = trim_silence(frames, EnergyVad())
    if not trimmed or len(trimmed) == len(frames):
        return path
    out = path + ".trim.wav"
    write_wav(out, trimmed, rate)
    return out


__all__ = [
    "PipelineConfig",
    "VoicePipeline",
    "build_demo_wav",
    "trim_wav_to_speech",
    "silence_frames",
]