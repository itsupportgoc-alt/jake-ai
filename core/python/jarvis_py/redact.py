"""Secret redaction applied before anything leaves this process for a cloud provider.

Mirrors `jarvis_core::logging::redact` in Rust: a small scanner, no regex engine,
no dependencies. Two layers:

1. **Key-shaped values** (``sk-...``, ``ghp_...``, JWTs, AWS keys …) are masked
   wherever they appear.
2. **Assignments** (``api_key=...``, ``password: ...``, ``Bearer ...``) are masked.

The runtime redacts logs the same way, so a secret printed anywhere is masked twice.
"""

from __future__ import annotations

from typing import Iterable, List, Tuple

SENSITIVE_KEYS: Tuple[str, ...] = (
    "api_key",
    "apikey",
    "api-key",
    "access_token",
    "refresh_token",
    "token",
    "password",
    "passwd",
    "passphrase",
    "secret",
    "client_secret",
    "authorization",
    "auth",
    "credential",
    "credentials",
    "private_key",
    "session",
    "cookie",
)

SECRET_PREFIXES: Tuple[str, ...] = (
    "sk-ant-",
    "sk-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "github_pat_",
    "AKIA",
    "ASIA",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxs-",
    "eyJ",
    "AIza",
    "glpat-",
)

_TOKEN_EXTRA = set("_-./+= ")


def _is_token_char(ch: str) -> bool:
    return ch.isalnum() or ch in _TOKEN_EXTRA


def _match_assignment(text: str, i: int) -> Tuple[int, str] | None:
    lowered = text[i:].lower()
    for key in SENSITIVE_KEYS:
        if not lowered.startswith(key):
            continue
        if i > 0 and (text[i - 1].isalnum() or text[i - 1] == "_"):
            continue
        j = i + len(key)
        if j < len(text) and (text[j].isalnum() or text[j] == "_"):
            continue
        while j < len(text) and text[j].isspace():
            j += 1
        if j >= len(text) or text[j] not in "=:":
            continue
        j += 1
        while j < len(text) and text[j].isspace():
            j += 1
        if j >= len(text):
            return None
        k = j
        while k < len(text) and _is_token_char(text[k]):
            k += 1
        if k == j:
            return None
        return k - i, text[i:j] + "***redacted***"
    return None


def _match_prefix(text: str, i: int) -> Tuple[int, str] | None:
    for prefix in SECRET_PREFIXES:
        if not text.startswith(prefix, i):
            continue
        if i > 0 and prefix[0].isalpha():
            prev = text[i - 1]
            if prev.isalnum() or prev in "_-":
                continue
        j = i + len(prefix)
        while j < len(text) and _is_token_char(text[j]):
            j += 1
        length = j - i
        return length, "***redacted-long***" if length > 24 else "***redacted***"
    return None


def redact(text: str) -> str:
    """Mask secret-shaped substrings. Pure function, always returns a new string."""
    if not text:
        return text
    out: List[str] = []
    i = 0
    n = len(text)
    while i < n:
        hit = _match_assignment(text, i) or _match_prefix(text, i)
        if hit:
            consume, replacement = hit
            out.append(replacement)
            i += consume
            continue
        out.append(text[i])
        i += 1
    return "".join(out)


def redact_obj(value, key: str | None = None):
    """Recursively redact strings inside dicts/lists (used before cloud egress).

    A dict entry whose *key* is sensitive has its value masked outright — the value
    itself carries no prefix we could recognise.
    """
    if isinstance(value, str):
        if key is not None and key.lower() in SENSITIVE_KEYS:
            return "***redacted***"
        return redact(value)
    if isinstance(value, dict):
        return {k: redact_obj(v, key=k) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [redact_obj(v) for v in value]
    return value


def looks_like_secret(text: str) -> bool:
    """Used by memory writes: refuse to store something that smells like a secret."""
    return redact(text) != text


def iter_findings(text: str) -> Iterable[str]:
    """Diagnostic helper for tests: yields the masked regions of `text`."""
    red = redact(text)
    if red != text:
        yield red