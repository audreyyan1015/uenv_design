"""General reference SDK. In-process checks are not production security isolation."""
from __future__ import annotations
from abc import ABC, abstractmethod
from copy import deepcopy
from dataclasses import dataclass, field, fields
from typing import Callable
import time
from .modeling import (
    ArtifactRef,
    ContentPart,
    EvaluationPlan,
    Field,
    UEnvModel,
    bind_schema,
    model_from_envelope,
    model_json_schema,
    model_to_envelope,
)
from .schema_registry import SchemaRegistry


def to_wire(value):
    if isinstance(value, UEnvModel):
        return model_to_envelope(value)
    if hasattr(value, '__dataclass_fields__'):
        return {f.name:to_wire(getattr(value,f.name)) for f in fields(value)
                if getattr(value,f.name) is not None or f.name in {'success','reward','environment_reward'}}
    if isinstance(value, dict):
        return {k:to_wire(v) for k,v in value.items()}
    if isinstance(value, (tuple,list)):
        return [to_wire(v) for v in value]
    return value


def text_part(text):
    return {'kind':'text','text':text}


@dataclass(frozen=True)
class PreparedSample:
    sample_id: str
    input: UEnvModel
    private_data: UEnvModel | None = None
    runtime: dict | None = None


@dataclass(frozen=True)
class TaskSpec:
    """Typed component view; transport-only digest and envelope stay hidden."""

    task_id: str
    dataset: dict
    sample_id: str
    input: UEnvModel
    runtime: dict | None = None


def task_from_wire(value: dict, input_model: type[UEnvModel]) -> TaskSpec:
    return TaskSpec(
        task_id=value['task_id'],
        dataset=deepcopy(value['dataset']),
        sample_id=value['sample_id'],
        input=model_from_envelope(input_model, value['input']),
        runtime=deepcopy(value.get('runtime')),
    )


class DatasetAdapter(ABC):
    @abstractmethod
    def normalize(self, row: dict) -> PreparedSample:
        """Deterministically convert one source row; never choose backend or agent."""


@dataclass(frozen=True)
class Observation:
    content: list[dict] = field(default_factory=list)
    data: UEnvModel | None = None


@dataclass(frozen=True)
class Transition:
    observation: Observation
    terminated: bool = False
    episode_truncated: bool = False
    environment_reward: float | None = None


@dataclass(frozen=True)
class Outcome:
    final_answer: list[dict] = field(default_factory=list)
    artifacts: list[dict] = field(default_factory=list)
    termination_reason: str = 'final_answer'
    state: UEnvModel | None = None

    @property
    def text(self):
        if any(part['kind'] != 'text' for part in self.final_answer):
            raise ValueError('Text-only scorer cannot discard non-text answer parts')
        return ''.join(part['text'] for part in self.final_answer)


@dataclass(frozen=True)
class EnvironmentContext:
    registry: SchemaRegistry
    artifacts: object
    cancelled: Callable[[], bool]
    deadline: float
    seed: int
    session: dict

    def check(self):
        if self.cancelled():
            raise RuntimeError('EPISODE_CANCELLED')
        if time.monotonic() >= self.deadline:
            raise TimeoutError('EPISODE_TIMEOUT')


class Environment(ABC):
    def __init__(self, config: dict):
        if not isinstance(config, dict):
            raise TypeError('Environment config must be an object')
        self.config = deepcopy(config)

    @abstractmethod
    def reset(self, task: TaskSpec, context: EnvironmentContext) -> Observation:
        """TaskSpec contains only public input and identity; no scoring material references."""

    def step(self, action: UEnvModel, context: EnvironmentContext) -> Transition:
        raise NotImplementedError('This environment declares no action interface')

    def state_snapshot(self, context: EnvironmentContext) -> UEnvModel | None:
        return None

    def snapshot(self, context: EnvironmentContext) -> Outcome:
        return Outcome(termination_reason='in_progress', state=deepcopy(self.state_snapshot(context)))

    def finalize(self, outcome: Outcome, context: EnvironmentContext) -> Outcome:
        if outcome.state is not None or outcome.termination_reason == 'in_progress':
            raise ValueError('Agent submission must finish and cannot provide environment state')
        return Outcome(deepcopy(outcome.final_answer), deepcopy(outcome.artifacts),
                             outcome.termination_reason, deepcopy(self.state_snapshot(context)))

    def close(self, context: EnvironmentContext):
        """Optional plugin cleanup; Worker still owns underlying resources."""


