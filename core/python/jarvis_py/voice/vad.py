"""Energy-based voice activity detection (VAD).

Pure signal math — no audio device needed, so it is fully unit-testable with
synthetic frames. Used to (a) trim silence before speech recognition and (b)
decide when an utterance has ended (endpointing) during live listening.

The threshold adapts to the noise floor: quiet frames lower it, loud speech
raises the trigger, so a humming room does not count as talking forever.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Iterable, Iterator, List, Optional, Tuple

from .audio import pcm16_rms


@dataclass
class VadConfig:
    # Below `noise_floor_rms` a frame is definitely silence.
    noise_floor_rms: float = 120.0
    # A frame must exceed max(noise_floor * multiplier, speech_min_rms) to count.
    speech_multiplier: float = 3.0
    speech_min_rms: float = 500.0
    # Endpointing: this many consecutive non-speech frames end an utterance.
    hangover_frames: int = 40  # 40 × 10 ms = 400 ms of silence ends speech
    # Ignore utterances shorter than this (a cough / key clack).
    min_speech_frames: int = 10  # 100 ms
    # Adaptation: how quickly the noise floor tracks quiet frames.
    floor_adapt: float = 0.05


class EnergyVad:
    def __init__(self, config: Optional[VadConfig] = None):
        self.config = config or VadConfig()
        self.noise_floor_rms = self.config.noise_floor_rms
        self._in_speech = False
        self._silent_run = 0
        self._speech_run = 0

    def threshold(self) -> float:
        return max(
            self.noise_floor_rms * self.config.speech_multiplier,
            self.config.speech_min_rms,
        )

    def is_speech(self, frame: bytes) -> bool:
        rms = pcm16_rms(frame)
        # Adapt the floor only while we are not in an active utterance.
        if not self._in_speech and rms < self.threshold():
            self.noise_floor_rms = (
                (1 - self.config.floor_adapt) * self.noise_floor_rms
                + self.config.floor_adapt * max(rms, 1.0)
            )
        speech = rms >= self.threshold()
        if speech:
            self._speech_run += 1
            self._silent_run = 0
            # require a minimal burst before flipping into speech (debounce)
            if not self._in_speech and self._speech_run >= 2:
                self._in_speech = True
        else:
            self._silent_run += 1
            self._speech_run = 0
            if self._in_speech and self._silent_run >= self.config.hangover_frames:
                self._in_speech = False
        return speech

    def reset(self) -> None:
        self._in_speech = False
        self._silent_run = 0
        self._speech_run = 0


def trim_silence(frames: Iterable[bytes], vad: Optional[EnergyVad] = None) -> List[bytes]:
    """Drop leading/trailing non-speech frames (keeps the utterance, minus padding)."""
    vad = vad or EnergyVad()
    flags = [vad.is_speech(f) for f in frames]
    if not any(flags):
        return []
    first = next(i for i, f in enumerate(flags) if f)
    last = len(flags) - 1 - next(i for i, f in enumerate(reversed(flags)) if f)
    # keep 100 ms of context on each side
    pad = 10
    start = max(0, first - pad)
    end = min(len(flags), last + 1 + pad)
    out = list(frames)[start:end] if not isinstance(frames, list) else frames[start:end]
    return out


def utterances(
    frames: Iterable[bytes],
    vad: Optional[EnergyVad] = None,
    max_frames: Optional[int] = None,
) -> Iterator[List[bytes]]:
    """Split a frame stream into speech utterances using endpointing.

    Ends an utterance after `hangover_frames` of silence, and aborts (yielding
    what was gathered) when `max_frames` is reached — a hard bound so a screaming
    fan or endless noise cannot make the loop run forever.
    """
    vad = vad or EnergyVad()
    current: List[bytes] = []
    seen_speech = False
    count = 0
    for frame in frames:
        count += 1
        speech = vad.is_speech(frame)
        if speech:
            seen_speech = True
            current.append(frame)
            vad._silent_run = 0  # noqa: SLF001 - same-module state machine
        elif seen_speech:
            current.append(frame)
            if vad._silent_run >= vad.config.hangover_frames:  # noqa: SLF001
                if len(current) >= vad.config.min_speech_frames:
                    yield current
                current, seen_speech = [], False
                vad.reset()
        if max_frames is not None and count >= max_frames:
            if current and len(current) >= vad.config.min_speech_frames:
                yield current
            return
    if current and len(current) >= vad.config.min_speech_frames and seen_speech:
        yield current


def summarize(frames: List[bytes], vad: Optional[EnergyVad] = None) -> Tuple[int, int, float]:
    """(frames, speech_frames, mean_rms) — used by tests and diagnostics."""
    vad = vad or EnergyVad()
    speech = sum(1 for f in frames if vad.is_speech(f))
    rms = [pcm16_rms(f) for f in frames]
    mean = sum(rms) / len(rms) if rms else 0.0
    return len(frames), speech, mean