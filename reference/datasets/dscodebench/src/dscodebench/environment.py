from uenv.sdk import Environment, Observation, structured_part, text_part


class DscodebenchEnvironment(Environment):
    """Dataset-specific interface example; real workspace setup is not implemented."""

    def reset(self, task, context) -> Observation:
        context.check()
        input_data = task.input
        return Observation(content=[text_part("Generate a Python solution."), structured_part(input_data)])
