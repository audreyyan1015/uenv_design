"""Public extension examples reuse the installed SDK implementation."""
import asyncio
from uenv.sdk import ToolExecutor
from uenv.sdk.agents import PlainAgent


class ReadFileTool(ToolExecutor):
    async def execute(self, arguments, context):
        return await asyncio.to_thread(context.session.read_file, arguments["path"])
