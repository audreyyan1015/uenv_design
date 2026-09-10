from typing import Annotated

from uenv.sdk import ArtifactRef, EvaluationPlan, Field, UEnvModel


class DscodebenchInput(UEnvModel):
    """Model-visible DSCodeBench problem."""

    instruction: Annotated[str, Field("Programming problem", min_length=1)]
    library: Annotated[str, Field("Required library")]
    entry_point: Annotated[str, Field("Required callable entry point")]


class DscodebenchPrivateData(UEnvModel):
    """Scorer-only DSCodeBench tests and evaluation plan."""

    tests: Annotated[ArtifactRef, Field("Versioned test artifact")]
    reference_solution: Annotated[ArtifactRef | None, Field("Optional reference solution")] = None
    num_tests: Annotated[int, Field("Number of tests", minimum=1)]
    test_seed: Annotated[int, Field("Test sampling seed", minimum=0)]
    evaluation_plan: Annotated[EvaluationPlan, Field("Only harness execution configuration")]
