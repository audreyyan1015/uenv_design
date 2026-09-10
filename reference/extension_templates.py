"""Public extension shapes for design discussion; not transport implementations."""
from uenv.sdk import AgentRunner, Outcome, ToolExecutor


class PlainAgent(AgentRunner):
    def __init__(self, config):
        if set(config) != {'history_policy','system_prompt'} or config['history_policy'] not in ('full','last_generation'):
            raise ValueError('UNSUPPORTED_PLAIN_AGENT_CONFIGURATION')
        super().__init__(config)

    async def run(self, context):
        # Model provider returns a complete GenerationEvent. SDK does not invent tokens.
        messages = [{'role':'user','content':context.observation.content}]
        if self.config['system_prompt']:
            messages.insert(0,{'role':'system','content':[{'kind':'text','text':self.config['system_prompt']}]})
        # A single-generation agent has the same history for full and last_generation.
        generation = await context.generate(messages)
        if generation['finish_reason'] != 'stop':
            raise ValueError('Single-generation text agent requires a final model response')
        return Outcome(final_answer=generation['response'])


class ReadFileTool(ToolExecutor):
    async def execute(self, arguments, context):
        return await context.session.read_file(arguments["path"])


