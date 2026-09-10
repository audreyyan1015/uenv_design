"""Stateful interaction example using the same typed-model author interface."""
from typing import Annotated

from uenv.sdk import (
    AgentRunner,
    Environment,
    Field,
    Observation,
    Outcome,
    Scorer,
    ScoreResult,
    Transition,
    UEnvModel,
    model_json_schema,
    text_part,
)

INPUT = "urn:example:counter:input:v1"
ACTION = "urn:example:counter:action:v1"
STATE = "urn:example:counter:state:v1"


class CounterInput(UEnvModel):
    goal: Annotated[int, Field("Target counter value", minimum=1)]
    note: Annotated[str | None, Field("Optional task note")] = None


class CounterAction(UEnvModel):
    increment: Annotated[int, Field("Increment for one step", minimum=1)]


class CounterState(UEnvModel):
    value: Annotated[int, Field("Current counter value", minimum=0)]
    goal: Annotated[int, Field("Target counter value", minimum=1)]


def register_schemas(registry):
    for model, identifier in ((CounterInput, INPUT), (CounterAction, ACTION), (CounterState, STATE)):
        registry.register(model_json_schema(model, identifier))


class CounterEnvironment(Environment):
    def __init__(self, config):
        super().__init__(config)
        self.closed = False
        self.value = 0

    def observe(self):
        state = CounterState(value=self.value, goal=self.goal)
        return Observation(
            [text_part("Increase the counter to its goal."), {"kind": "artifact", "artifact": self.image}],
            state,
        )

    def reset(self, task, context):
        if not isinstance(task.input, CounterInput):
            raise TypeError("CounterEnvironment requires CounterInput")
        self.goal = task.input.goal
        self.value = 0
        self.image = context.artifacts.put(
            b'<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="1" height="1"/></svg>',
            "image/svg+xml",
        )
        return self.observe()

    def step(self, action, context):
        context.check()
        if not isinstance(action, CounterAction) or action.increment != 1 or self.value >= self.goal:
            raise ValueError("Invalid counter action or task already finished")
        self.value += action.increment
        return Transition(self.observe(), terminated=self.value >= self.goal, environment_reward=1 / self.goal)

    def state_snapshot(self, context):
        return CounterState(value=self.value, goal=self.goal)

    def close(self, context):
        self.closed = True
        self.value = -1


class ProgressScorer(Scorer):
    def score(self, request, context):
        state = request.outcome.state
        if not isinstance(state, CounterState):
            raise TypeError("ProgressScorer requires CounterState")
        progress = state.value / state.goal
        return ScoreResult(
            success=state.value >= state.goal,
            metrics=[
                {"name": "progress", "value": progress, "unit": "ratio", "direction": "higher"},
                {"name": "actions", "value": state.value, "unit": "count", "direction": "lower"},
            ],
            reward=progress,
        )


class CounterAgent(AgentRunner):
    """Scripted test Agent; no model endpoint is called."""

    async def run(self, context):
        while True:
            transition = await context.step(CounterAction(increment=1))
            if transition.terminated:
                return Outcome(termination_reason="environment_terminal")
