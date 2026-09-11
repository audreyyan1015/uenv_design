from uenv.sdk import Environment, Observation, text_part


class PubmedqaEnvironment(Environment):
    """Dataset-specific initial observation."""

    def reset(self, task, context) -> Observation:
        context.check()
        input_data = task.input
        prompt = "Read the abstract and answer yes, no, or maybe.\n" + "\n".join(input_data.contexts) + "\n" + input_data.instruction
        return Observation([text_part(prompt)])
