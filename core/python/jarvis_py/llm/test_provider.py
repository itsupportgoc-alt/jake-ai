"""Deterministic offline provider used by tests and by `llm.provider = "test"`.

It is **not** a chatbot pretending to be a model: it maps recognisable requests onto
real tool calls so the whole pipeline (brain → gate → tool → verification) can be
exercised with no network and no API key. Anything it cannot map, it says so.
"""

from __future__ import annotations

import re
from typing import Any, Dict, List, Optional

from .base import LLMProvider, LLMResult, ToolCall

_TIME = re.compile(r"\b(time|date|today|what day)\b", re.I)
_SYS = re.compile(r"\b(system|computer info|hostname|cpu|cores|what is this machine)\b", re.I)
_PROCS = re.compile(r"\b(processes|running apps|using (my )?ram|memory usage|tasklist)\b", re.I)
_LIST = re.compile(r"(?:\b(?:list|show)\b.*\b(?:dir|folder|directory|files?)\b|\bwhat(?:'s| is) in\b)", re.I)
_READ = re.compile(r"\bread\s+(\S+\.\w{1,6})\b", re.I)
_SEARCH = re.compile(r"\bfind\s+(files?|a file)\s*(?:named\s+|called\s+|containing\s+)?[\"']?([^\"']+)[\"']?", re.I)
_DELETE = re.compile(r"\bdelete\s+(?:file\s+)?([^\s]+\.\w{1,6})\b", re.I)
_SCREENSHOT = re.compile(r"\b(?:take|capture)\s+a?\s*(?:screenshot|screen shot|screen capture)\b|\bscreenshot\b", re.I)
_CLOSE = re.compile(r"\bclose\s+(?:the\s+)?([a-z][\w\-]{0,40})\b", re.I)
_OPEN = re.compile(r"\bopen\s+(?:the\s+)?(?:app\s+)?([a-z][\w\- ]{0,40})\b", re.I)
_FILEISH = re.compile(r"\.\w{1,6}$")


class TestProvider(LLMProvider):
    id = "test"
    # keep pytest from trying to collect this class as a test
    __test__ = False

    def __init__(self, model: str = "test-offline-1"):
        self._model = model

    def complete(
        self,
        messages: List[Dict[str, Any]],
        tools: Optional[List[Dict[str, Any]]] = None,
        *,
        max_tokens: int = 2048,
        temperature: float = 0.4,
        timeout: float = 60.0,
    ) -> LLMResult:
        available = {t["function"]["name"] for t in (tools or []) if "function" in t}
        tool_results = [m for m in messages if m.get("role") == "tool"]
        last_user = next(
            (m.get("content", "") for m in reversed(messages) if m.get("role") == "user"),
            "",
        )

        # 1) A tool already answered: verify + summarise honestly.
        if tool_results:
            return self._summarise(tool_results[-1], last_user)

        # 2) Map the request onto a real tool call.
        call = self._route(last_user, available)
        if call is not None:
            return LLMResult(
                content="",
                tool_calls=[call],
                model=self._model,
                provider=self.id,
                finish_reason="tool_calls",
            )

        # 3) No tool needed: answer plainly (offline, so keep it short and honest).
        return LLMResult(
            content=(
                "I am running with the offline test provider, so I have no language model "
                "behind me yet. I can still operate real tools — try “what time is it”, "
                "“list the files in the project”, “read README.md”, “find files named Cargo”, "
                "“what's using my RAM”, or “system info”. Configure llm.provider = "
                "openai_compat with an API key for full conversation."
            ),
            model=self._model,
            provider=self.id,
            finish_reason="stop",
        )

    def health(self) -> Dict[str, Any]:
        return {
            "ok": True,
            "provider": self.id,
            "model": self._model,
            "detail": "offline test provider (no network required)",
        }

    # ------------------------------------------------------------------ helpers
    def _route(self, text: str, available: set) -> Optional[ToolCall]:
        def call(name: str, args: Dict[str, Any]) -> Optional[ToolCall]:
            if name in available:
                return ToolCall(id="call_test_1", name=name, arguments=args)
            return None

        if match := _SEARCH.search(text):
            return call("fs.search", {"query": match.group(2).strip()})
        if match := _READ.search(text):
            return call("fs.read", {"path": match.group(1)})
        if match := _DELETE.search(text):
            return call("fs.delete", {"paths": [match.group(1)]})
        if _SCREENSHOT.search(text):
            return call("screenshot", {})
        if _TIME.search(text):
            return call("time.now", {})
        if _SYS.search(text):
            return call("sys.info", {})
        if _PROCS.search(text):
            return call("app.list", {})
        if _LIST.search(text):
            return call("fs.list", {"path": "."})
        if match := _CLOSE.search(text):
            candidate = match.group(1).strip()
            if candidate and not candidate.lower().startswith(("the ", "files", "folder", "window of")):
                return call("app.close", {"app": candidate})
        if match := _OPEN.search(text):
            candidate = match.group(1).strip().rstrip(". ")
            if candidate:
                if _FILEISH.search(candidate):
                    return call("fs.read", {"path": candidate})
                return call("app.open", {"app": candidate})
        if re.search(r"\b(open|show)\b.+\b(folder|directory|files)\b", text, re.I):
            return call("fs.list", {"path": "."})
        return None

    def _summarise(self, tool_message: Dict[str, Any], user_text: str) -> LLMResult:
        raw = tool_message.get("content", "")
        try:
            import json

            payload = json.loads(raw) if isinstance(raw, str) else raw
        except Exception:  # noqa: BLE001
            payload = {"ok": False, "error": "unreadable tool result"}

        ok = bool(payload.get("ok"))
        data = payload.get("data")
        error = payload.get("error")

        if not ok:
            summary = (
                f"I could not do that: {error or payload.get('error_code') or 'the tool reported a failure'}. "
                "Nothing was changed."
            )
        else:
            summary = _describe(data, user_text)

        return LLMResult(
            content=summary,
            model=self._model,
            provider=self.id,
            finish_reason="stop",
        )


def _describe(data: Any, user_text: str) -> str:
    if not isinstance(data, dict):
        return f"Done — the tool returned: {str(data)[:400]}"

    if "utc_time" in data:
        return f"It is {data.get('utc_time')} UTC ({data.get('local')})."
    if "hostname" in data:
        return (
            f"{data.get('hostname')} is running {data.get('os')} ({data.get('arch')}), "
            f"{data.get('cpu_count')} CPU cores; JARVIS v{data.get('jarvis_version')} "
            f"rooted at {data.get('project_root')}."
        )
    if "processes" in data:
        top = ", ".join(
            f"{p.get('name')} ({p.get('pid')})" for p in data.get("processes", [])[:5]
        )
        return f"I can see {data.get('count')} processes — {top}."
    if "matches" in data:
        if not data.get("count"):
            return "I found no files matching that query."
        names = ", ".join(m.get("name", "?") for m in data.get("matches", [])[:5])
        return f"I found {data.get('count')} file(s): {names}."
    if "content" in data:
        body = str(data.get("content"))
        head = body[:300]
        suffix = "…" if data.get("truncated") else ""
        return f"{data.get('path')} ({data.get('size')} bytes): {head}{suffix}"
    if "entries" in data:
        names = ", ".join(str(e.get("name")) for e in data.get("entries", [])[:8])
        return f"{data.get('path')} contains {data.get('total')} entries: {names}."
    return f"Done — {str(data)[:400]}"