from copy import deepcopy
import json
from pathlib import Path
import unittest
from uenv.sdk import SchemaRegistry, UEnvModel, Observation, text_part
from uenv.sdk.agent_context import WorkerAgentContext


class AgentContextTests(unittest.IsolatedAsyncioTestCase):
    def test_native_close_does_not_require_interrupt_method(self):
        from types import SimpleNamespace
        from uenv.sdk.integrations.openhands import OpenHandsIntegration
        closed = []
        integration = object.__new__(OpenHandsIntegration)
        integration._closed_native = set()
        integration.native_definitions = {'finish': SimpleNamespace(executor=SimpleNamespace(close=lambda: closed.append(True)))}
        integration.close_native('finish')
        integration.close_native('finish')
        self.assertEqual(closed, [True])

    async def test_public_context_returns_observation_and_copies_task_and_seed(self):
        root = Path(__file__).resolve().parents[3]
        plan = json.loads((root/'reference/generated/episodes/gsm8k/execution_plan.json').read_text(encoding='utf-8'))
        class Input(UEnvModel):
            __schema_id__ = plan['task']['input']['schema_ref']
            instruction: str
        feedback = {'content': [text_part('tool feedback')], 'terminated': False, 'episode_truncated': False}
        class Peer:
            def request(self, method, value):
                self.method = method
                return {'tool_call_id': 'call-1', 'status': 'ok', 'observation': deepcopy(feedback), 'output_truncated': False}
        peer = Peer()
        context = WorkerAgentContext(peer, SchemaRegistry.bundled(), [])
        context.bind_task(plan['task'], {'content': [], 'terminated': False, 'episode_truncated': False}, 42, Input)
        self.assertEqual(context.seed, 42)
        context.task.input.instruction = 'changed'
        self.assertNotEqual(context.task.input.instruction, 'changed')
        observed = await context.call_tool({'tool_call_id': 'call-1'})
        self.assertIsInstance(observed, Observation)
        self.assertEqual(observed.content, feedback['content'])
        self.assertEqual(peer.method, 'step')
        observed.content.clear()
        self.assertEqual(context.observation.content, feedback['content'])
