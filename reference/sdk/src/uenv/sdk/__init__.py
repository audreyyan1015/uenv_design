"""General reference SDK. In-process checks are not production security isolation."""
from __future__ import annotations
from abc import ABC, abstractmethod
from copy import deepcopy
from dataclasses import dataclass, field, fields
from typing import Callable
import time
import json
import hashlib
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
from .session import Session


def to_wire(value):
    if isinstance(value, UEnvModel):
        return model_to_envelope(value)
    if hasattr(value, '__dataclass_fields__'):
        return {f.name:to_wire(getattr(value,f.name)) for f in fields(value)
                if getattr(value,f.name) is not None or f.name in {'success','reward'}}
    if isinstance(value, dict):
        return {k:to_wire(v) for k,v in value.items()}
    if isinstance(value, (tuple,list)):
        return [to_wire(v) for v in value]
    return value


def text_part(text):
    return {'kind':'text','text':text}


def structured_part(value: UEnvModel) -> dict:
    """One public structured content item; authors do not write transport envelopes."""
    if not isinstance(value, UEnvModel):
        raise TypeError('Structured content requires a registered UEnvModel')
    return {'kind': 'structured', 'structured': model_to_envelope(value)}


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
    terminated: bool = False
    episode_truncated: bool = False


@dataclass(frozen=True)
class EnvironmentContext:
    registry: SchemaRegistry
    artifacts: object
    cancelled: Callable[[], bool]
    deadline: float
    seed: int
    session: Session

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

    def state_snapshot(self, context: EnvironmentContext) -> UEnvModel | None:
        return None

    def close(self, context: EnvironmentContext):
        """Optional plugin cleanup; Worker still owns underlying resources."""


class AgentRuntimeError(RuntimeError):
    """SDK representation of the existing Worker ErrorRecord, not a new wire type."""

    def __init__(self, error: dict):
        self.error = deepcopy(error)
        super().__init__(error['code'])

    @property
    def code(self):
        return self.error['code']


class AgentContext(ABC):
    """Worker-provided capability view; it never owns scheduling or budgets."""

    task: TaskSpec
    observation: Observation
    tools: tuple[dict, ...]
    seed: int
    # observation is the current reset/step result, including end flags.

    @abstractmethod
    async def generate(self, messages: list[dict]) -> dict:
        """Request one generation through Rust; denied operations raise AgentRuntimeError."""

    @abstractmethod
    async def call_tool(self, call: dict) -> Observation:
        """Return public feedback; transport envelopes stay inside the SDK."""



class AgentRunner(ABC):
    def __init__(self, config: dict):
        if not isinstance(config, dict):
            raise TypeError('AgentRunner config must be an object')
        self.config = deepcopy(config)

    @abstractmethod
    async def run(self, context: AgentContext) -> list[dict]:
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
    final_answer: list[dict]
    trajectory_ref: dict
    state: UEnvModel | None = None
    private_data: UEnvModel | None = None


@dataclass(frozen=True)
class ScoreResult:
    success: bool | None = None
    metrics: list[dict] = field(default_factory=list)
    reward: float | None = None
    evidence: list[dict] = field(default_factory=list)
    generation_rewards: list[dict] | None = None

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


def read_scoring_trajectory(request: ScoreInput, context: ScoringContext) -> list[dict]:
    """Read a verified, complete pre-score snapshot through the scoped reader."""
    if context.read_artifact is None:
        raise RuntimeError('Worker-bound trajectory reader is unavailable')
    def read(reference):
        context.check()
        content = context.read_artifact(reference)
        if (len(content) != reference['size_bytes']
                or 'sha256:' + hashlib.sha256(content).hexdigest() != reference['digest']):
            raise ValueError('Trajectory artifact integrity mismatch')
        return content
    manifest = json.loads(read(request.trajectory_ref))
    context.registry.validate('TrajectoryManifest', manifest)
    if manifest['trajectory_status'] != 'scoring_checkpoint' or manifest['task_id'] != request.task.task_id:
        raise ValueError('Expected this task scoring checkpoint')
    events = []
    for segment in manifest['event_segments']:
        for line in read(segment).splitlines():
            if not line:
                raise ValueError('Empty trajectory record')
            event = json.loads(line)
            context.registry.validate('TrajectoryEvent', event)
            if any(event[field] != manifest[field] for field in ('run_id', 'episode_id', 'attempt_id', 'task_id')):
                raise ValueError('Trajectory identity mismatch')
            if event['sequence'] != len(events):
                raise ValueError('Incomplete scoring trajectory')
            events.append(event)
    if len(events) != manifest['event_count']:
        raise ValueError('Trajectory event count mismatch')
    context.check()
    return events


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
    if any(part['kind'] != 'text' for part in request.final_answer):
        raise ValueError('Text-only scorer cannot discard non-text answer parts')
    return ''.join(part['text'] for part in request.final_answer), request.private_data.model_dump()


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
    command = {'final_answer':deepcopy(to_wire(request.final_answer)),
               'private_data':model_to_envelope(request.private_data), 'remaining_timeout_ms':remaining_timeout_ms}
    if request.state is not None:
        command['state'] = model_to_envelope(request.state)
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

# User-defined tools use the same authoring API in independent and dataset packages.
from .tools import tool
