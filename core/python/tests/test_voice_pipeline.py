"""Voice pipeline tests: VAD, wake word, audio sources, and the offline chain.

The integration test renders *real speech* with TTS and drives it through VAD →
wake grammar → STT, so the voice loop is verified on a machine with no
microphone. Live-mic paths are asserted only to fail honestly.
"""

import os

import pytest

from jarvis_py.voice.audio import (
    AudioError,
    MicSource,
    SyntheticSource,
    WavSource,
    pcm16_rms,
    read_wav_info,
    silence_frames,
    tone_frames,
    write_wav,
)
from jarvis_py.voice.pipeline import PipelineConfig, VoicePipeline, build_demo_wav
from jarvis_py.voice.vad import EnergyVad, trim_silence, utterances
from jarvis_py.voice.wake import WakeWordEngine, normalise

IS_WINDOWS = os.name == "nt"
LOGS = os.path.join(os.path.abspath("."), "logs")


# --------------------------------------------------------------------- audio

def test_wav_round_trip_and_info(tmp_path):
    path = str(tmp_path / "tone.wav")
    frames = tone_frames(0.3) + silence_frames(0.2)
    size = write_wav(path, frames)
    assert size > 0
    rate, channels, duration_ms = read_wav_info(path)
    assert rate == 16_000 and channels == 1
    assert 450 <= duration_ms <= 600
    with WavSource(path) as src:
        read = list(src.frames(160))
    assert len(read) == 50  # 500 ms / 10 ms
    assert pcm16_rms(read[0]) > 1000  # the tone is loud
    assert pcm16_rms(read[-1]) < 100  # the tail is silence


def test_wav_source_rejects_missing_and_bad_files(tmp_path):
    with pytest.raises(AudioError):
        WavSource(str(tmp_path / "nope.wav"))
    bad = tmp_path / "bad.wav"
    bad.write_bytes(b"not a wav at all")
    with pytest.raises(AudioError):
        WavSource(str(bad))


def test_mic_source_fails_honestly_without_a_device():
    # No `sounddevice` installed (and no audio device): must raise AudioError,
    # never return a fake stream.
    try:
        import sounddevice  # noqa: F401

        pytest.skip("sounddevice installed — mic path untested here")
    except ImportError:
        pass
    with pytest.raises(AudioError) as exc:
        MicSource()
    assert "sounddevice" in str(exc.value)


def test_synthetic_source_yields_its_frames():
    frames = tone_frames(0.1) + silence_frames(0.1)
    src = SyntheticSource(frames)
    assert len(list(src.frames(160))) == 20
    src.close()


# ------------------------------------------------------------------------ VAD

def test_vad_separates_tone_from_silence():
    vad = EnergyVad()
    silent = [vad.is_speech(f) for f in silence_frames(0.3)]
    assert sum(silent) <= 2  # debounce lets at most a frame or two through
    loud = [vad.is_speech(f) for f in tone_frames(0.3)]
    assert sum(loud) >= 25


def test_vad_adapts_to_a_constant_noise_floor():
    vad = EnergyVad()
    # a steady hum below the speech threshold must not count as speech
    hum = tone_frames(1.0, amplitude=200, freq=120.0)
    flags = [vad.is_speech(f) for f in hum]
    assert sum(flags) < len(flags) // 2


def test_trim_silence_keeps_only_the_utterance():
    frames = silence_frames(0.5) + tone_frames(0.4) + silence_frames(0.5)
    trimmed = trim_silence(frames, EnergyVad())
    assert 0 < len(trimmed) < len(frames)
    assert len(trimmed) >= 40  # the 400 ms tone survives


def test_utterances_splits_on_silence_and_bounds_stream():
    frames = (
        silence_frames(0.3)
        + tone_frames(0.3)
        + silence_frames(0.6)
        + tone_frames(0.3)
        + silence_frames(0.6)
    )
    segs = list(utterances(frames, EnergyVad()))
    assert len(segs) == 2
    # hard bound: a never-ending stream must stop at max_frames
    endless = tone_frames(2.0)
    bounded = list(utterances(endless, EnergyVad(), max_frames=50))
    assert sum(len(s) for s in bounded) <= 50


def test_trim_silence_on_pure_silence_returns_empty():
    assert trim_silence(silence_frames(1.0), EnergyVad()) == []


# ------------------------------------------------------------------ wake word

def test_wake_word_prefix_match_and_strip():
    engine = WakeWordEngine(["Jarvis"])
    hit = engine.check("Jarvis, open notepad")
    assert hit.woke and hit.matched == "Jarvis"
    assert hit.remainder.lower().startswith("open notepad")

    assert engine.check("jarvis what time is it").woke
    assert engine.check("Jarvis!").woke  # exact match with punctuation
    assert not engine.check("what time is it").woke


