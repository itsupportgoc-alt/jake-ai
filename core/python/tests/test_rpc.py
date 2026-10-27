import json
import threading
import time

import pytest

from jarvis_py.rpc import INTERNAL_ERROR, METHOD_NOT_FOUND, PARSE_ERROR, TIMEOUT, RpcError, RpcServer


class QueueStdin:
    """Iterable stdin fed line-by-line from the test."""

    def __init__(self):
        self._q = __import__("queue").Queue()

    def feed(self, line: str) -> None:
        self._q.put(line.encode("utf-8"))

    def feed_obj(self, obj) -> None:
        self.feed(json.dumps(obj))

    def close(self) -> None:
        self._q.put(None)

    def __iter__(self):
        return self

    def __next__(self):
        item = self._q.get(timeout=10)
        if item is None:
            raise StopIteration
        return item


class BufOut:
    def __init__(self):
        self._buf = __import__("io").BytesIO()
        self._lock = threading.Lock()

    def write(self, data: bytes) -> int:
        with self._lock:
            return self._buf.write(data)

    def flush(self) -> None:
        pass

    def text(self) -> str:
        with self._lock:
            return self._buf.getvalue().decode("utf-8")

    def responses(self):
        out = []
        for line in self.text().splitlines():
            line = line.strip()
            if line:
                out.append(json.loads(line))
        return out

    def wait_for_responses(self, count, timeout=5.0):
        deadline = time.time() + timeout
        while time.time() < deadline:
            got = self.responses()
            if len(got) >= count:
                return got
            time.sleep(0.01)
        raise AssertionError(f"expected {count} responses, got {self.responses()}")


def make_server():
    stdin, stdout = QueueStdin(), BufOut()
    server = RpcServer(stdin=stdin, stdout=stdout)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, stdin, stdout, thread


def test_serves_a_request_and_returns_the_result():
    server, stdin, stdout, _ = make_server()
    server.handle("echo", lambda params: {"echoed": params})
    stdin.feed_obj({"jsonrpc": "2.0", "id": 1, "method": "echo", "params": {"a": 1}})
    resp = stdout.wait_for_responses(1)[0]
    assert resp["id"] == 1
    assert resp["result"] == {"echoed": {"a": 1}}
    assert "error" not in resp
    stdin.close()


def test_unknown_method_returns_method_not_found():
    server, stdin, stdout, _ = make_server()
    stdin.feed_obj({"jsonrpc": "2.0", "id": 2, "method": "nope", "params": {}})
    resp = stdout.wait_for_responses(1)[0]
    assert resp["error"]["code"] == METHOD_NOT_FOUND
    assert "nope" in resp["error"]["message"]
    stdin.close()


def test_malformed_json_returns_parse_error_and_survives():
    server, stdin, stdout, _ = make_server()
    server.handle("ping", lambda params: "pong")
    stdin.feed("{broken json")
    stdin.feed_obj({"jsonrpc": "2.0", "id": 3, "method": "ping", "params": {}})
    got = stdout.wait_for_responses(2)
    assert got[0]["error"]["code"] == PARSE_ERROR
    assert got[1]["result"] == "pong"
    stdin.close()


def test_notification_produces_no_response():
    server, stdin, stdout, _ = make_server()
    seen = []
    server.handle_notification("log.event", lambda params: seen.append(params))
    stdin.feed_obj({"jsonrpc": "2.0", "method": "log.event", "params": {"message": "x"}})
    deadline = time.time() + 2
    while not seen and time.time() < deadline:
        time.sleep(0.01)
    assert seen and seen[0]["message"] == "x"
    time.sleep(0.1)
    assert stdout.responses() == []
    stdin.close()


def test_handler_exception_becomes_internal_error_with_traceback():
    server, stdin, stdout, _ = make_server()

    def boom(_params):
        raise RuntimeError("kaboom")

    server.handle("boom", boom)
    stdin.feed_obj({"jsonrpc": "2.0", "id": 4, "method": "boom", "params": {}})
    resp = stdout.wait_for_responses(1)[0]
    assert resp["error"]["code"] == INTERNAL_ERROR
    assert "kaboom" in resp["error"]["message"]
    assert "traceback" in resp["error"]["data"]
    stdin.close()


def test_rpc_error_code_is_preserved():
    server, stdin, stdout, _ = make_server()

    def denied(_params):
        raise RpcError(-32003, "permission denied: level 3")

    server.handle("tool.execute", denied)
    stdin.feed_obj({"jsonrpc": "2.0", "id": 5, "method": "tool.execute", "params": {}})
    resp = stdout.wait_for_responses(1)[0]
    assert resp["error"]["code"] == -32003
    assert "level 3" in resp["error"]["message"]
    stdin.close()


def test_outbound_call_resolves_when_the_response_arrives():
    server, stdin, stdout, _ = make_server()
    results = {}

    def worker():
        try:
            results["value"] = server.call("tool.execute", {"name": "fs.read"}, timeout=5)
        except Exception as exc:  # noqa: BLE001
            results["error"] = exc

    t = threading.Thread(target=worker)
    t.start()
    # wait for the outbound request to hit stdout, then answer it
    deadline = time.time() + 5
    req_id = None
    while time.time() < deadline and req_id is None:
        for frame in stdout.responses():
            if frame.get("method") == "tool.execute":
                req_id = frame["id"]
                break
        time.sleep(0.01)
    assert req_id is not None, "outbound request never sent"
    stdin.feed_obj({"jsonrpc": "2.0", "id": req_id, "result": {"ok": True, "data": {"content": "hi"}}})
    t.join(timeout=5)
    assert "error" not in results, results
    assert results["value"]["ok"] is True


def test_outbound_call_raises_on_error_response():
    server, stdin, stdout, _ = make_server()
    caught = {}

    def worker():
        try:
            server.call("memory.put", {}, timeout=5)
        except RpcError as exc:
            caught["err"] = exc

    t = threading.Thread(target=worker)
    t.start()
    deadline = time.time() + 5
    req_id = None
    while time.time() < deadline and req_id is None:
        for frame in stdout.responses():
            if frame.get("method") == "memory.put":
                req_id = frame["id"]
                break
        time.sleep(0.01)
    assert req_id is not None
    stdin.feed_obj({"jsonrpc": "2.0", "id": req_id, "error": {"code": METHOD_NOT_FOUND, "message": "not implemented"}})
    t.join(timeout=5)
    assert "err" in caught
    assert caught["err"].code == METHOD_NOT_FOUND


def test_outbound_call_times_out_rather_than_hanging():
    server, _stdin, _stdout, _ = make_server()
    started = time.time()
    with pytest.raises(RpcError) as exc:
        server.call("brain.chat", {}, timeout=0.3)
    assert exc.value.code == TIMEOUT
    assert time.time() - started < 3


def test_serve_forever_stops_on_stop():
    server, stdin, stdout, thread = make_server()
    server.handle("ping", lambda p: "pong")
    stdin.feed_obj({"jsonrpc": "2.0", "id": 9, "method": "ping", "params": {}})
    stdout.wait_for_responses(1)
    server.stop()
    stdin.close()
    thread.join(timeout=5)
    assert not thread.is_alive()