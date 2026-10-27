"""Audio sources and WAV handling (standard library only).

`AudioSource` abstracts *where* samples come from: a microphone (live, needs an
audio device), a WAV file (offline, fully testable), or synthetic frames (unit
tests). The rest of the voice pipeline only ever sees frames.
"""

from __future__ import annotations

import struct
import wave
from abc import ABC, abstractmethod
from typing import Iterator, List, Optional, Tuple

# 16-bit PCM frame chunk used by the VAD (10 ms at 16 kHz = 160 samples)
DEFAULT_SAMPLE_RATE = 16_000
FRAME_MS = 10


class AudioError(Exception):
    """Raised when an audio source cannot start (device missing, bad file …)."""


class AudioSource(ABC):
    """Yields 16-bit PCM frames."""

    id: str = "abstract"

    @abstractmethod
    def frames(self, frame_samples: int) -> Iterator[bytes]:
        """Yield fixed-size little-endian PCM16 frames until the source ends."""

    @abstractmethod
    def close(self) -> None: ...

    @property
    @abstractmethod
    def sample_rate(self) -> int: ...


class WavSource(AudioSource):
    """Reads a WAV file — the offline, testable source."""

    id = "wav"

    def __init__(self, path: str):
        try:
            self._wav = wave.open(path, "rb")
        except (wave.Error, OSError) as exc:
            raise AudioError(f"cannot open WAV `{path}`: {exc}") from exc
        if self._wav.getsampwidth() != 2:
            self._wav.close()
            raise AudioError(f"`{path}` is not 16-bit PCM (sample width {self._wav.getsampwidth()})")
        self._path = path

    @property
    def sample_rate(self) -> int:
        return self._wav.getframerate()

    @property
    def channels(self) -> int:
        return self._wav.getnchannels()

    @property
    def duration_s(self) -> float:
        rate = max(1, self._wav.getframerate())
        return self._wav.getnframes() / rate

    def frames(self, frame_samples: int) -> Iterator[bytes]:
        channels = self._wav.getnchannels()
        # wave.readframes() counts *samples per channel*, not bytes.
        want = frame_samples * channels
        while True:
            data = self._wav.readframes(want)
            if not data:
                break
            got_samples = len(data) // 2 // channels
            if got_samples == 0:
                break
            if channels > 1:
                data = _downmix_mono16(data, channels)
            yield data
            if got_samples < want:
                break

    def close(self) -> None:
        self._wav.close()

    def __enter__(self) -> "WavSource":
        return self

    def __exit__(self, *exc) -> None:
        self.close()


def _downmix_mono16(data: bytes, channels: int) -> bytes:
    count = len(data) // 2 // channels
    samples = struct.unpack(f"<{count * channels}h", data[: count * channels * 2])
    mono = []
    for i in range(count):
        base = i * channels
        mono.append(sum(samples[base : base + channels]) // channels)
    return struct.pack(f"<{count}h", *mono)


class MicSource(AudioSource):
    """Live microphone capture via the optional `sounddevice` package.

    This machine (and CI) may have no audio device — starting the mic then raises
    `AudioError` with the platform's own reason; the pipeline reports it honestly.
    """

    id = "mic"

    def __init__(self, sample_rate: int = DEFAULT_SAMPLE_RATE):
        try:
            import sounddevice  # type: ignore
        except ImportError as exc:
            raise AudioError(
                "microphone capture requires the optional `sounddevice` package "
                "(pip install sounddevice)"
            ) from exc
        self._sd = sounddevice
        self._rate = sample_rate
        try:
            self._stream = sounddevice.RawInputStream(
                samplerate=sample_rate,
                channels=1,
                dtype="int16",
                blocksize=0,
            )
            self._stream.start()
        except Exception as exc:  # PortAudio raises its own hierarchy
            raise AudioError(f"cannot open the microphone: {exc}") from exc

    @property
    def sample_rate(self) -> int:
        return self._rate

    def frames(self, frame_samples: int) -> Iterator[bytes]:
        while True:
            data, overflowed = self._stream.read(frame_samples)
            if overflowed:
                continue
            if isinstance(data, (bytes, bytearray)):
                yield bytes(data)
            else:  # numpy/raw buffer path
                yield bytes(memoryview(data).tobytes())

    def close(self) -> None:
        try:
            self._stream.stop()
            self._stream.close()
        except Exception:  # noqa: BLE001 - closing must not raise
            pass


def read_wav_info(path: str) -> Tuple[int, int, int]:
    """(sample_rate, channels, duration_ms) without loading samples."""
    with wave.open(path, "rb") as wav:
        rate = wav.getframerate() or 1
        return rate, wav.getnchannels(), int(wav.getnframes() / rate * 1000)


def pcm16_rms(frame: bytes) -> float:
    """Root-mean-square of little-endian PCM16 bytes — the VAD's only input."""
    if len(frame) < 2:
        return 0.0
    count = len(frame) // 2
    samples = struct.unpack(f"<{count}h", frame[: count * 2])
    if not samples:
        return 0.0
    return (sum(s * s for s in samples) / count) ** 0.5


def write_wav(path: str, frames: List[bytes], sample_rate: int = DEFAULT_SAMPLE_RATE) -> int:
    """Write PCM16 frames to a WAV (used by tests to build synthetic audio)."""
    with wave.open(path, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(sample_rate)
        for f in frames:
            wav.writeframes(f)
    import os

    return os.path.getsize(path)


def silence_frames(duration_s: float, sample_rate: int = DEFAULT_SAMPLE_RATE) -> List[bytes]:
    frame_samples = sample_rate * FRAME_MS // 1000
    n = int(duration_s * 1000 / FRAME_MS)
    quiet = b"\x00\x00" * frame_samples
    return [quiet] * n


def tone_frames(
    duration_s: float,
    freq: float = 440.0,
    amplitude: int = 8000,
    sample_rate: int = DEFAULT_SAMPLE_RATE,
) -> List[bytes]:
    import math

    frame_samples = sample_rate * FRAME_MS // 1000
    n = int(duration_s * 1000 / FRAME_MS)
    frames = []
    phase = 0.0
    step = 2 * math.pi * freq / sample_rate
    for _ in range(n):
        samples = []
        for _i in range(frame_samples):
            samples.append(int(amplitude * math.sin(phase)))
            phase += step
        frames.append(struct.pack(f"<{frame_samples}h", *samples))
    return frames


class SyntheticSource(AudioSource):
    """Deterministic frames for tests."""

    id = "synthetic"

    def __init__(self, frames: List[bytes], sample_rate: int = DEFAULT_SAMPLE_RATE):
        self._frames = frames
        self._rate = sample_rate

    @property
    def sample_rate(self) -> int:
        return self._rate

    def frames(self, frame_samples: int) -> Iterator[bytes]:
        for f in self._frames:
            yield f

    def close(self) -> None:
        return None


def create_source(kind: str, path: Optional[str] = None) -> AudioSource:
    if kind == "wav":
        if not path:
            raise AudioError("wav source requires a path")
        return WavSource(path)
    if kind == "mic":
        return MicSource()
    raise AudioError(f"unknown audio source `{kind}`")