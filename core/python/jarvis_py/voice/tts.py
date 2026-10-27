"""Text-to-speech engines behind a single interface.

`TtsEngine` is what the rest of the system talks to; the Windows implementation
uses the built-in .NET `System.Speech` synthesizer (works offline, no packages),
and every call is verifiable: `speak_to_wav` writes a real RIFF file we can check.
"""

from __future__ import annotations

import base64
import os
import subprocess
from abc import ABC, abstractmethod
from .psrun import stdout_text
from typing import Any, Dict, Optional

_POWERSHELL = "powershell"


def _b64(text: str) -> str:
    """Encode for safe embedding in a PowerShell script (no quoting hazards)."""
    return base64.b64encode(text.encode("utf-8")).decode("ascii")


class TtsEngine(ABC):
    id: str = "abstract"

    @abstractmethod
    def available(self) -> Dict[str, Any]:
        """Probe: {ok, detail, voices}. Must never raise."""

    @abstractmethod
    def speak(self, text: str, rate: int = 0, volume: int = 100) -> Dict[str, Any]:
        """Play `text` aloud. Returns {ok, detail, elapsed_ms}."""

    @abstractmethod
    def speak_to_wav(self, text: str, path: str, rate: int = 0, volume: int = 100) -> Dict[str, Any]:
        """Render `text` into a WAV file (used by tests and offline verification)."""

    @abstractmethod
    def stop(self) -> Dict[str, Any]:
        """Interrupt any in-flight speech (barge-in / emergency stop)."""


