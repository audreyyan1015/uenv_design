from uenv.sdk import Environment, Observation, structured_part


class SweVerifiedEnvironment(Environment):
    """Dataset-specific interface example; real workspace setup is not implemented."""

    def reset(self, task, context) -> Observation:
        context.check()
        input_data = task.input
        return Observation(content=[structured_part(input_data)])
