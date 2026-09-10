from typing import Annotated

from uenv.sdk import Field, UEnvModel


class Gsm8kInput(UEnvModel):
    """Model-visible GSM8K task data."""

    instruction: Annotated[str, Field("Task question", min_length=1)]


class Gsm8kPrivateData(UEnvModel):
    """Scorer-only GSM8K reference data."""

    answer: Annotated[str, Field("Reference answer", min_length=1)]
