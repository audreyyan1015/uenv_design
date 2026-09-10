from uenv.sdk import Environment, Observation, text_part


class SweProEnvironment(Environment):
    """Dataset-specific interface example; real workspace setup is not implemented."""

    def reset(self, task, context) -> Observation:
        context.check()
        input_data = task.input
        return Observation([text_part(input_data.instruction)], input_data)
