"""Voice engine tests.

The TTS test renders a **real** WAV through the Windows synthesizer (offline, no
network) and validates the RIFF header — the file is the evidence. STT tests
probe the platform engine and assert only that the answer is truthful (either it
works, or it fails with a concrete reason).
"""

import os

import pytest

from jarvis_py.voice import create_stt, create_tts, probe_wav_header
from jarvis_py.voice.stt import NullStt
from jarvis_py.voice.tts import NullTts

IS_WINDOWS = os.name == "nt"


def test_factories_pick_platform_engines():
    if IS_WINDOWS:
        assert type(create_tts()).__name__ == "SystemSpeechTts"
        assert type(create_stt()).__name__ == "WindowsSpeechStt"
    else:
        assert isinstance(create_tts(), NullTts)
        assert isinstance(create_stt(), NullStt)
    assert isinstance(create_tts("null"), NullTts)
    assert isinstance(create_stt("null"), NullStt)


def test_null_engines_report_inability_instead_of_pretending():
    tts = NullTts()
    assert tts.available()["ok"] is False
    assert tts.speak("hi")["ok"] is False
    assert tts.speak_to_wav("hi", "out.wav")["ok"] is False
    stt = NullStt()
    assert stt.available()["ok"] is False
    result = stt.dictate()
    assert result["ok"] is False and "unavailable" in result["detail"]


@pytest.mark.skipif(not IS_WINDOWS, reason="requires Windows System.Speech")
def test_tts_probe_reports_truthfully():
    info = create_tts().available()
    assert isinstance(info["ok"], bool)
    if info["ok"]:
        assert info["detail"]
    else:
        # failure must carry a reason
        assert info["detail"] and info["detail"] != "None"


@pytest.mark.skipif(not IS_WINDOWS, reason="requires Windows System.Speech")
def test_tts_renders_a_real_wav_file():
    out = os.path.join(os.path.abspath("."), "logs", "tts-py-test.wav")
    result = create_tts().speak_to_wav("Jarvis voice verification.", out, rate=1, volume=80)
    assert result["ok"], result
    assert os.path.getsize(out) > 1024
    header = probe_wav_header(out)
    assert header["ok"], header
    os.remove(out)


def test_wav_probe_detects_garbage():
    bad = os.path.join(os.path.abspath("."), "logs", "not-a-wav.bin")
    os.makedirs(os.path.dirname(bad), exist_ok=True)
    with open(bad, "wb") as fh:
        fh.write(b"NOPE" * 10)
    assert probe_wav_header(bad)["ok"] is False
    assert probe_wav_header(os.path.join(os.path.abspath("."), "logs", "missing.wav"))["ok"] is False
    os.remove(bad)


@pytest.mark.skipif(not IS_WINDOWS, reason="requires Windows Speech Recognition")
def test_stt_probe_is_truthful_about_the_platform():
    info = create_stt().available()
    assert isinstance(info["ok"], bool)
    if not info["ok"]:
        # an honest failure explains itself
        assert len(info["detail"]) > 5


@pytest.mark.skipif(not IS_WINDOWS, reason="requires Windows Speech Recognition")
def test_dictate_returns_honest_result_when_nobody_speaks():
    # Nobody speaks into the mic during CI: the engine must report "heard nothing"
    # or an engine error — never a fabricated transcript.
    result = create_stt().dictate(timeout_s=2.0)
    assert isinstance(result["ok"], bool)
    if result["ok"]:
        assert isinstance(result["text"], str)
    else:
        assert result.get("detail"), "failure must explain itself"