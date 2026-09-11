"""Actual Python execution/conversion tests; Worker gating is tested in Rust."""
from copy import deepcopy
from types import SimpleNamespace
import unittest

from jsonschema import ValidationError

from uenv.sdk import Observation, text_part, tool
from uenv.sdk.schema_registry import SchemaRegistry
from uenv.sdk.tools import AgentToolClient, NativeToolAdapter, ToolHost, verify_agent_tools


AGENT = {"id":"agents/example", "version":"1", "digest":"sha256:" + "a"*64}


def binding(name, native=False):
    value = {
        "name":name,
        "implementation":{"id":f"agents/example/tools/{name}" if native else f"tools/{name}",
                          "version":"1", "digest":AGENT["digest"]},
        "config":{"schema_ref":"uenv://schemas/vnext/EmptyConfig", "data":{}},
        "execution_scope":"agent_state" if native else "sandbox", "required_capabilities":[],
    }
    if native: value["native_agent"] = deepcopy(AGENT)
    return value


class ToolsTests(unittest.IsolatedAsyncioTestCase):
    async def test_freeze_cancels_only_tool_owned_background_tasks(self):
        import asyncio
        writes = []
        @tool
        async def background() -> str:
            async def writer():
                await asyncio.sleep(0.05)
                writes.append('unexpected')
            asyncio.create_task(writer())
            return 'started'
        b = binding('background')
        host = ToolHost(SchemaRegistry.bundled())
        host.prepare([b], {b['implementation']['id']: background},
                     {'background': SimpleNamespace(config={})}, AGENT)
        unrelated = asyncio.create_task(asyncio.sleep(0.1, result='alive'))
        await host.execute({'name': 'background', 'implementation': b['implementation'],
                            'arguments': {}, 'tool_call_id': 'background-1'})
        await host.freeze()
        self.assertEqual(await unrelated, 'alive')
        self.assertEqual(writes, [])
        self.assertTrue(host.frozen)

    async def test_openhands_adapter_reads_original_schema_and_executor(self):
        calls = []
        class Action:
            @classmethod
            def model_json_schema(cls):
                return {"type":"object", "properties":{"command":{"type":"string"}},
                        "required":["command"], "additionalProperties":False}
            @classmethod
            def model_validate(cls, data):
                value = cls()
                value.command = data["command"]
                return value
        class Definition:
            description = "SDK terminal fixture"
            action_type = Action
            def __call__(self, action, conversation):
                calls.append((action.command, conversation))
                return SimpleNamespace(text="native result")
        session = object()
        adapter = NativeToolAdapter.from_openhands(Definition(), session,
            lambda value: Observation([text_part(value.text)]))
        result = await adapter.execute({"command":"pwd"}, None)
        self.assertEqual(calls, [("pwd", session)])
        self.assertEqual(result.content, [text_part("native result")])
        self.assertEqual(adapter.definition.input_schema, Action.model_json_schema())

    async def test_shared_state_is_serial_and_freeze_waits_for_active_call(self):
        import asyncio
        entered, release = asyncio.Event(), asyncio.Event()
        @tool
        async def slow(*, context) -> str:
            entered.set()
            await release.wait()
            context.writes.append("done")
            return "done"
        b = binding("slow")
        context = SimpleNamespace(config={}, writes=[])
        host = ToolHost(SchemaRegistry.bundled())
        host.prepare([b], {b["implementation"]["id"]:slow}, {"slow":context}, AGENT)
        call = {"name":"slow", "implementation":b["implementation"], "arguments":{}, "tool_call_id":"1"}
        running = asyncio.create_task(host.execute(call))
        await entered.wait()
        queued = asyncio.create_task(host.execute({**call,"tool_call_id":"2"}))
        freezing = asyncio.create_task(host.freeze())
        await asyncio.sleep(0)
        self.assertFalse(freezing.done())
        release.set()
        await running
        with self.assertRaisesRegex(RuntimeError, "FROZEN"): await queued
        await freezing
        self.assertEqual(context.writes, ["done"])

    async def test_different_native_format_uses_same_selected_table_and_result(self):
        executions = []

        @tool
        def add(value: int, *, context) -> str:
            """Add to the current value."""
            executions.append("python")
            return str(context.base + value)

        class NativeAction:
            def __init__(self, amount): self.amount = amount

        def original_executor(action, context):
            self.assertIsInstance(action, NativeAction)
            executions.append("native")
            return SimpleNamespace(answer=context.base + action.amount)

        native = NativeToolAdapter(
            "Native add", {"type":"object", "properties":{"amount":{"type":"integer"}},
                           "required":["amount"], "additionalProperties":False},
            lambda arguments: NativeAction(**arguments), original_executor,
            lambda output: Observation([text_part(str(output.answer))]),
        )
        bindings = [binding("add"), binding("native_add", True)]
        host = ToolHost(SchemaRegistry.bundled())
        host.prepare(bindings,
            {bindings[0]["implementation"]["id"]:add, bindings[1]["implementation"]["id"]:native.definition},
            {b["name"]:SimpleNamespace(config={}, base=2) for b in bindings}, AGENT)
        requests = []

        async def step_transport(call):
            requests.append(deepcopy(call))
            return await host.execute(call)  # Test transport, not a second budget authority.

        client = AgentToolClient(step_transport)
        for b, arguments in zip(bindings, [{"value":3}, {"amount":3}]):
            result = await client.call_tool({"tool_call_id":b["name"],
                "implementation":b["implementation"], "name":b["name"], "arguments":arguments})
            self.assertEqual(result["observation"]["content"], [text_part("5")])
            self.assertNotIn("data", result)
        self.assertEqual(executions, ["python", "native"])
        self.assertEqual(len(requests), 2)
        self.assertNotIn("context", add.input_schema["properties"])

    async def test_bad_arguments_and_context_do_not_execute(self):
        executions = []

        @tool
        def echo(value: int) -> str:
            executions.append(value)
            return str(value)

        b = binding("echo")
        host = ToolHost(SchemaRegistry.bundled())
        host.prepare([b], {b["implementation"]["id"]:echo},
                     {"echo":SimpleNamespace(config={})}, AGENT)
        for args in ({"value":"1"}, {"value":True}, {}, {"value":1,"extra":2},
                     {"value":1,"context":{}}):
            with self.subTest(args=args), self.assertRaises((ValidationError, ValueError)):
                await host.execute({"name":"echo", "implementation":b["implementation"],
                                    "arguments":args, "tool_call_id":"x"})
        self.assertEqual(executions, [])
        await host.freeze()
        with self.assertRaisesRegex(RuntimeError, "FROZEN"):
            await host.execute({"name":"echo"})

    async def test_denied_transport_never_calls_executor(self):
        async def denied(call): raise RuntimeError("TOOL_CALL_LIMIT")
        client = AgentToolClient(denied)
        with self.assertRaisesRegex(RuntimeError, "TOOL_CALL_LIMIT"):
            await client.call_tool({"name":"native"})

    def test_native_owner_version_and_hidden_defaults_are_rejected(self):
        b = binding("native", True)
        for owner, changed in (({}, b), (AGENT, {**b,"implementation":{**b["implementation"],"version":"2"}})):
            with self.assertRaisesRegex(ValueError, "NATIVE_TOOL"):
                ToolHost(SchemaRegistry.bundled()).prepare([changed], {}, {}, owner)
        verify_agent_tools([b], [deepcopy(b)])
        for actual in ([], [{"name":"native"},{"name":"hidden"}], [{"name":"native"}]*2):
            with self.assertRaisesRegex(ValueError, "TOOL_TABLE"):
                verify_agent_tools([b], actual)


if __name__ == "__main__": unittest.main()
