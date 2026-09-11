"""Tool definitions and host-side format conversion, with no budget authority.

The Worker alone calls ToolHost.execute after admitting a call through step.
Agent integrations receive a step transport, never this host's executors.
"""
from __future__ import annotations

from copy import deepcopy
from contextvars import ContextVar
from dataclasses import dataclass, replace
import asyncio
import inspect
import json
import threading
from typing import Callable, get_type_hints

from jsonschema import Draft202012Validator

from . import Observation, UEnvModel, model_json_schema, structured_part, text_part, to_wire


_TOOL_OWNER = ContextVar('uenv_tool_owner', default=None)


def _track_tool_tasks(loop):
    """Track child tasks by their creator, not by unrelated loop activity."""
    existing = loop.get_task_factory()
    if getattr(existing, '_uenv_tracker', False):
        return

    def factory(loop, coro, **kwargs):
        context = kwargs.get('context')
        owner = context.get(_TOOL_OWNER) if context is not None else _TOOL_OWNER.get()
        task = existing(loop, coro, **kwargs) if existing else asyncio.Task(coro, loop=loop, **kwargs)
        if owner is not None:
            owner._background_tasks.add(task)
        return task

    factory._uenv_tracker = True
    loop.set_task_factory(factory)


@dataclass(frozen=True)
class ToolDefinition:
    description: str
    input_schema: dict
    execute: Callable
    entrypoint: str | None = None
    execution_scope: str = "sandbox"
    required_capabilities: tuple[str, ...] = ()
    side_effect: str = "non_idempotent"
    shutdown: Callable | None = None


def tool(function=None, *, execution_scope="sandbox", required_capabilities=(), side_effect="non_idempotent"):
    """Declare a Python tool; reserve keyword-only context for Host injection."""
    if function is None:
        return lambda fn: tool(fn, execution_scope=execution_scope,
                               required_capabilities=required_capabilities, side_effect=side_effect)
    if execution_scope not in ("sandbox", "agent_state", "external_service"):
        raise ValueError("INVALID_TOOL_EXECUTION_SCOPE")
    if side_effect not in ("read_only", "idempotent", "non_idempotent"):
        raise ValueError("INVALID_TOOL_SIDE_EFFECT")
    signature = inspect.signature(function)
    hints = get_type_hints(function, include_extras=True)
    annotations, defaults = {}, {}
    for name, parameter in signature.parameters.items():
        if name == "context":
            if parameter.kind != parameter.KEYWORD_ONLY:
                raise TypeError("Tool context must be keyword-only")
            continue
        if parameter.kind not in (parameter.POSITIONAL_OR_KEYWORD, parameter.KEYWORD_ONLY):
            raise TypeError("Tool parameters must have explicit names")
        if name not in hints:
            raise TypeError(f"Tool parameter needs a type: {name}")
        annotations[name] = hints[name]
        if parameter.default is not parameter.empty:
            defaults[name] = parameter.default
    model = type(function.__name__ + "Arguments", (UEnvModel,), {
        "__annotations__": annotations, **defaults,
    })
    schema = model_json_schema(model, f"urn:uenv:tool:{function.__module__}:{function.__name__}")
    Draft202012Validator.check_schema(schema)
    if "return" not in hints:
        raise TypeError("Tool return needs a type")
    output_validator = None
    if hints["return"] is not Observation:
        output_model = type(function.__name__ + "Return", (UEnvModel,), {"__annotations__":{"value":hints["return"]}})
        output_validator = Draft202012Validator(model_json_schema(output_model, schema["$id"] + ":return"))

    async def execute(arguments, context):
        values = model(**arguments)
        kwargs = {name: getattr(values, name) for name in annotations}
        if "context" in signature.parameters:
            kwargs["context"] = context
        result = function(**kwargs)
        result = await result if inspect.isawaitable(result) else result
        if output_validator is not None:
            output_validator.validate({"value":result.model_dump() if isinstance(result, UEnvModel) else result})
        elif not isinstance(result, Observation):
            raise TypeError("Tool declared Observation but returned another type")
        return result

    return ToolDefinition(inspect.getdoc(function) or function.__name__, schema, execute,
        f"{function.__module__}:{function.__name__}", execution_scope,
        tuple(required_capabilities), side_effect)


def observation_from_output(value):
    if isinstance(value, Observation):
        return value
    if isinstance(value, str):
        return Observation([text_part(value)])
    if isinstance(value, UEnvModel):
        return Observation([structured_part(value)])
    if value is None or isinstance(value, (bool, int, float, list, dict)):
        return Observation([text_part(json.dumps(value, ensure_ascii=False, allow_nan=False))])
    raise TypeError("Unsupported tool return type")


