"""Managed Python process entrypoint and concrete Agent/Tool/Model RPC handlers.

The launcher supplies a trusted factory from a verified installed package. RPC
requests cannot import arbitrary modules or replace that factory. Private scorer
data must not be supplied to this Agent process.
"""
import asyncio
from copy import deepcopy
from contextvars import ContextVar
import importlib
import sys
import threading
from types import SimpleNamespace

from jsonschema import Draft202012Validator, ValidationError
from . import AgentRunner, UEnvModel
from .agent_context import WorkerAgentContext
from .rpc import RpcError, RpcPeer
from .tools import ToolHost


class ComponentHost:
    def __init__(self, registry, python_definitions, native_factories=None, workspace=".",
                 contexts=None, agent_types=None, input_models=None):
        self.registry = registry
        if agent_types is None:
            from .agents import PlainAgent
            from .integrations.openhands import OpenHandsAdapter
            agent_types = {'agents/plain': PlainAgent, 'agents/openhands': OpenHandsAdapter}
        self.agent_types = agent_types
        self.input_models = input_models or {}
        self.python_definitions = python_definitions
        self.native_factories = native_factories or {}
        self.workspace = workspace
        self.contexts = contexts
        self.bindings = None
        self.local_bindings = None
        self.agent_spec = None
        self._closed = False
        self.host = ToolHost(registry)
        self.integration = None
        self.mcp = None
        self.definitions = {}
        self.active_call = ContextVar("uenv_tool_call", default={})
        self.loop = asyncio.new_event_loop()
        self.loop_thread = threading.Thread(target=self.loop.run_forever, daemon=True)
        self.loop_thread.start()

    def bind(self, peer):
        self.peer = peer
        return {"tools.prepare": self.prepare_tools, "agent.prepare": self.prepare_agent,
                "agent.run": self.run_agent, "tools.validate": self.validate_tool,
                "tools.execute": self.execute_tool, "tools.freeze": self.freeze_tools,
                "host.close": self.close}

    def prepare_tools(self, params):
        if self.local_bindings is not None:
            raise RpcError("TOOL_HOST_ALREADY_PREPARED")
        if any(b['execution_scope'] != 'agent_state' for b in params['tools']):
            raise RpcError('TOOL_EXECUTION_SCOPE_MISMATCH')
        self.local_bindings = deepcopy(params['tools'])
        return self.local_bindings

    def prepare_agent(self, params):
        local = [b for b in params['tools'] if b['execution_scope'] == 'agent_state']
        if self.agent_spec is not None or local != self.local_bindings:
            raise RpcError("AGENT_TOOL_TABLE_MISMATCH")
        self.bindings = deepcopy(params['tools'])
        self.agent_spec = deepcopy(params["agent"])
        implementation = self.agent_spec["implementation"]
        self.registry.validate('TypedConfig', self.agent_spec['config'])
        agent_type = self.agent_types.get(implementation['id'])
        if not isinstance(agent_type, type) or not issubclass(agent_type, AgentRunner):
            raise RpcError('AGENT_ENTRYPOINT_UNAVAILABLE')
        self.agent = agent_type(self.agent_spec['config']['data'])
        self.context = WorkerAgentContext(self.peer, self.registry, self.bindings)
        selected_python = {b["name"]: self.python_definitions[b["implementation"]["id"]]
                           for b in self.bindings if "native_agent" not in b}
        # Only integrations with native framework state need this preparation
        # hook. Every agent, including OpenHands, still runs via AgentRunner.
        prepare = getattr(self.agent, 'prepare', None)
        if prepare is not None:
            from .interfaces.mcp_server import McpToolServer
            if selected_python:
                self.mcp = McpToolServer(selected_python,
                    lambda name, args: self.integration.call_mcp_tool(name, args))
            prepare(self.context, self.bindings, self.native_factories,
                    self.mcp.client_config if self.mcp else {}, self.workspace, params['model_id'])
            self.integration = self.agent.integration
            self.definitions = {**selected_python,
                **self.integration.tool_definitions(self.active_call.get)}
        else:
            if any('native_agent' in b for b in self.bindings):
                raise RpcError('NATIVE_TOOL_AGENT_MISMATCH')
            self.definitions = selected_python
        contexts = self.contexts or {b["name"]: SimpleNamespace(config=b["config"]["data"],
                    environment=None) for b in self.bindings}
        definitions_by_id = {b["implementation"]["id"]: self.definitions[b["name"]] for b in self.bindings}
        self.host.prepare(self.local_bindings, definitions_by_id, contexts, implementation)
        return self.bindings

    def validate_tool(self, params):
        call, binding = params["call"], params["binding"]
        if binding not in self.local_bindings:
            raise RpcError('TOOL_EXECUTION_SCOPE_MISMATCH')
        if binding not in self.bindings or call["implementation"] != binding["implementation"] or call["name"] != binding["name"]:
            raise RpcError("TOOL_CALL_BINDING_MISMATCH")
        try:
            Draft202012Validator(self.definitions[call["name"]].input_schema).validate(call["arguments"])
        except ValidationError:
            raise RpcError("INVALID_TOOL_ARGUMENTS") from None
        if "context" in call["arguments"]:
            raise RpcError("TOOL_CONTEXT_IS_SYSTEM_INJECTED")
        return None

    def execute_tool(self, params):
        self.validate_tool(params)
        async def execute():
            token = self.active_call.set(params["call"])
            try:
                return await self.host.execute(params["call"])
            finally:
                self.active_call.reset(token)
        return asyncio.run_coroutine_threadsafe(execute(), self.loop).result()

    def freeze_tools(self, params):
        asyncio.run_coroutine_threadsafe(self.host.freeze(), self.loop).result()
        return None

    def run_agent(self, params):
        schema_id = params['task']['input']['schema_ref']
        model = self.input_models.get(schema_id)
        if model is None:
            # Agent needs a typed public view but need not import environment
            # executable code. Reconstruct it from the validated package schema.
            schema = self.registry.extensions[schema_id]
            from typing import Any
            model = type('PublicInput', (UEnvModel,), {
                '__annotations__': {name: Any for name in schema['properties']},
                '__schema_id__': schema_id,
                **{name: None for name in schema['properties'] if name not in schema.get('required', [])}})
        self.context.bind_task(params['task'], params['observation'], params['seed'], model)
        return asyncio.run(self.agent.run(self.context))

    def close(self, params=None):
        if self._closed:
            return None
        self._closed = True
        try:
            try:
                if self.integration:
                    self.integration.close()
            finally:
                if self.mcp:
                    self.mcp.close()
        finally:
            self.loop.call_soon_threadsafe(self.loop.stop)
            self.loop_thread.join(timeout=5)
        return None


def main():
    # Keep the protocol stdout separate from framework/extension prints.
    protocol_out = sys.stdout.buffer
    sys.stdout = sys.stderr
    module_name, function = sys.argv[1].split(":", 1)
    factory = getattr(importlib.import_module(module_name), function)
    component = factory()
    handlers = {}
    peer = RpcPeer(sys.stdin.buffer, protocol_out, handlers, start=False)
    handlers.update(component.bind(peer))
    peer.start()
    try:
        peer.wait()
    finally:
        component.close()


if __name__ == "__main__":
    main()
