"""Agent-side public capabilities over the Worker transport."""
import asyncio
from copy import deepcopy
from . import AgentContext, AgentRuntimeError, Observation, task_from_wire
from .rpc import RpcError


def observation_from_result(result):
    # Recoverable tool failures already carry the same public feedback recorded
    # by Worker. Never append an unrecorded error message to the model history.
    if result['status'] == 'cancelled':
        raise AgentRuntimeError(result['error'])
    return Observation(**deepcopy(result['observation']))


class WorkerAgentContext(AgentContext):
    def __init__(self, peer, registry, tools):
        self._peer, self._registry = peer, registry
        self._tools = deepcopy(tools)
        self._task = None
        self._observation = Observation()
        self._seed = None

    def bind_task(self, task, observation, seed, input_model):
        if self._task is not None:
            raise RpcError('AGENT_ALREADY_RUNNING')
        self._registry.validate('TaskSpec', task)
        self._registry.validate('Observation', observation)
        self._task = task_from_wire(task, input_model)
        self._observation = Observation(**deepcopy(observation))
        self._seed = seed

    @property
    def task(self):
        return deepcopy(self._task)

    @property
    def observation(self):
        return deepcopy(self._observation)

    @property
    def tools(self):
        return tuple(deepcopy(self._tools))

    @property
    def seed(self):
        return self._seed

    async def _request(self, method, value):
        try:
            return await asyncio.to_thread(self._peer.request, method, deepcopy(value))
        except RpcError as error:
            raise AgentRuntimeError({'code': error.code}) from error

    async def generate(self, messages):
        return await self._request('generate', messages)

    async def call_tool(self, call):
        result = await self._request('step', call)
        self._registry.validate('ToolResult', result)
        self._observation = observation_from_result(result)
        return self.observation