class ToolHost:
    """Attempt-local executable table assembled from the selected bindings.

    Native factories are supplied by the selected Agent integration. They read
    framework definitions instead of duplicating the parameter schema in UEnv.
    This host is not exposed to untrusted Agent code as a local capability.
    """

    def __init__(self, registry):
        self.registry = registry
        self._tools = {}
        self._frozen = False
        self._prepared = False
        self._lock = asyncio.Lock()
        self._background_tasks = set()
        self._background_threads = set()
        self._settled = False

    @property
    def frozen(self):
        return self._settled

    def validate_call(self, binding, call):
        registered = self._tools.get(call['name'])
        if registered is None or registered[0] != binding or call['implementation'] != binding['implementation']:
            raise ValueError('TOOL_CALL_BINDING_MISMATCH')
        if 'context' in call['arguments']:
            raise ValueError('TOOL_CONTEXT_IS_SYSTEM_INJECTED')
        Draft202012Validator(registered[1].input_schema).validate(call['arguments'])

    def prepare(self, bindings, definitions, contexts, agent):
        if self._prepared or self._frozen:
            raise RuntimeError("TOOL_HOST_ALREADY_PREPARED")
        staged = {}
        for binding in bindings:
            name = binding["name"]
            if name in staged:
                raise ValueError("DUPLICATE_TOOL_NAME")
            owner = binding.get("native_agent")
            if owner is not None:
                if owner != agent:
                    raise ValueError("NATIVE_TOOL_AGENT_MISMATCH")
                if any(binding["implementation"][k] != owner[k] for k in ("version", "digest")):
                    raise ValueError("NATIVE_TOOL_VERSION_MISMATCH")
            key = binding["implementation"]["id"]
            definition = replace(definitions[key], input_schema=deepcopy(definitions[key].input_schema))
            self.registry.validate("TypedConfig", binding["config"])
            Draft202012Validator.check_schema(definition.input_schema)
            context = contexts[name]
            # The Host constructs contexts using the one resolved config.
            if context.config != binding["config"]["data"]:
                raise ValueError("TOOL_CONFIG_MISMATCH")
            staged[name] = (deepcopy(binding), definition, context)
        self._tools = staged
        self._prepared = True
        return [deepcopy(value[0]) for value in staged.values()]

    async def execute(self, call):
        # One reference Host serializes operations, including tools sharing an
        # Environment. Production may parallelize only independent resources.
        async with self._lock:
            _track_tool_tasks(asyncio.get_running_loop())
            threads = set(threading.enumerate())
            token = _TOOL_OWNER.set(self)
            try:
                return await self._execute(call)
            finally:
                _TOOL_OWNER.reset(token)
                self._background_threads.update(set(threading.enumerate()) - threads)

    async def _execute(self, call):
        if self._frozen:
            raise RuntimeError("TOOL_HOST_FROZEN")
        binding, definition, context = self._tools[call["name"]]
        if call["implementation"] != binding["implementation"]:
            raise ValueError("TOOL_CALL_BINDING_MISMATCH")
        arguments = deepcopy(call["arguments"])
        json.dumps(arguments, allow_nan=False)
        if "context" in arguments:
            raise ValueError("TOOL_CONTEXT_IS_SYSTEM_INJECTED")
        Draft202012Validator(definition.input_schema).validate(arguments)
        value = definition.execute(arguments, context)
        if inspect.isawaitable(value):
            value = await value
        observation = to_wire(observation_from_output(value))
        self.registry.validate("Observation", observation)
        if (observation["terminated"] or observation["episode_truncated"]) and getattr(context, "environment", None) is None:
            raise ValueError("TOOL_HAS_NO_ENVIRONMENT_STATE")
        return {
            "tool_call_id": call["tool_call_id"], "status": "ok",
            "observation": observation, "output_truncated": False,
        }

    async def freeze(self):
        if self._settled:
            return
        self._frozen = True
        async with self._lock:
            for task in tuple(self._background_tasks):
                task.cancel()
            if self._background_tasks:
                await asyncio.gather(*self._background_tasks, return_exceptions=True)
            seen = set()
            for _, definition, context in self._tools.values():
                for shutdown in (definition.shutdown, getattr(context, 'freeze', None)):
                    if shutdown is not None and shutdown not in seen:
                        seen.add(shutdown)
                        result = shutdown()
                        if inspect.isawaitable(result):
                            await result
            if any(thread.is_alive() for thread in self._background_threads):
                # Unknown threads cannot safely be stopped inside Python.
                # Fail the freeze; Worker must close/kill the role process and
                # must not score a supposedly immutable view.
                raise RuntimeError('TOOL_BACKGROUND_WRITER_NOT_STOPPED')
            self._settled = True


class NativeToolAdapter:
    """Framework integrator supplies conversion once; authors do not implement it.

    decode converts JSON to the native Action; encode converts the original
    native return to Observation. The original result can remain in Agent Host
    memory, so the framework need not reconstruct private session objects from RPC.
    """

    def __init__(self, description, input_schema, decode, execute, encode):
        self._decode, self._execute, self._encode = decode, execute, encode
        self.definition = ToolDefinition(description, deepcopy(input_schema), self.execute)

    @classmethod
    def from_openhands(cls, native_tool, conversation, encode_observation):
        """Use the SDK's existing schema, Action validator and original executor.

        No OpenHands import is required until an integration supplies an actual
        ToolDefinition. encode_observation is the integration's common content
        conversion, not a function each dataset/tool author must implement.
        The native tool here must be the saved original, never its step proxy.
        """
        return cls(
            native_tool.description,
            native_tool.action_type.model_json_schema(),
            native_tool.action_type.model_validate,
            lambda action, context: native_tool(action, conversation),
            encode_observation,
        )

    async def execute(self, arguments, context):
        action = self._decode(arguments)
        result = self._execute(action, context)
        if inspect.isawaitable(result):
            result = await result
        return self._encode(result)


def verify_agent_tools(bindings, actual_tools):
    """Fail before generation if framework defaults add or replace any tool."""
    expected = {item["name"]: item for item in bindings}
    names = [item["name"] for item in actual_tools]
    if len(names) != len(set(names)) or expected != {item["name"]: item for item in actual_tools}:
        raise ValueError("AGENT_TOOL_TABLE_MISMATCH")
    # Names alone do not prove an execution hook was installed: integration tests
    # must also exercise the proxy and show that every invocation reaches step.


class AgentToolClient:
    """Only calls the Worker transport; never invokes a native executor directly."""

    def __init__(self, step):
        self._step = step

    async def call_tool(self, call):
        return await self._step(deepcopy(call))
