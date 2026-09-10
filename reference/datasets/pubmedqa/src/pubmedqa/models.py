from typing import Annotated

from uenv.sdk import Field, UEnvModel


class PubmedqaInput(UEnvModel):
    """Model-visible PubMedQA task data."""

    instruction: Annotated[str, Field("Question", min_length=1)]
    contexts: Annotated[list[str], Field("Abstract passages in source order")]


class PubmedqaPrivateData(UEnvModel):
    """Scorer-only PubMedQA label."""

    answer: Annotated[str, Field("Reference label", min_length=1)]
