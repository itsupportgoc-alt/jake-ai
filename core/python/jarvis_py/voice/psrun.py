"""Shared helpers for running the platform PowerShell snippets safely.

Kept in one module so every voice engine handles subprocess output the same way.
The important subtlety: `b""` is falsy, so `(proc.stdout or "")` silently converts
*empty bytes* into an empty *string* and crashes on `.decode()` — `stdout_text()`
never does that.
"""

from __future__ import annotations

import os
import subprocess
from typing import List, Optional

POWERSHELL = "powershell"

# CREATE_NO_WINDOW — never flash a console for background speech/recognition.
_CREATE_NO_WINDOW = 0x0800_0000


def stdout_text(proc: subprocess.CompletedProcess) -> str:
    """UTF-8 text of `proc.stdout`, whatever its type — never raises."""
    raw = getattr(proc, "stdout", None)
    if raw is None:
        return ""
    if isinstance(raw, (bytes, bytearray)):
        return raw.decode("utf-8", errors="replace").strip()
    return str(raw).strip()


def stderr_text(proc: subprocess.CompletedProcess) -> str:
    raw = getattr(proc, "stderr", None)
    if raw is None:
        return ""
    if isinstance(raw, (bytes, bytearray)):
        return raw.decode("utf-8", errors="replace").strip()
    return str(raw).strip()


def run_ps(
    script: str,
    timeout_s: float,
    *,
    capture: bool = True,
) -> subprocess.CompletedProcess:
    """Run a PowerShell script non-interactively with a hard timeout."""
    args: List[str] = [
        POWERSHELL,
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]
    kwargs = dict(
        timeout=timeout_s,
        creationflags=_CREATE_NO_WINDOW if os.name == "nt" else 0,
    )
    if capture:
        kwargs["capture_output"] = True
    return subprocess.run(args, **kwargs)  # noqa: S603 - fixed arg vector


def popen_ps(script: str) -> subprocess.Popen:
    """Start a PowerShell script as a tracked child (interruptible speech)."""
    return subprocess.Popen(  # noqa: S603
        [
            POWERSHELL,
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        creationflags=_CREATE_NO_WINDOW if os.name == "nt" else 0,
    )


def b64_encode(text: str) -> str:
    import base64

    return base64.b64encode(text.encode("utf-8")).decode("ascii")


def decode_safe(value: Optional[str]) -> str:
    import base64

    try:
        return base64.b64decode(value or "").decode("utf-8")
    except Exception:  # noqa: BLE001
        return ""