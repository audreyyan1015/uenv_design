"""Dataset-owned counter operation, using the same API as independent tools."""
from uenv.sdk import Observation, tool


@tool
def increment(amount: int, *, context) -> Observation:
    """Increase the current counter by one."""
    environment = context.environment
    if amount != 1 or environment.value >= environment.goal:
        raise ValueError("INVALID_COUNTER_INCREMENT")
    environment.value += amount
    return environment.observe()
