"""Minimal HTTP transport built on the standard library.

Deliberately not `requests`/`httpx`: Phase 1 must run on a stock Python install.
Swapping this module for an async client later is a one-file change.
"""

from __future__ import annotations

import json
import socket
import urllib.error
import urllib.request
from typing import Any, Dict, Optional, Tuple

from .base import ProviderError

DEFAULT_USER_AGENT = "jarvis-brain/1.0"


def post_json(
    url: str,
    payload: Dict[str, Any],
    headers: Dict[str, str],
    timeout: float,
) -> Tuple[int, Dict[str, Any]]:
    """POST `payload` as JSON. Returns (status, body).

    Raises `ProviderError` with a classified code for network/HTTP failures.
    """
    body = json.dumps(payload).encode("utf-8")
    request_headers = {"Content-Type": "application/json", "User-Agent": DEFAULT_USER_AGENT}
    request_headers.update(headers)
    req = urllib.request.Request(url, data=body, headers=request_headers, method="POST")

    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310 - URL comes from user config
            status = resp.status
            raw = resp.read()
    except urllib.error.HTTPError as exc:
        raw = exc.read() if hasattr(exc, "read") else b""
        detail = raw.decode("utf-8", errors="replace")[:2000]
        status = exc.code
        if status in (401, 403):
            raise ProviderError(
                "auth",
                f"the provider rejected the credentials (HTTP {status}): {detail}",
            ) from exc
        if status == 429:
            raise ProviderError("rate_limited", f"rate limited (HTTP 429): {detail}", retryable=True)
        if status >= 500:
            raise ProviderError("bad_response", f"provider server error (HTTP {status}): {detail}", retryable=True)
        raise ProviderError("bad_response", f"provider returned HTTP {status}: {detail}") from exc
    except urllib.error.URLError as exc:
        reason = getattr(exc, "reason", exc)
        if isinstance(reason, socket.timeout):
            raise ProviderError("timeout", f"request timed out after {timeout:.0f}s", retryable=True) from exc
        raise ProviderError("network", f"cannot reach the provider: {reason}", retryable=True) from exc
    except socket.timeout as exc:
        raise ProviderError("timeout", f"request timed out after {timeout:.0f}s", retryable=True) from exc

    text = raw.decode("utf-8", errors="replace")
    if not text.strip():
        return status, {}
    try:
        parsed = json.loads(text)
    except json.JSONDecodeError as exc:
        raise ProviderError("bad_response", f"provider returned non-JSON body: {text[:300]}") from exc
    if not isinstance(parsed, dict):
        return status, {"value": parsed}
    return status, parsed


def get_json(url: str, headers: Dict[str, str], timeout: float) -> Tuple[int, Dict[str, Any]]:
    request_headers = {"User-Agent": DEFAULT_USER_AGENT}
    request_headers.update(headers)
    req = urllib.request.Request(url, headers=request_headers, method="GET")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310
            raw = resp.read()
            status = resp.status
    except urllib.error.HTTPError as exc:
        raise ProviderError("bad_response", f"HTTP {exc.code} from {url}") from exc
    except urllib.error.URLError as exc:
        raise ProviderError("network", f"cannot reach {url}: {getattr(exc, 'reason', exc)}", retryable=True) from exc
    try:
        return status, json.loads(raw.decode("utf-8", errors="replace") or "{}")
    except json.JSONDecodeError as exc:
        raise ProviderError("bad_response", f"non-JSON response from {url}") from exc