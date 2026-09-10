from typing import Annotated, Literal

from uenv.sdk import Field, UEnvModel


class OlymmathInput(UEnvModel):
    """Model-visible OlymMATH problem."""

    instruction: Annotated[str, Field("Math problem", min_length=1)]
    language: Annotated[Literal["en", "zh"], Field("Problem language")]
    difficulty: Annotated[Literal["easy", "hard"], Field("Problem difficulty")]
    subject: Annotated[str, Field("Math subject")]


class OlymmathPrivateData(UEnvModel):
    """Scorer-only OlymMATH reference answer."""

    answer: Annotated[str, Field("Reference answer", min_length=1)]
