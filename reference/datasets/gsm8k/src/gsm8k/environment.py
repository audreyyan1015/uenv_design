from uenv.sdk import Environment, Observation, text_part


class Gsm8kEnvironment(Environment):
    """Dataset-specific initial observation."""

    def reset(self, task, context) -> Observation:
        context.check()
        input_data = task.input
        return Observation([text_part(input_data.instruction)])
