"""Newline-delimited JSON-RPC 2.0 over stdio.

Design notes
------------
* One thread reads stdin forever. Responses are matched to pending calls by id;
  requests are handed to a small thread pool so a handler that itself needs to call
  the runtime (``tool.execute``) never blocks the reader.
* Writes are serialised under a lock — two threads interleaving a line would corrupt
  the stream.
* Malformed frames are answered with a proper JSON-RPC error instead of killing the
  process (the Rust side logs and continues).
"""

from __future__ import annotations

import json
import sys
import threading
import traceback
from concurrent.futures import ThreadPoolExecutor
from typing import Any, Callable, Dict, Optional

# JSON-RPC error codes (mirrors core/rust/crates/jarvis-protocol/src/rpc.rs)
PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
INTERNAL_ERROR = -32603
BRAIN_ERROR = -32001
TIMEOUT = -32002
PERMISSION_DENIED = -32003
CANCELLED = -32004
PEER_DISCONNECTED = -32005

Handler = Callable[[Any], Any]


class RpcError(Exception):
    def __init__(self, code: int, message: str, data: Any = None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.data = data

    def to_wire(self) -> Dict[str, Any]:
        err: Dict[str, Any] = {"code": self.code, "message": self.message}
        if self.data is not None:
            err["data"] = self.data
        return err


class RpcServer:
    """Serves handlers on stdin/stdout and can call back into the Rust runtime."""

    def __init__(self, stdin=None, stdout=None, workers: int = 8):
        self._stdin = stdin or sys.stdin.buffer
        self._stdout = stdout or sys.stdout.buffer
        self._write_lock = threading.Lock()
        self._handlers: Dict[str, Handler] = {}
        self._notifications: Dict[str, Handler] = {}
        self._pending: Dict[int, threading.Event] = {}
        self._pending_results: Dict[int, Any] = {}
        self._pending_lock = threading.Lock()
        self._next_id = 1
        self._id_lock = threading.Lock()
        self._executor = ThreadPoolExecutor(max_workers=workers, thread_name_prefix="jarvis-rpc")
        self._running = threading.Event()
        self._running.set()

    # ---------------------------------------------------------------- registration
    def handle(self, method: str, handler: Handler) -> None:
        self._handlers[method] = handler

    def handle_notification(self, method: str, handler: Handler) -> None:
        self._notifications[method] = handler

    def has_handler(self, method: str) -> bool:
        return method in self._handlers or method in self._notifications

    # -------------------------------------------------------------------- sending
    def _write(self, payload: Dict[str, Any]) -> None:
        data = (json.dumps(payload, ensure_ascii=False, default=str) + "\n").encode("utf-8")
        with self._write_lock:
            self._stdout.write(data)
            self._stdout.flush()

    def notify(self, method: str, params: Any = None) -> None:
        self._write({"jsonrpc": "2.0", "method": method, "params": params if params is not None else {}})

    def call(self, method: str, params: Any = None, timeout: float = 30.0) -> Any:
        """Request/response call towards the Rust runtime."""
        with self._id_lock:
            req_id = self._next_id
            self._next_id += 1
        done = threading.Event()
        with self._pending_lock:
            self._pending[req_id] = done
            self._pending_results.pop(req_id, None)
        self._write({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": method,
            "params": params if params is not None else {},
        })
        if not done.wait(timeout):
            with self._pending_lock:
                self._pending.pop(req_id, None)
            raise RpcError(TIMEOUT, f"`{method}` timed out after {timeout}s")
        with self._pending_lock:
            self._pending.pop(req_id, None)
            result = self._pending_results.pop(req_id, None)
        if isinstance(result, dict) and "__error__" in result:
            err = result["__error__"]
            raise RpcError(err.get("code", INTERNAL_ERROR), err.get("message", "peer error"), err.get("data"))
        return result

    # ------------------------------------------------------------------- serving
    def serve_forever(self) -> None:
        for raw in self._stdin:
            if not self._running.is_set():
                break
            line = raw.decode("utf-8", errors="replace").strip() if isinstance(raw, (bytes, bytearray)) else str(raw).strip()
            if not line:
                continue
            self._submit_line(line)
        self._running.clear()
        self._executor.shutdown(wait=False)

    def stop(self) -> None:
        self._running.clear()

    def _submit_line(self, line: str) -> None:
        try:
            frame = json.loads(line)
        except json.JSONDecodeError as exc:
            self._write({
                "jsonrpc": "2.0",
                "id": None,
                "error": {"code": PARSE_ERROR, "message": f"invalid JSON: {exc}"},
            })
            return
        if not isinstance(frame, dict):
            self._write({
                "jsonrpc": "2.0",
                "id": None,
                "error": {"code": INVALID_REQUEST, "message": "frame must be an object"},
            })
            return

        if "method" in frame:
            self._executor.submit(self._serve_request, frame)
        else:
            self._resolve_response(frame)

    def _resolve_response(self, frame: Dict[str, Any]) -> None:
        req_id = frame.get("id")
        if not isinstance(req_id, (int, float)):
            return
        req_id = int(req_id)
        with self._pending_lock:
            done = self._pending.get(req_id)
            if "error" in frame and frame["error"] is not None:
                self._pending_results[req_id] = {"__error__": frame["error"]}
            else:
                self._pending_results[req_id] = frame.get("result")
        if done is not None:
            done.set()

    def _serve_request(self, frame: Dict[str, Any]) -> None:
        method = frame.get("method")
        req_id = frame.get("id")
        params = frame.get("params")
        is_notification = req_id is None
        try:
            handler = self._handlers.get(method) or self._notifications.get(method)
            if handler is None:
                raise RpcError(METHOD_NOT_FOUND, f"method `{method}` is not handled by the brain")
            result = handler(params)
            if is_notification:
                return
            self._write({"jsonrpc": "2.0", "id": req_id, "result": result})
        except RpcError as exc:
            if is_notification:
                return
            self._write({"jsonrpc": "2.0", "id": req_id, "error": exc.to_wire()})
        except Exception as exc:  # noqa: BLE001 - the peer must learn about any failure
            if is_notification:
                return
            self._write({
                "jsonrpc": "2.0",
                "id": req_id,
                "error": {
                    "code": INTERNAL_ERROR,
                    "message": f"{type(exc).__name__}: {exc}",
                    "data": {"traceback": traceback.format_exc(limit=6)},
                },
            })

    # ------------------------------------------------------------------ helpers
    def log(self, level: str, message: str, **fields: Any) -> None:
        """Route a structured log line into the Rust logger (redaction happens there too)."""
        try:
            self.notify("log.event", {"level": level, "message": message, "fields": fields})
        except Exception:  # noqa: BLE001 - logging must never break the brain
            pass