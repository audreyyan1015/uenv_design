"""Attempt-local MCP service over authenticated loopback HTTP.

It exposes the selected Python definitions and delegates every call to Worker
step. It never executes tools itself. Both SDK and native MCP clients use the
same bindings; URLs/tokens are ephemeral Host details, not RunSpec fields.
"""
import asyncio
import logging
import json
from contextlib import asynccontextmanager
import secrets
import socket
import threading
import time
from .. import to_wire


class McpToolServer:
    def __init__(self, definitions, call_tool):
        import mcp.types as types
        from mcp.server.lowlevel import Server
        from mcp.server.streamable_http_manager import StreamableHTTPSessionManager
        from starlette.applications import Starlette
        from starlette.routing import Mount
        import uvicorn

        self.token = secrets.token_urlsafe(32)
        token = self.token
        class RedactToken(logging.Filter):
            def filter(self, record):
                record.msg = record.getMessage().replace(token, "[redacted]")
                record.args = ()
                return True
        self._redactor = RedactToken()
        self._log_handlers = list(logging.getLogger().handlers)
        for handler in self._log_handlers:
            handler.addFilter(self._redactor)
        self._calls = 0
        server = Server("uenv")

        @server.list_tools()
        async def list_tools():
            return [types.Tool(name=name, description=value.description,
                               inputSchema=value.input_schema)
                    for name, value in definitions.items()]

        @server.call_tool()
        async def invoke(name, arguments):
            if name not in definitions:
                raise ValueError("TOOL_NOT_SELECTED")
            self._calls += 1
            observation = to_wire(await asyncio.to_thread(call_tool, name, arguments))
            # ToolResult remains the internal canonical result. MCP content is
            # solely its public model-facing view, not a second execution result.
            content = []
            for part in observation["content"]:
                if part["kind"] == "text":
                    content.append(types.TextContent(type="text", text=part["text"]))
                elif part["kind"] == "structured":
                    content.append(types.TextContent(type="text", text=json.dumps(part["structured"]["data"], ensure_ascii=False)))
                elif part["kind"] == "artifact":
                    artifact = part["artifact"]
                    content.append(types.ResourceLink(type="resource_link", uri=artifact["uri"],
                        name=artifact["uri"].rsplit("/", 1)[-1], mimeType=artifact["media_type"]))
                else:
                    raise ValueError("MCP_CONTENT_KIND_NOT_SUPPORTED")
            return types.CallToolResult(content=content, structuredContent=observation)

        manager = StreamableHTTPSessionManager(app=server, stateless=True, json_response=True)

        @asynccontextmanager
        async def lifespan(app):
            async with manager.run():
                yield

        app = Starlette(routes=[Mount("/mcp", app=manager.handle_request)], lifespan=lifespan)
        token = self.token

        async def authenticated(scope, receive, send):
            if scope["type"] == "http":
                headers = dict(scope["headers"])
                auth = headers.get(b"authorization", b"").decode()
                if not secrets.compare_digest(auth, "Bearer " + token):
                    await send({"type": "http.response.start", "status": 401, "headers": []})
                    await send({"type": "http.response.body", "body": b"Unauthorized"})
                    return
            await app(scope, receive, send)

        self.socket = socket.socket()
        self.socket.bind(("127.0.0.1", 0))
        self.socket.listen(16)
        self.url = f"http://127.0.0.1:{self.socket.getsockname()[1]}/mcp/"
        self.server = uvicorn.Server(uvicorn.Config(authenticated, log_level="error", access_log=False))
        self.thread = threading.Thread(target=self.server.run, kwargs={"sockets": [self.socket]}, daemon=True)
        self.thread.start()
        deadline = time.monotonic() + 10
        while not self.server.started:
            if not self.thread.is_alive() or time.monotonic() >= deadline:
                self.close()
                raise RuntimeError("MCP_START_FAILED")
            time.sleep(0.01)

    @property
    def client_config(self):
        return {"mcpServers": {"uenv": {"url": self.url,
                "headers": {"Authorization": "Bearer " + self.token}}}}

    def close(self):
        self.server.should_exit = True
        self.thread.join(timeout=5)
        self.socket.close()
        if self.thread.is_alive():
            raise RuntimeError("MCP_CLOSE_TIMEOUT")
        for handler in self._log_handlers:
            handler.removeFilter(self._redactor)
