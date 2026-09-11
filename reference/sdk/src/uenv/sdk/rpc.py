"""Private, duplex JSON-lines RPC over inherited pipes.

Only the Worker creates this channel. It is not a public network endpoint.
The reader never executes extension code: nested Worker callbacks must remain
readable while an Agent is waiting for generate/step to complete.
"""
from concurrent.futures import Future, ThreadPoolExecutor
import json
import threading

MAX_FRAME_BYTES = 8 * 1024 * 1024


class RpcError(RuntimeError):
    def __init__(self, code):
        super().__init__(code)
        self.code = code


class RpcPeer:
    def __init__(self, reader, writer, handlers=None, *, start=True):
        self.reader, self.writer = reader, writer
        self.handlers = handlers if handlers is not None else {}
        self._lock = threading.Lock()
        self._pending = {}
        self._next = 0
        self._closed = False
        self._executor = ThreadPoolExecutor(max_workers=8, thread_name_prefix="uenv-rpc")
        self._reader = threading.Thread(target=self._read, daemon=True)
        if start:
            self.start()

    def start(self):
        self._reader.start()

    def _send(self, message):
        data = (json.dumps(message, ensure_ascii=False, allow_nan=False) + "\n").encode()
        if len(data) > MAX_FRAME_BYTES:
            raise RpcError("RPC_FRAME_LIMIT")
        with self._lock:
            if self._closed:
                raise RpcError("RPC_CLOSED")
            self.writer.write(data)
            self.writer.flush()

    def request(self, method, params, timeout=None):
        with self._lock:
            if self._closed:
                raise RpcError("RPC_CLOSED")
            self._next += 1
            request_id = f"python:{self._next}"
            future = self._pending[request_id] = Future()
        try:
            self._send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
            return future.result(timeout)
        finally:
            with self._lock:
                self._pending.pop(request_id, None)

    def _dispatch(self, message):
        try:
            handler = self.handlers.get(message["method"])
            if handler is None:
                raise RpcError("RPC_METHOD_NOT_ALLOWED")
            result = handler(message.get("params", {}))
            response = {"result": result}
        except Exception as error:
            # Do not leak credentials, arguments or private exception strings.
            response = {"error": {"code": -32000, "message": "Component call failed",
                                  "data": {"code": getattr(error, "code", "COMPONENT_CALL_FAILED")}}}
        try:
            self._send({"jsonrpc": "2.0", "id": message["id"], **response})
        except (OSError, RpcError):
            pass

    def _read(self):
        try:
            while True:
                line = self.reader.readline(MAX_FRAME_BYTES + 1)
                if not line:
                    break
                if len(line) > MAX_FRAME_BYTES or not line.endswith(b"\n"):
                    raise RpcError("RPC_FRAME_LIMIT")
                message = json.loads(line)
                if message.get("jsonrpc") != "2.0" or not isinstance(message.get("id"), str):
                    raise RpcError("RPC_INVALID_FRAME")
                if "method" in message:
                    self._executor.submit(self._dispatch, message)
                else:
                    with self._lock:
                        future = self._pending.get(message["id"])
                    if future is None:
                        raise RpcError("RPC_UNKNOWN_RESPONSE")
                    if "error" in message:
                        future.set_exception(RpcError(message["error"]["data"]["code"]))
                    else:
                        future.set_result(message["result"])
        except (OSError, ValueError, KeyError, RpcError):
            pass
        finally:
            self.close()

    def close(self):
        with self._lock:
            self._closed = True
            pending = list(self._pending.values())
        for future in pending:
            if not future.done():
                future.set_exception(RpcError("RPC_CLOSED"))
        self._executor.shutdown(wait=False, cancel_futures=True)

    def wait(self):
        self._reader.join()
