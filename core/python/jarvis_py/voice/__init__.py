"""Voice package: TTS/STT behind swappable interfaces.

Voice conversation (mic capture loop, wake word, VAD, barge-in) is Phase 2
continuation; what exists here is real, probe-able speech output and a
dictation engine that honestly reports when the platform has none.
"""

from __future__ import annotations

from typing import Optional

from .audio import AudioError, AudioSource, SyntheticSource, WavSource, create_source
from .pipeline import PipelineConfig, VoicePipeline, build_demo_wav
from .stt import NullStt, SttEngine, WindowsSpeechStt, create_stt
from .tts import NullTts, SystemSpeechTts, TtsEngine, create_tts, probe_wav_header
from .vad import EnergyVad, VadConfig, trim_silence, utterances
from .wake import WakeResult, WakeWordEngine

__all__ = [
    "TtsEngine",
    "SttEngine",
    "SystemSpeechTts",
    "WindowsSpeechStt",
    "NullTts",
    "NullStt",
    "create_tts",
    "create_stt",
    "probe_wav_header",
    "AudioSource",
    "AudioError",
    "WavSource",
    "SyntheticSource",
    "create_source",
    "EnergyVad",
    "VadConfig",
    "trim_silence",
    "utterances",
    "WakeWordEngine",
    "WakeResult",
    "PipelineConfig",
    "VoicePipeline",
    "build_demo_wav",
]


def create_voice(tts_id: Optional[str] = None, stt_id: Optional[str] = None):
    return create_tts(tts_id), create_stt(stt_id)