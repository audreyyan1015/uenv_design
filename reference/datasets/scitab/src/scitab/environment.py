import json
from uenv.sdk import Environment, Observation, text_part


class ScitabEnvironment(Environment):
    """Dataset-specific initial observation."""

    def reset(self, task, context) -> Observation:
        context.check()
        input_data = task.input
        prompt = "Classify as supports, refutes, or not enough info.\n" + json.dumps(input_data.model_dump(), ensure_ascii=False)
        return Observation([text_part(prompt)])
