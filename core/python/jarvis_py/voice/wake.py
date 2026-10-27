"""Wake-word matching.

Text-level: given a transcript (from STT), decide whether it *starts* with a
configured wake word ("Jake", "Hey Jake", "Computer" …) and strip it so the
rest becomes the command. Matching is normalised (case, punctuation) and
supports multi-word phrases.

The audio-level flow (VAD → segment → STT → this module) lives in `pipeline.py`;
microphone streaming replaces the source, not the logic, so the same code is
tested offline.
"""

from __future__ import annotations

import re
import unicodedata
from dataclasses import dataclass, field
from typing import Iterable, List, Optional, Sequence, Tuple

_PUNCT_RE = re.compile(r"[^\w\s]", re.UNICODE)
_WS_RE = re.compile(r"\s+")


def normalise(text: str) -> str:
    """Casefold, strip punctuation, collapse whitespace — for tolerant matching."""
    text = unicodedata.normalize("NFKC", text or "")
    text = _PUNCT_RE.sub(" ", text)
    return _WS_RE.sub(" ", text).strip().lower()


@dataclass
class WakeResult:
    woke: bool
    #: transcript with the wake phrase removed ("" when not awake)
    remainder: str
    #: the wake phrase that matched, if any
    matched: str = ""
    #: STT confidence (0..1) when the engine supplied one
    confidence: float = 1.0
    #: the full original transcript, kept for logs/memory
    transcript: str = ""

    def to_wire(self) -> dict:
        return {
            "woke": self.woke,
            "remainder": self.remainder,
            "matched": self.matched,
            "confidence": self.confidence,
            "transcript": self.transcript,
        }


class WakeWordEngine:
    def __init__(self, wake_words: Optional[Sequence[str]] = None):
        words = [w for w in (wake_words or []) if w and str(w).strip()]
        # normalise for matching, but remember the original spelling so results
        # can say "Jake", not "Jake".
        self._original: dict[str, str] = {}
        for w in words:
            norm = normalise(w)
            if norm and norm not in self._original:
                self._original[norm] = str(w).strip()
        # longest phrases first so "hey Jake" wins over "Jake".
        self.wake_words: List[str] = sorted(self._original.keys(), key=len, reverse=True)

    @classmethod
    def from_config(cls, value: str) -> "WakeWordEngine":
        """Accepts `"Jake, Hey Jake"` (comma-separated) or a single word."""
        parts = [p.strip() for p in (value or "").split(",")]
        return cls([p for p in parts if p])

    def check(
        self,
        transcript: str,
        confidence: float = 1.0,
        min_confidence: float = 0.0,
        require_prefix: bool = True,
    ) -> WakeResult:
        """Decide whether `transcript` is addressed to us.

        `require_prefix=True` (default) demands the wake word at the *start* —
        that's what a real always-on detector sees. Set to `False` only for
        grammar-based engines that already proved the utterance contained it.
        """
        norm = normalise(transcript)
        if not norm:
            return WakeResult(False, "", transcript=transcript or "")
        if confidence < min_confidence:
            # Too uncertain: refuse rather than act on a hallucinated wake word.
            return WakeResult(
                False, "", confidence=confidence, transcript=transcript
            )
        for wake in self.wake_words:
            if not wake:
                continue
            original = self._original.get(wake, wake)
            if norm == wake:
                return WakeResult(True, "", original, confidence, transcript)
            if norm.startswith(wake + " "):
                remainder = norm[len(wake) :].strip()
                # restore an approximate original casing slice for display
                raw = transcript.strip()
                stripped = raw
                if raw.lower().startswith(wake):
                    stripped = raw[len(wake) :].lstrip(" ,.!")
                return WakeResult(True, stripped or remainder, original, confidence, transcript)
            if not require_prefix and re.search(rf"\b{re.escape(wake)}\b", norm):
                remainder = norm.replace(wake, " ", 1).strip()
                return WakeResult(True, remainder, original, confidence, transcript)
        return WakeResult(False, "", confidence=confidence, transcript=transcript)

    def any_match(self, transcript: str, confidence: float = 1.0, min_confidence: float = 0.0) -> bool:
        return self.check(transcript, confidence, min_confidence, require_prefix=False).woke

    def describe(self) -> dict:
        return {"wake_words": list(self.wake_words)}


@dataclass
class ConversationState:
    """Tiny state machine for the voice loop: idle → listening → acting."""

    awake: bool = False
    last_wake: str = ""
    turns: int = 0
    history: List[Tuple[str, str]] = field(default_factory=list)

    def record(self, user: str, assistant: str) -> None:
        self.history.append((user, assistant))
        if len(self.history) > 20:
            self.history = self.history[-20:]

    def reset(self) -> None:
        self.awake = False
        self.last_wake = ""


def wake_words_from_iterable(words: Iterable[str]) -> WakeWordEngine:
    return WakeWordEngine(list(words))