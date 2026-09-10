from typing import Annotated

from uenv.sdk import ArtifactRef, EvaluationPlan, Field, UEnvModel


class SweSmithInput(UEnvModel):
    """Model-visible SWE-smith task."""

    instruction: Annotated[str, Field("Repository repair request", min_length=1)]
    repo: Annotated[str, Field("Repository identity", min_length=1)]
    base_commit: Annotated[str, Field("Exact starting commit", min_length=1)]
    workspace_path: Annotated[str, Field("Sandbox workspace path", min_length=1)]
    runtime_assets: Annotated[list[ArtifactRef], Field("Versioned repository and dependency inputs")]


class SweSmithPrivateData(UEnvModel):
    """Scorer-only SWE-smith tests."""

    test_patch: Annotated[ArtifactRef, Field("Hidden test patch")]
    reference_patch: Annotated[ArtifactRef | None, Field("Optional reference patch")] = None
    fail_to_pass: Annotated[list[str], Field("Tests that must change from failing to passing")]
    pass_to_pass: Annotated[list[str], Field("Tests that must remain passing")]
    evaluation_plan: Annotated[EvaluationPlan, Field("Only harness execution configuration")]
