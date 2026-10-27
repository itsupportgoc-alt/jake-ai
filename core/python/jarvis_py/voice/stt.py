"""Speech-to-text engines behind a single interface.

The Windows implementation probes the built-in `System.Speech.Recognition`
dictation engine. If the platform has no speech recognizer installed (Windows 11
devices increasingly ship without Windows Speech Recognition), this reports
`ok: false` with the reason — the assistant then tells the user that voice input
is unavailable instead of pretending it heard something.

Microphone capture + wake word + VAD are deliberately NOT here: they need an audio
capture stack (Phase 2 continuation, see `docs/roadmap.md`).
"""

from __future__ import annotations

import base64
import os
import subprocess
from abc import ABC, abstractmethod
from .psrun import stdout_text
from typing import Any, Dict, Optional

_POWERSHELL = "powershell"

# Probe script: instantiate the recognizer only (never blocks on audio), report
# the installed recognizers' culture + engine status.
_PROBE = r"""
$ErrorActionPreference = 'Stop'
try {
    Add-Type -AssemblyName System.Speech -ErrorAction Stop
    $r = New-Object System.Speech.Recognition.SpeechRecognizer -ErrorAction Stop
    $name = $r.Recognizer.Culture.Name
    $r.Dispose()
    Write-Output ('OK|' + $name)
} catch {
    Write-Output ('ERR|' + $_.Exception.Message)
}
"""

# Dictation script: listen for at most N seconds, print the best hypothesis as
# base64 so non-ASCII (Filipino/Taglish) text survives the pipe intact.
_DICTATE_TMPL = r"""
$ErrorActionPreference = 'Stop'
try {{
    Add-Type -AssemblyName System.Speech
    $g = New-Object System.Speech.Recognition.DictationGrammar
    $g.Name = 'dictation'
    $r = New-Object System.Speech.Recognition.SpeechRecognizer
    $r.LoadGrammar($g)
    $r.BabbleTimeout = [TimeSpan]::FromSeconds(2)
    $r.EndSilenceTimeout = [TimeSpan]::FromMilliseconds(600)
    $result = $null
    $done = New-Object System.Threading.ManualResetEvent $false
    $handler = [System.Speech.Recognition.RecognizeCompletedEventHandler] {{
        param($s, $e)
        if ($e.Result -ne $null) {{ $script:result = $e.Result.Text }}
        $done.Set() | Out-Null
    }}
    $r.add_RecognizeCompleted($handler)
    $r.RecognizeAsync()
    if (-not $done.WaitOne({timeout_ms})) {{ $r.RecognizeAsyncStop() }}
    $r.remove_RecognizeCompleted($handler)
    $r.Dispose()
    if ($script:result -eq $null) {{ Write-Output 'EMPTY' }}
    else {{
        $bytes = [Text.Encoding]::UTF8.GetBytes($script:result)
        Write-Output ('TEXT|' + [Convert]::ToBase64String($bytes))
    }}
}} catch {{
    Write-Output ('ERR|' + $_.Exception.Message)
}}
"""


class SttEngine(ABC):
    id: str = "abstract"

    @abstractmethod
    def available(self) -> Dict[str, Any]:
        """Probe: {ok, detail, culture}. Must never raise."""

    @abstractmethod
    def dictate(self, timeout_s: float = 8.0) -> Dict[str, Any]:
        """Listen once and return {ok, text|detail}."""

    @abstractmethod
    def transcribe_file(self, path: str, timeout_s: float = 15.0) -> Dict[str, Any]:
        """Transcribe a WAV file (device-free path — works without a microphone)."""

    @abstractmethod
    def detect_wake_file(
        self, path: str, wake_words: list, timeout_s: float = 15.0
    ) -> Dict[str, Any]:
        """Grammar-based wake-word match against a WAV file."""


