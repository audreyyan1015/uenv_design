"""Built-in generic agents; budgets and tool routing belong to Worker."""
from copy import deepcopy
from . import AgentRunner, AgentRuntimeError


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
            observation = context.observation
            if observation.terminated or observation.episode_truncated:
                return last_response
            try:
                generation = await context.generate(deepcopy(messages))
                last_response = deepcopy(generation['response'])
                reason = generation['finish_reason']
                if reason == 'stop':
                    return last_response
                if reason == 'length':
                    return last_response
                if reason != 'tool_calls' or not generation.get('tool_calls'):
                    raise ValueError('PLAIN_AGENT_REQUIRES_FINAL_ANSWER_OR_TOOL_CALLS')
                # ToolCall is normalized and checked by Worker before reaching Agent.
                calls = deepcopy(generation['tool_calls'])
                exchange = [{'role': 'assistant', 'content': last_response, 'tool_calls': calls}]
                for call in calls:
                    observation = await context.call_tool(deepcopy(call))
                    exchange.append({'role': 'tool', 'tool_call_id': call['tool_call_id'],
                                     'content': deepcopy(observation.content)})
                    observation = context.observation
                    if observation.terminated or observation.episode_truncated:
                        return last_response
                if self.config['history_policy'] == 'last_generation':
                    messages = deepcopy(base_messages)
                messages.extend(exchange)
            except AgentRuntimeError as error:
                if error.error['code'] in {
                    'GENERATION_LIMIT', 'OUTPUT_TOKEN_LIMIT', 'TOOL_CALL_LIMIT',
                    'FINALIZE_RESERVE_REACHED',
                }:
                    return last_response
                # Cancellation, model/IPC failures and authorization errors stay failures.
                raise