def test_wake_word_is_not_matched_in_the_middle_by_default():
    engine = WakeWordEngine(["Jarvis"])
    assert not engine.check("hey ask jarvis later").woke
    # …unless prefix matching is relaxed (grammar-based engines):
    assert engine.check("hey ask jarvis later", require_prefix=False).woke


def test_multi_word_and_alternative_wake_words():
    engine = WakeWordEngine.from_config("Jarvis, Hey Jarvis, Computer")
    assert engine.check("hey jarvis, status report").woke
    assert engine.check("computer, open the folder").woke
    assert not engine.check("hey there").woke


def test_low_confidence_wake_is_refused():
    engine = WakeWordEngine(["Jarvis"])
    result = engine.check("Jarvis do it", confidence=0.1, min_confidence=0.3)
    assert not result.woke  # refuse rather than act on a hallucinated wake


def test_normalisation_handles_punctuation_and_case():
    assert normalise("  Jarvis,  WHAT?! ") == "jarvis what"
    assert normalise("naïve—test") == "naïve test"


def test_empty_wake_config_never_wakes():
    engine = WakeWordEngine([])
    assert engine.wake_words == []
    assert not engine.check("Jarvis, now").woke


# ------------------------------------------------------------------ pipeline

def test_pipeline_reports_missing_file_honestly(tmp_path):
    out = VoicePipeline(PipelineConfig(wake_words=["Jarvis"])).process_wav(
        str(tmp_path / "missing.wav")
    )
    assert not out["ok"] and out["stage"] == "input"
    assert "no such audio file" in out["error"]


def test_pipeline_rejects_silence_before_spending_time_in_stt(tmp_path):
    path = str(tmp_path / "quiet.wav")
    write_wav(path, silence_frames(1.0))
    out = VoicePipeline(PipelineConfig(wake_words=["Jarvis"])).process_wav(path)
    assert not out["ok"] and out["stage"] == "vad"


@pytest.mark.skipif(not IS_WINDOWS, reason="requires Windows TTS + SR engines")
def test_full_offline_voice_chain_wake_and_command():
    """TTS → VAD → wake grammar → STT → command extraction, for real."""
    rendered = build_demo_wav("Jarvis, what time is it?")
    assert rendered["ok"], rendered
    out = VoicePipeline(PipelineConfig(wake_words=["Jarvis"])).process_wav(rendered["path"])
    assert out["ok"], out
    assert out["woke"] is True
    assert out["wake_word"].lower() == "jarvis"
    assert out["wake_confidence"] >= 0.5
    # the command survives recognition (spoken slightly slowly for fidelity)
    assert "time" in out["command"].lower(), out
    os.remove(rendered["path"])


@pytest.mark.skipif(not IS_WINDOWS, reason="requires Windows TTS + SR engines")
def test_audio_without_wake_word_is_not_addressed_to_us():
    rendered = build_demo_wav("what time is it today")
    assert rendered["ok"], rendered
    out = VoicePipeline(PipelineConfig(wake_words=["Jarvis"])).process_wav(rendered["path"])
    assert out["ok"], out
    assert out["woke"] is False
    assert "wake word" in out["reason"]
    os.remove(rendered["path"])


def test_mic_mode_fails_honestly_without_an_audio_device():
    out = VoicePipeline(PipelineConfig(wake_words=["Jarvis"])).process_mic(timeout_s=2.0)
    assert not out["ok"]
    assert out["stage"] in ("input", "vad")
    assert out["error"], "failure must explain itself"


def test_pipeline_health_is_truthful():
    health = VoicePipeline(PipelineConfig(wake_words=["Jarvis"])).health()
    assert isinstance(health["stt_ok"], bool)
    if not health["stt_ok"]:
        assert health["stt_detail"]
    assert health["wake"]["wake_words"] == ["jarvis"]


@pytest.mark.skipif(not IS_WINDOWS, reason="requires Windows System.Speech")
def test_barge_in_stops_speech_midway():
    """`stop()` must kill in-flight speech — the reply reports interruption,
    never success."""
    import threading
    import time

    from jarvis_py.voice.tts import create_tts

    tts = create_tts()
    long_text = (
        "This sentence exists only to keep the synthesizer busy for a while. "
        * 60
    )
    results = []
    worker = threading.Thread(target=lambda: results.append(tts.speak(long_text)))
    worker.start()
    time.sleep(2.0)  # enough for PowerShell to start and begin speaking
    stop_result = tts.stop()
    worker.join(timeout=15)
    assert not worker.is_alive(), "speech did not stop"
    assert stop_result["ok"] is True
    assert results, "speak() never returned"
    outcome = results[0]
    # killed mid-utterance → reported honestly as stopped/interrupted
    assert outcome["ok"] is False, f"killed speech reported success: {outcome}"
    assert "stop" in outcome["detail"].lower() or "interrupt" in outcome["detail"].lower(), outcome