class WindowsSpeechStt(SttEngine):
    id = "windows_system_speech"

    # Wake words arrive as a fixed Choices grammar — grammar-constrained
    # recognition is far more accurate than free dictation for short phrases.
    _WAKE_TMPL = r"""
$ErrorActionPreference = 'Stop'
try {{
    Add-Type -AssemblyName System.Speech
    $e = New-Object System.Speech.Recognition.SpeechRecognitionEngine
    $path = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{path_b64}'))
    $e.SetInputToWaveFile($path)
    $choices = New-Object System.Speech.Recognition.Choices
    {choice_blocks}
    $grammar = New-Object System.Speech.Recognition.Grammar (New-Object System.Speech.Recognition.GrammarBuilder $choices)
    $e.LoadGrammar($grammar)
    $res = $e.Recognize([TimeSpan]::FromSeconds({timeout_s}))
    $e.Dispose()
    if ($res -eq $null) {{ Write-Output 'NOMATCH' }}
    else {{
        $bytes = [Text.Encoding]::UTF8.GetBytes($res.Text)
        $conf = [Math]::Round($res.Confidence, 3)
        Write-Output ('MATCH|' + $conf + '|' + [Convert]::ToBase64String($bytes))
    }}
}} catch {{
    Write-Output ('ERR|' + $_.Exception.Message)
}}
"""

    # Free dictation over a wave file (device-free engine class — no mic needed).
    _DICTATE_WAV_TMPL = r"""
$ErrorActionPreference = 'Stop'
try {{
    Add-Type -AssemblyName System.Speech
    $e = New-Object System.Speech.Recognition.SpeechRecognitionEngine
    $path = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{path_b64}'))
    $e.SetInputToWaveFile($path)
    $g = New-Object System.Speech.Recognition.DictationGrammar
    $e.LoadGrammar($g)
    $res = $e.Recognize([TimeSpan]::FromSeconds({timeout_s}))
    $e.Dispose()
    if ($res -eq $null) {{ Write-Output 'EMPTY' }}
    else {{
        $bytes = [Text.Encoding]::UTF8.GetBytes($res.Text)
        Write-Output ('TEXT|' + [Math]::Round($res.Confidence, 3) + '|' + [Convert]::ToBase64String($bytes))
    }}
}} catch {{
    Write-Output ('ERR|' + $_.Exception.Message)
}}
"""

    def __init__(self, timeout_s: float = 20.0):
        self._timeout_s = timeout_s

    def _run(self, script: str, timeout_s: float) -> subprocess.CompletedProcess:
        return subprocess.run(
            [_POWERSHELL, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script],
            capture_output=True,
            timeout=timeout_s,
            creationflags=0x0800_0000 if os.name == "nt" else 0,
        )

    def available(self) -> Dict[str, Any]:
        try:
            proc = self._run(_PROBE, timeout_s=25)
        except (OSError, subprocess.TimeoutExpired) as exc:
            return {"ok": False, "detail": f"powershell unavailable: {exc}"}
        out = stdout_text(proc)
        if out.startswith("OK|"):
            return {"ok": True, "detail": "dictation engine ready", "culture": out[3:]}
        return {"ok": False, "detail": out[4:] if out.startswith("ERR|") else (out or "no output")}

    def dictate(self, timeout_s: float = 8.0) -> Dict[str, Any]:
        timeout_ms = int(max(2.0, timeout_s) * 1000)
        script = _DICTATE_TMPL.format(timeout_ms=timeout_ms)
        try:
            proc = self._run(script, timeout_s=timeout_s + 15)
        except subprocess.TimeoutExpired:
            return {"ok": False, "detail": "recognition timed out (no speech engine response)"}
        except OSError as exc:
            return {"ok": False, "detail": f"cannot run recognizer: {exc}"}
        out = stdout_text(proc)
        if out.startswith("TEXT|"):
            try:
                text = base64.b64decode(out[5:]).decode("utf-8")
            except Exception:  # noqa: BLE001
                return {"ok": False, "detail": "recognition returned undecodable audio text"}
            return {"ok": True, "text": text}
        if out == "EMPTY":
            return {"ok": False, "detail": "heard nothing (no speech detected before the timeout)"}
        return {"ok": False, "detail": out[4:] if out.startswith("ERR|") else (out or "no output")}

    # ------------------------------------------------- file-based (no mic) paths
    def transcribe_file(self, path: str, timeout_s: float = 15.0) -> Dict[str, Any]:
        """Dictate a WAV file via `SpeechRecognitionEngine` — the device-free engine
        class, so this works on machines without an audio device."""
        if not os.path.isfile(path):
            return {"ok": False, "detail": f"no such audio file: {path}"}
        path_b64 = base64.b64encode(os.path.abspath(path).encode("utf-8")).decode("ascii")
        script = self._DICTATE_WAV_TMPL.format(path_b64=path_b64, timeout_s=float(timeout_s))
        try:
            proc = self._run(script, timeout_s=timeout_s + 15)
        except subprocess.TimeoutExpired:
            return {"ok": False, "detail": "file transcription timed out"}
        except OSError as exc:
            return {"ok": False, "detail": f"cannot run recognizer: {exc}"}
        out = stdout_text(proc)
        if out.startswith("TEXT|"):
            parts = out[5:].split("|", 1)
            if len(parts) != 2:
                return {"ok": False, "detail": "malformed recognition output"}
            try:
                confidence = float(parts[0])
            except ValueError:
                confidence = 1.0
            try:
                text = base64.b64decode(parts[1]).decode("utf-8")
            except Exception:  # noqa: BLE001
                return {"ok": False, "detail": "recognition returned undecodable text"}
            return {"ok": True, "text": text, "confidence": confidence}
        if out == "EMPTY":
            return {"ok": False, "detail": "no speech recognized in the file"}
        return {"ok": False, "detail": out[4:] if out.startswith("ERR|") else (out or "no output")}

    def detect_wake_file(
        self, path: str, wake_words: list, timeout_s: float = 15.0
    ) -> Dict[str, Any]:
        """Grammar-constrained wake-word pass over a WAV file."""
        if not os.path.isfile(path):
            return {"ok": False, "detail": f"no such audio file: {path}"}
        words = [str(w).strip() for w in wake_words if str(w or "").strip()]
        if not words:
            return {"ok": True, "woke": False, "detail": "no wake words configured"}
        path_b64 = base64.b64encode(os.path.abspath(path).encode("utf-8")).decode("ascii")
        # Each phrase becomes a Choices entry; multi-word phrases are added as a
        # single choice so "hey Jake" is recognized as one unit.
        choice_blocks = []
        for word in words:
            b64 = base64.b64encode(word.encode("utf-8")).decode("ascii")
            choice_blocks.append(
                f"$choices.Add([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{b64}')))"
            )
        script = self._WAKE_TMPL.format(
            path_b64=path_b64,
            choice_blocks="\n    ".join(choice_blocks),
            timeout_s=float(timeout_s),
        )
        try:
            proc = self._run(script, timeout_s=timeout_s + 15)
        except subprocess.TimeoutExpired:
            return {"ok": False, "detail": "wake check timed out"}
        except OSError as exc:
            return {"ok": False, "detail": f"cannot run recognizer: {exc}"}
        out = stdout_text(proc)
        if out.startswith("MATCH|"):
            # Grammar recognition returns only the matched phrase as Text —
            # parts = [confidence, base64(matched phrase)]
            parts = out[6:].split("|", 1)
            if len(parts) != 2:
                return {"ok": False, "detail": f"malformed wake output: {out[:80]}"}
            try:
                confidence = float(parts[0])
                matched = base64.b64decode(parts[1]).decode("utf-8")
            except Exception:  # noqa: BLE001
                return {"ok": False, "detail": "undecodable wake result"}
            return {
                "ok": True,
                "woke": True,
                "matched": matched,
                "confidence": confidence,
                "transcript": matched,
                "remainder": "",
            }
        if out == "NOMATCH":
            return {"ok": True, "woke": False, "confidence": 0.0, "transcript": "", "remainder": ""}
        return {"ok": False, "detail": out[4:] if out.startswith("ERR|") else (out or "no output")}


class NullStt(SttEngine):
    id = "null"

    def available(self) -> Dict[str, Any]:
        return {"ok": False, "detail": "no speech recognition engine configured"}

    def dictate(self, timeout_s: float = 8.0) -> Dict[str, Any]:
        return {"ok": False, "detail": "speech recognition unavailable on this platform"}

    def transcribe_file(self, path: str, timeout_s: float = 15.0) -> Dict[str, Any]:
        return {"ok": False, "detail": "speech recognition unavailable on this platform"}

    def detect_wake_file(
        self, path: str, wake_words: list, timeout_s: float = 15.0
    ) -> Dict[str, Any]:
        return {"ok": False, "detail": "speech recognition unavailable on this platform"}


def create_stt(engine_id: Optional[str] = None) -> SttEngine:
    engine_id = (engine_id or "auto").lower()
    if engine_id == "null":
        return NullStt()
    if engine_id in ("auto", "windows_system_speech", "system"):
        if os.name == "nt":
            return WindowsSpeechStt()
        return NullStt()
    return NullStt()