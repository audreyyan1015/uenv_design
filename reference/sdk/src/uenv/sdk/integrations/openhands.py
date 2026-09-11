"""OpenHands SDK 1.15 integration: real Conversation and original executors.

The only SDK-private hook is Agent._initialize; it is version-pinned and covered
by the real-session test. Defaults, extra LLMs and parallel execution are disabled.
The Worker retains the sole generation/tool budget and event writer.
"""
from copy import deepcopy
import asyncio
from importlib.metadata import version
import json
import threading

from .. import AgentRunner, AgentRuntimeError, Observation, text_part
from ..rpc import RpcError
from ..tools import ToolDefinition
from ..model_provider import text_content


class InteractionStopped(BaseException):
    """Internal loop exit; not a fabricated tool result or final answer."""


class OpenHandsIntegration:
    def __init__(self, context, bindings, native_factories, mcp_config, workspace, model_id, system_prompt=""):
        if version("openhands-sdk") != "1.15.0":
            raise RpcError("OPENHANDS_VERSION_NOT_VALIDATED")
        from openhands.sdk import Agent, Conversation, LLM
        from openhands.sdk.llm import LLMResponse, Message, MessageToolCall, TextContent
        from openhands.sdk.llm.utils.metrics import MetricsSnapshot
        from openhands.sdk.tool import ToolExecutor
        from openhands.sdk.context import AgentContext
        from litellm import ModelResponse
        from pydantic import PrivateAttr

        self.context, self.bindings = context, {b["name"]: b for b in bindings}
        self.native_factories = native_factories
        self.pending = {}
        self.all_calls = {}
        self.native_results = {}
        self.native_actions = {}
        self.native_definitions = {}
        self._closed_native = set()
        self.native_to_name = {}
        self.name_to_native = {}
        self.lock = threading.Lock()
        self.final_answer = []
        self.stopped = False
        self.failure = None
        self.native_executions = 0
        self.native_returns = 0
        owner = self

        class WorkerLLM(LLM):
            def completion(self, messages, tools=None, **kwargs):
                if owner.failure:
                    raise owner.failure
                if owner.stopped:
                    raise InteractionStopped()
                actual_names = {t.name for t in tools or []}
                if actual_names != set(owner.native_to_name):
                    raise RpcError("AGENT_TOOL_TABLE_MISMATCH")
                wire = []
                for message in messages:
                    parts = []
                    for part in message.content:
                        if not isinstance(part, TextContent):
                            raise RpcError("OPENHANDS_CONTENT_KIND_NOT_SUPPORTED")
                        parts.append(text_part(part.text))
                    value = {"role": message.role, "content": parts}
                    if message.tool_call_id:
                        value["tool_call_id"] = message.tool_call_id
                    if message.tool_calls:
                        value["tool_calls"] = [deepcopy(owner.all_calls[call.id]) for call in message.tool_calls]
                    wire.append(value)
                try:
                    generation = asyncio.run(owner.context.generate(wire))
                except (RpcError, AgentRuntimeError) as error:
                    owner.failure = error
                    raise
                owner.final_answer = generation["response"]
                calls = generation.get("tool_calls", [])
                with owner.lock:
                    owner.pending = {c["tool_call_id"]: deepcopy(c) for c in calls}
                    owner.all_calls.update(deepcopy(owner.pending))
                native_calls = [MessageToolCall(id=c["tool_call_id"],
                    name=owner.name_to_native[c["name"]], arguments=json.dumps(c["arguments"]),
                    origin="completion") for c in calls]
                if generation["finish_reason"] == "length":
                    raise InteractionStopped()
                message = Message(role="assistant", content=[TextContent(text=text_content(generation["response"]))],
                                  tool_calls=native_calls or None)
                raw = ModelResponse(id=generation["generation_id"], model=generation["model_id"],
                    choices=[{"finish_reason": generation["finish_reason"],
                              "message": {"role":"assistant", "content":text_content(generation["response"]),
                                  "tool_calls":[c.to_chat_dict() for c in native_calls] or None}}],
                    usage={"completion_tokens": generation["output_token_count"], "prompt_tokens": 0,
                           "total_tokens": generation["output_token_count"]})
                return LLMResponse(message=message, raw_response=raw,
                                   metrics=MetricsSnapshot(model_name=generation["model_id"]))

            def responses(self, *args, **kwargs):
                raise RpcError("OPENHANDS_RESPONSES_API_NOT_SUPPORTED")

        class WorkerExecutor(ToolExecutor):
            def __init__(self, name, original):
                self.name, self.original = name, original

            def __call__(self, action, conversation=None):
                call = owner.claim(self.name, action, native=True)
                key = call["tool_call_id"]
                owner.native_actions[key] = (action, conversation)
                try:
                    owner.step(call)
                    native_result = owner.native_results.pop(key)
                    owner.native_returns += 1
                    if self.original.name == "finish":
                        owner.final_answer = [text_part(action.message)]
                    return native_result  # Exact original object, not deserialized JSON.
                finally:
                    owner.native_actions.pop(key, None)
                    owner.native_results.pop(key, None)

            def close(self):
                owner.close_native(self.name)

            def interrupt(self):
                interrupt = getattr(self.original.executor, 'interrupt', None)
                if interrupt:
                    interrupt()

        class WorkerAgent(Agent):
            @property
            def prompt_dir(self):
                import inspect
                from pathlib import Path
                return str(Path(inspect.getfile(Agent)).parent / "prompts")

            def _initialize(self, state):
                if self._initialized:
                    return
                super()._initialize(state)
                selected = {}
                for name, binding in owner.bindings.items():
                    if "native_agent" not in binding:
                        continue
                    factory = native_factories[binding["implementation"]["id"]]
                    created = factory.create(state, **binding["config"]["data"])
                    if len(created) != 1:
                        raise RpcError("NATIVE_TOOL_EXPORT_MUST_BE_SINGLE")
                    original = created[0]
                    if original.name in selected:
                        raise RpcError("DUPLICATE_NATIVE_TOOL_NAME")
                    owner.native_definitions[name] = original
                    selected[original.name] = original.set_executor(WorkerExecutor(name, original))
                    owner.native_to_name[original.name] = name
                    owner.name_to_native[name] = original.name
                for native_name, definition in self._tools.items():
                    if native_name not in owner.bindings or "native_agent" in owner.bindings[native_name]:
                        raise RpcError("AGENT_TOOL_TABLE_MISMATCH")
                    if native_name in selected:
                        raise RpcError("DUPLICATE_TOOL_NAME")
                    selected[native_name] = definition
                    owner.native_to_name[native_name] = native_name
                    owner.name_to_native[native_name] = native_name
                if set(owner.name_to_native) != set(owner.bindings):
                    raise RpcError("AGENT_TOOL_TABLE_MISMATCH")
                self._tools = selected

        # Identity is derived from the sole plan.model; SDK prompt selection
        # must see the same model as the Worker transport.
        agent = WorkerAgent(llm=WorkerLLM(model=model_id, num_retries=0),
                            tools=[], include_default_tools=[], mcp_config=mcp_config,
                            condenser=None, critic=None, tool_concurrency_limit=1,
                            agent_context=AgentContext(system_message_suffix=system_prompt))
        self.conversation = Conversation(agent=agent, workspace=workspace, visualizer=None,
            persistence_dir=None, stuck_detection=False, max_iteration_per_run=2**31-1)
        self.conversation.agent._initialize(self.conversation.state)

    def claim(self, name, arguments, native=False):
        with self.lock:
            for key, call in self.pending.items():
                if call["name"] != name:
                    continue
                expected = call["arguments"]
                if native:
                    expected = self.native_definitions[name].action_from_arguments(expected)
                    matches = expected == arguments
                else:
                    matches = expected == arguments
                if matches:
                    return self.pending.pop(key)
        raise RpcError("TOOL_CALL_NOT_GENERATED")

    def step(self, call):
        try:
            observation = asyncio.run(self.context.call_tool(call))
            self.stopped |= observation.terminated or observation.episode_truncated
            return observation
        except (RpcError, AgentRuntimeError) as error:
            self.failure = error
            raise

    def call_mcp_tool(self, name, arguments):
        return self.step(self.claim(name, arguments))

    def tool_definitions(self, current_call):
        result = {}
        for name, original in self.native_definitions.items():
            def shutdown(name=name):
                self.close_native(name)
            def execute(arguments, context, original=original):
                key = current_call().get("tool_call_id")
                action, conversation = self.native_actions[key]
                native = original(action, conversation)
                self.native_results[key] = native
                self.native_executions += 1
                parts = []
                for part in native.to_llm_content:
                    if not hasattr(part, "text"):
                        raise RpcError("OPENHANDS_CONTENT_KIND_NOT_SUPPORTED")
                    parts.append(text_part(part.text))
                return Observation(parts)
            result[name] = ToolDefinition(original.description,
                original.action_type.model_json_schema(), execute, shutdown=shutdown)
        return result

    def close_native(self, name):
        if name in self._closed_native:
            return
        executor = self.native_definitions[name].executor
        if executor:
            interrupt = getattr(executor, 'interrupt', None)
            if interrupt:
                interrupt()
            executor.close()
        self._closed_native.add(name)

    def run(self, observation):
        if observation['terminated'] or observation['episode_truncated']:
            return []
        self.conversation.send_message(text_content(observation["content"]))
        try:
            self.conversation.run()
            if self.failure:
                raise self.failure
        except InteractionStopped:
            pass
        except Exception:
            # OpenHands wraps executor/LLM exceptions in ConversationRunError.
            # Use the original Worker failure, not the framework's error text.
            if self.failure is None:
                raise
            if self.failure.code not in {"GENERATION_LIMIT", "OUTPUT_TOKEN_LIMIT", "TOOL_CALL_LIMIT", "FINALIZE_RESERVE_REACHED"}:
                raise self.failure
        return self.final_answer

    def close(self):
        self.conversation.close()


class OpenHandsAdapter(AgentRunner):
    """The same public AgentRunner interface as a user-defined agent."""
    def prepare(self, context, bindings, native_factories, mcp_config, workspace, model_id):
        if set(self.config) != {'system_prompt'} or not isinstance(self.config['system_prompt'], str):
            raise RpcError('OPENHANDS_CONFIG_NOT_SUPPORTED')
        self.integration = OpenHandsIntegration(context, bindings, native_factories,
            mcp_config, workspace, model_id, self.config['system_prompt'])

    async def run(self, context):
        from .. import to_wire
        return await asyncio.to_thread(self.integration.run, to_wire(context.observation))
