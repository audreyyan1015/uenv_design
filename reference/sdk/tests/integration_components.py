"""Deterministic HTTP model fixture; OpenHands, MCP and pipes are real.

Only the model's responses are scripted. This fixture performs no network access
except ephemeral loopback HTTP. It is not a replacement Agent or tool transport.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import tempfile
import threading
from urllib.request import Request, urlopen
from urllib.error import HTTPError

from uenv.sdk import SchemaRegistry, tool, UEnvModel, bind_schema, model_json_schema
from uenv.sdk.component_host import ComponentHost


def create_host():
    from openhands.sdk.tool.builtins import FinishTool
    root = Path(__file__).resolve().parents[3]
    registry = SchemaRegistry.bundled()
    for path in (root / "contracts/extensions").glob("*.json"):
        registry.register(json.loads(path.read_text()))
    executions = []
    requests = []

    class Sum(UEnvModel):
        total: int
    bind_schema(Sum, "uenv://integration/Sum")
    registry.register(model_json_schema(Sum, "uenv://integration/Sum"))

    @tool(execution_scope='agent_state')
    def add(left: int, right: int) -> Sum:
        """Add two integers."""
        executions.append((left, right))
        return Sum(total=left + right)

    class ModelHandler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass
        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            requests.append(body)
            index = len(requests)
            if index == 1:
                message = {"role": "assistant", "content": "Calculate with the selected tool.",
                           "tool_calls": [{"id": "call-add", "type": "function",
                             "function": {"name": "add", "arguments": '{"left":2,"right":3}'}}]}
                reason = "tool_calls"
            elif any(t["function"]["name"] == "finish" for t in body.get("tools", [])):
                message = {"role": "assistant", "content": "The calculation returned five.",
                           "tool_calls": [{"id": "call-finish", "type": "function",
                             "function": {"name": "finish", "arguments": '{"message":"5"}'}}]}
                reason = "tool_calls"
            else:
                message = {"role": "assistant", "content": "5"}
                reason = "stop"
            data = json.dumps({"id": f"model-{index}", "object": "chat.completion",
                "model": body["model"], "choices": [{"index": 0, "message": message, "finish_reason": reason}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 3, "total_tokens": 13}}).encode()
            self.send_response(200); self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data))); self.end_headers(); self.wfile.write(data)

    server = ThreadingHTTPServer(("127.0.0.1", 0), ModelHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    workspace = tempfile.TemporaryDirectory(prefix="uenv-agent-session-", dir=root/"target")

    class FixtureHost(ComponentHost):
        def prepare_agent(self, params):
            try:
                return super().prepare_agent(params)
            except Exception:
                import traceback, sys
                diagnostic = traceback.format_exc()
                if self.mcp:
                    diagnostic = diagnostic.replace(self.mcp.token, "[redacted]")
                print(diagnostic, file=sys.stderr)
                raise
        def bind(self, peer):
            handlers = {**super().bind(peer),
                    "fixture.info": lambda _: {"endpoint": f"http://127.0.0.1:{server.server_port}/v1"},
                    "fixture.stats": self.stats}
            def checked(handler):
                def run(params):
                    try:
                        return handler(params)
                    except Exception:
                        import traceback, sys
                        diagnostic = traceback.format_exc()
                        if self.mcp:
                            diagnostic = diagnostic.replace(self.mcp.token, "[redacted]")
                        print(diagnostic, file=sys.stderr)
                        raise
                return run
            return {name: checked(handler) for name, handler in handlers.items()}
        def stats(self, _):
            unauthorized = None
            if self.mcp:
                try:
                    urlopen(Request(self.mcp.url, data=b"{}", headers={"Content-Type": "application/json"}), timeout=3)
                except HTTPError as error:
                    unauthorized = error.code
            return {"executions": executions, "model_requests": requests,
                    "mcp_calls": self.mcp._calls if self.mcp else 0,
                    "unauthorized_status": unauthorized,
                    "native_executions": self.integration.native_executions if self.integration else 0,
                    "native_returns": self.integration.native_returns if self.integration else 0,
                    "native_result_cache": len(self.integration.native_results) if self.integration else 0}
        def close(self, params=None):
            if getattr(self, "fixture_closed", False):
                return None
            self.fixture_closed = True
            try:
                return super().close(params)
            finally:
                server.shutdown(); server.server_close(); thread.join(timeout=3); workspace.cleanup()

    return FixtureHost(registry, {"tools/add": add},
        {"agents/openhands/tools/finish": FinishTool}, workspace=workspace.name)


def create_model_host():
    from types import SimpleNamespace
    from uenv.sdk.model_host import ModelHost
    from openhands.sdk.tool.builtins import FinishTool
    native = FinishTool.create(None)[0]
    definitions = {
        'add': SimpleNamespace(description='Add two integers.', input_schema={
            'type':'object','properties':{'left':{'type':'integer'},'right':{'type':'integer'}},
            'required':['left','right'],'additionalProperties':False}),
        'finish': SimpleNamespace(description=native.description,
                                  input_schema=native.action_type.model_json_schema()),
    }
    return ModelHost(SchemaRegistry.bundled(), definitions)
