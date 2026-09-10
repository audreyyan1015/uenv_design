"""Executable extension examples; real Worker IPC is provided by the SDK adapter."""
from copy import deepcopy

from uenv.sdk import AgentRunner, AgentRuntimeError, Outcome, ToolExecutor


class PlainAgent(AgentRunner):
    def __init__(self, config):
        if (set(config) != {'history_policy', 'system_prompt'}
                or config['history_policy'] not in ('full', 'last_generation')
                or not isinstance(config['system_prompt'], str)):
            raise ValueError('UNSUPPORTED_PLAIN_AGENT_CONFIGURATION')
        super().__init__(config)

    async def run(self, context):
        base_messages = []
        if self.config['system_prompt']:
            base_messages.append({'role': 'system', 'content': [
                {'kind': 'text', 'text': self.config['system_prompt']}]})
        base_messages.append({'role': 'user', 'content': deepcopy(context.observation.content)})
        messages = deepcopy(base_messages)
        last_response = []
        # The Worker owns all counters. No second max_rounds/max_generations here.
        while True:
            try:
                generation = await context.generate(deepcopy(messages))
                last_response = deepcopy(generation['response'])
                reason = generation['finish_reason']
                if reason == 'stop':
                    return Outcome(final_answer=last_response)
                if reason == 'length':
                    return Outcome(final_answer=last_response, termination_reason='budget_exhausted')
                if reason != 'tool_calls' or not generation.get('tool_calls'):
                    raise ValueError('PLAIN_AGENT_REQUIRES_FINAL_ANSWER_OR_TOOL_CALLS')
                # ToolCall is normalized and checked by Worker before reaching Agent.
                calls = deepcopy(generation['tool_calls'])
                exchange = [{'role': 'assistant', 'content': last_response, 'tool_calls': calls}]
                for call in calls:
                    result = await context.call_tool(deepcopy(call))
                    content = deepcopy(result['content'])
                    if result.get('error'):
                        content.append({'kind': 'text', 'text': (
                            f"Tool {result['status']}: {result['error']['code']}")})
                    exchange.append({'role': 'tool', 'tool_call_id': result['tool_call_id'],
                                     'content': content})
                if self.config['history_policy'] == 'last_generation':
                    messages = deepcopy(base_messages)
                messages.extend(exchange)
            except AgentRuntimeError as error:
                if error.error['code'] in {
                    'GENERATION_LIMIT', 'OUTPUT_TOKEN_LIMIT', 'TOOL_CALL_LIMIT',
                    'ENVIRONMENT_STEP_LIMIT', 'FINALIZE_RESERVE_REACHED',
                }:
                    return Outcome(final_answer=last_response, termination_reason='budget_exhausted')
                # Cancellation, model/IPC failures and authorization errors stay failures.
                raise


class ReadFileTool(ToolExecutor):
    async def execute(self, arguments, context):
        return await context.session.read_file(arguments["path"])
