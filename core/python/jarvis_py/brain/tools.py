"""Tool-call parsing and validation on the brain side.

The Rust registry is the authority; this module only filters the obviously broken
calls *before* they cross the bridge, and normalises them to the protocol shape.
"""

from __future__ import annotations

import json
from typing import Any, Dict, List, Sequence

from ..llm.base import ToolCall


def allowed_tool_names(tools: Sequence[Dict[str, Any]]) -> set:
    names = set()
    for t in tools:
        fn = t.get("function") if isinstance(t, dict) else None
        if isinstance(fn, dict) and fn.get("name"):
            names.add(fn["name"])
        elif isinstance(t, dict) and t.get("name"):
            names.add(t["name"])
    return names


def normalise_calls(calls: Sequence[ToolCall], allowed: set) -> tuple[List[ToolCall], List[str]]:
    """Split proposed calls into (kept, rejected-with-reason).

    Rejected calls are reported back to the model as tool errors by the orchestrator,
    so the model learns what it did wrong instead of silently failing.
    """
    kept: List[ToolCall] = []
    rejected: List[str] = []
    for i, call in enumerate(calls):
        if not call.name:
            rejected.append(f"call {i}: empty tool name")
            continue
        if call.name not in allowed:
            rejected.append(f"unknown tool `{call.name}` (allowed: {', '.join(sorted(allowed)) or 'none'})")
            continue
        if "__parse_error__" in call.arguments:
            rejected.append(f"arguments for `{call.name}` were not valid JSON")
            continue
        if not isinstance(call.arguments, dict):
            rejected.append(f"arguments for `{call.name}` must be a JSON object")
            continue
        kept.append(call)
    return kept, rejected


def tool_error_result(call_id: str, name: str, reason: str) -> Dict[str, Any]:
    """A protocol-shaped 'tool result' describing a rejected call."""
    return {
        "ok": False,
        "data": None,
        "error": reason,
        "error_code": "invalid_tool_call",
    } | {"__id": call_id, "__name": name}


def serialise_args(args: Dict[str, Any]) -> str:
    try:
        return json.dumps(args, ensure_ascii=False)
    except (TypeError, ValueError):
        return json.dumps({"__unserialisable__": True})