class AgentContext(ABC):
    """Worker-provided capability view; it never owns scheduling or budgets."""

    task: TaskSpec
    observation: Observation
    tools: tuple[dict, ...]
    seed: int

    @abstractmethod
    async def generate(self, messages: list[dict]) -> dict:
        """Request one model generation through the Rust AgentRuntime gate."""

    @abstractmethod
    async def call_tool(self, call: dict) -> dict:
        """Call one item from tools through the Rust AgentRuntime gate."""

    @abstractmethod
    async def step(self, action: UEnvModel) -> Transition:
        """Apply one environment action through the Rust AgentRuntime gate."""


class AgentRunner(ABC):
    def __init__(self, config: dict):
        if not isinstance(config, dict):
            raise TypeError('AgentRunner config must be an object')
        self.config = deepcopy(config)

    @abstractmethod
    async def run(self, context: AgentContext) -> Outcome:
        """Own the interaction loop; receive only visible capabilities."""


class ToolExecutor(ABC):
    def __init__(self, config: dict):
        if not isinstance(config, dict):
            raise TypeError('ToolExecutor config must be an object')
        self.config = deepcopy(config)

    @abstractmethod
    async def execute(self, arguments: dict, context) -> dict:
        """Execute one selected tool; input and output follow its published schemas."""


@dataclass(frozen=True)
class ScoreInput:
    task: TaskSpec
    outcome: Outcome
    trajectory_ref: dict
    private_data: UEnvModel | None = None


@dataclass(frozen=True)
class ScoreResult:
    success: bool | None = None
    metrics: list[dict] = field(default_factory=list)
    reward: float | None = None
    evidence: list[dict] = field(default_factory=list)

    # Only the Rust Worker fills these fields. Scorer-returned values are rejected.
    status: str | None = None
    scorer: dict | None = None
    error: dict | None = None

    @classmethod
    def binary(cls, passed):
        if type(passed) is not bool:
            raise TypeError('Binary score requires an actual bool')
        value = float(passed)
        return cls(passed, [{'name':'accuracy','value':value,'unit':'ratio','direction':'higher'}], value)


binary_score = ScoreResult.binary


@dataclass(frozen=True)
class ScoringContext:
    deadline: float
    registry: SchemaRegistry
    cancelled: Callable[[], bool]
    run_harness: Callable | None = None
    read_artifact: Callable | None = None

    def check(self):
        if self.cancelled():
            raise RuntimeError('EPISODE_CANCELLED')
        if time.monotonic() >= self.deadline:
            raise TimeoutError('SCORER_TIMEOUT')

    def remaining_timeout_ms(self) -> int:
        self.check()
        return max(1, int((self.deadline - time.monotonic()) * 1000))


class Scorer(ABC):
    def __init__(self, config: dict):
        if not isinstance(config, dict):
            raise TypeError('Scorer config must be an object')
        self.config = deepcopy(config)

    @abstractmethod
    def score(self, request: ScoreInput, context: ScoringContext) -> ScoreResult:
        """Evaluate frozen text/files/state/trace, with optional reference material."""


def read_reference_text(request: ScoreInput) -> tuple[str, dict]:
    """Opt-in extraction for rules requiring text and private reference material."""
    if request.private_data is None:
        raise ValueError('This scoring rule requires reference material')
    return request.outcome.text, request.private_data.model_dump()


def evaluate_harness(request: ScoreInput, context: ScoringContext) -> ScoreResult:
    """Run a Worker-bound test harness and validate its report; no model generation."""
    context.check()
    if context.run_harness is None:
        raise RuntimeError('Worker-bound official harness is unavailable')
    if request.private_data is None or 'evaluation_plan' not in request.private_data.model_dump():
        raise ValueError('Missing evaluation_plan')
    evaluation_plan = request.private_data['evaluation_plan']
    context.registry.validate('uenv://schemas/vnext/EvaluationPlan', evaluation_plan)
    remaining_timeout_ms = min(evaluation_plan['timeout_ms'], context.remaining_timeout_ms())
    if remaining_timeout_ms <= 0:
        raise TimeoutError('HARNESS_TIMEOUT')
    command = {'outcome':deepcopy(to_wire(request.outcome)),
               'private_data':model_to_envelope(request.private_data), 'remaining_timeout_ms':remaining_timeout_ms}
    context.registry.validate('HarnessRequest', command)
    report = context.run_harness(command)
    context.registry.validate('HarnessResult',report)
    if report['tests_passed'] > report['tests_run']:
        raise ValueError('Passed tests exceed executed tests')
    if report.get('status') != 'ok' or type(report.get('success')) is not bool:
        raise ValueError('Harness must complete and provide explicit success')
    reward = float(report['success'])
    return ScoreResult(report['success'], deepcopy(report.get('metrics',
        [{'name':'resolved','value':reward,'unit':'ratio','direction':'higher'}])),
        reward=reward, evidence=[deepcopy(report['report_ref'])])
