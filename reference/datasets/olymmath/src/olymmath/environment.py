from uenv.sdk import Environment, Observation, text_part


class OlymmathEnvironment(Environment):
    """Dataset-specific initial observation; final answers are returned by AgentRunner."""

    def reset(self, task, context) -> Observation:
        context.check()
        input_data = task.input
        return Observation([text_part("Put the final answer in \\boxed{}.\n" + input_data.instruction)], input_data)