class SystemSpeechTts(TtsEngine):
    """Windows .NET `System.Speech.Synthesis.SpeechSynthesizer` via PowerShell."""

    id = "windows_system_speech"

    def __init__(self, timeout_s: float = 60.0):
        self._timeout_s = timeout_s
        self._proc: Optional[subprocess.Popen] = None
        self._proc_lock = __import__("threading").Lock()

    def _run_interruptible(self, script: str, timeout_s: float) -> subprocess.CompletedProcess:
        """Run a script as a tracked child so `stop()` can kill it mid-speech."""
        proc = subprocess.Popen(
            [
                _POWERSHELL,
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            creationflags=0x0800_0000 if os.name == "nt" else 0,
        )
        with self._proc_lock:
            self._proc = proc
        try:
            stdout, stderr = proc.communicate(timeout=timeout_s)
            return subprocess.CompletedProcess(proc.args, proc.returncode, stdout, stderr)
        except subprocess.TimeoutExpired:
            proc.kill()
            stdout, stderr = proc.communicate()
            raise
        finally:
            with self._proc_lock:
                self._proc = None

    # ------------------------------------------------------------------ helpers
    def _run(self, script: str, timeout_s: Optional[float] = None) -> subprocess.CompletedProcess:
        # -NoProfile: don't load the user's profile (faster, deterministic).
        return subprocess.run(
            [
                _POWERSHELL,
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ],
            capture_output=True,
            timeout=timeout_s or self._timeout_s,
            creationflags=0x0800_0000 if os.name == "nt" else 0,  # CREATE_NO_WINDOW
        )

    def available(self) -> Dict[str, Any]:
        script = r"""
        try {
            Add-Type -AssemblyName System.Speech -ErrorAction Stop
            $s = New-Object System.Speech.Synthesis.SpeechSynthesizer -ErrorAction Stop
            $voices = @($s.GetInstalledVoices() | ForEach-Object { $_.VoiceInfo.Name })
            $s.Dispose()
            if ($voices.Count -eq 0) { Write-Output ('OK|no voices installed') }
            else { Write-Output ('OK|' + ($voices -join ',')) }
        } catch {
            Write-Output ('ERR|' + $_.Exception.Message)
        }
        """
        try:
            proc = self._run(script, timeout_s=20)
        except (OSError, subprocess.TimeoutExpired) as exc:
            return {"ok": False, "detail": f"powershell unavailable: {exc}", "voices": []}
        out = stdout_text(proc)
        if out.startswith("OK|"):
            voices = [v for v in out[3:].split(",") if v]
            return {"ok": True, "detail": "System.Speech available", "voices": voices}
        return {"ok": False, "detail": out or "no output from probe", "voices": []}

    def _script_for(self, text: str, wav_path: Optional[str], rate: int, volume: int) -> str:
        # Everything user-controlled travels as base64 — no injection surface.
        text_b64 = _b64(text)
        wav_b64 = _b64(wav_path) if wav_path else ""
        rate = max(-10, min(10, int(rate)))
        volume = max(0, min(100, int(volume)))
        return f"""
        $ErrorActionPreference = 'Stop'
        try {{
            Add-Type -AssemblyName System.Speech
            $s = New-Object System.Speech.Synthesis.SpeechSynthesizer
            $s.Rate = {rate}
            $s.Volume = {volume}
            $text = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{text_b64}'))
            $wav  = '{wav_b64}'
            if ($wav -ne '') {{
                $path = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($wav))
                $s.SetOutputToWaveFile($path)
                $s.Speak($text)
                $s.Dispose()
                $len = (Get-Item -LiteralPath $path).Length
                Write-Output ('WAV|' + $len)
            }} else {{
                $s.Speak($text)
                $s.Dispose()
                Write-Output 'SPOKE'
            }}
        }} catch {{
            Write-Output ('ERR|' + $_.Exception.Message)
            exit 1
        }}
        """

    # ------------------------------------------------------------------ public
    def speak(self, text: str, rate: int = 0, volume: int = 100) -> Dict[str, Any]:
        if not text.strip():
            return {"ok": False, "detail": "nothing to speak"}
        import time

        started = time.monotonic()
        try:
            proc = self._run_interruptible(self._script_for(text, None, rate, volume), self._timeout_s)
        except subprocess.TimeoutExpired:
            return {"ok": False, "detail": "speech was interrupted or timed out", "interrupted": True}
        except OSError as exc:
            return {"ok": False, "detail": f"cannot run synthesizer: {exc}"}
        out = stdout_text(proc)
        if proc.returncode != 0:
            # A kill from stop() lands here: report honestly, never as success.
            return {
                "ok": False,
                "detail": "speech was stopped",
                "interrupted": True,
            }
        if out == "SPOKE":
            return {"ok": True, "detail": "spoken", "elapsed_ms": int((time.monotonic() - started) * 1000)}
        return {"ok": False, "detail": out or "synthesizer produced no output"}

    def speak_to_wav(self, text: str, path: str, rate: int = 0, volume: int = 100) -> Dict[str, Any]:
        if not text.strip():
            return {"ok": False, "detail": "nothing to speak"}
        parent = os.path.dirname(os.path.abspath(path))
        os.makedirs(parent, exist_ok=True)
        try:
            proc = self._run(self._script_for(text, path, rate, volume))
        except subprocess.TimeoutExpired:
            return {"ok": False, "detail": f"render timed out after {self._timeout_s}s"}
        except OSError as exc:
            return {"ok": False, "detail": f"cannot run synthesizer: {exc}"}
        out = stdout_text(proc)
        if not os.path.exists(path):
            return {"ok": False, "detail": out or "no wav was written"}
        size = os.path.getsize(path)
        with open(path, "rb") as fh:
            magic = fh.read(4)
        if magic != b"RIFF":
            return {"ok": False, "detail": f"output is not a RIFF/WAV file (magic={magic!r})"}
        if size < 1024:
            return {"ok": False, "detail": f"suspiciously small wav: {size} bytes"}
        return {"ok": True, "detail": "wav written", "bytes": size, "path": path}

    def stop(self) -> Dict[str, Any]:
        """Barge-in: kill the in-flight speech child immediately."""
        with self._proc_lock:
            proc = self._proc
            if proc is not None and proc.poll() is None:
                proc.kill()
                return {"ok": True, "detail": "speech stopped"}
        return {"ok": True, "detail": "nothing speaking"}


class NullTts(TtsEngine):
    """Honest fallback: reports unavailability instead of pretending to speak."""

    id = "null"

    def available(self) -> Dict[str, Any]:
        return {"ok": False, "detail": "no TTS engine configured", "voices": []}

    def speak(self, text: str, rate: int = 0, volume: int = 100) -> Dict[str, Any]:
        return {"ok": False, "detail": "no TTS engine configured on this platform"}

    def speak_to_wav(self, text: str, path: str, rate: int = 0, volume: int = 100) -> Dict[str, Any]:
        return {"ok": False, "detail": "no TTS engine configured on this platform"}

    def stop(self) -> Dict[str, Any]:
        return {"ok": True, "detail": "nothing speaking"}


def create_tts(engine_id: Optional[str] = None) -> TtsEngine:
    engine_id = (engine_id or "auto").lower()
    if engine_id == "null":
        return NullTts()
    if engine_id in ("auto", "windows_system_speech", "system"):
        if os.name == "nt":
            return SystemSpeechTts()
        return NullTts()
    return NullTts()


def probe_wav_header(path: str) -> Dict[str, Any]:
    """Independent verification helper (also used by tests)."""
    try:
        with open(path, "rb") as fh:
            header = fh.read(12)
        if len(header) < 12 or header[0:4] != b"RIFF" or header[8:12] != b"WAVE":
            return {"ok": False, "detail": "not a RIFF/WAVE file"}
        return {"ok": True, "bytes": os.path.getsize(path), "detail": "valid wav header"}
    except OSError as exc:
        return {"ok": False, "detail": str(exc)}