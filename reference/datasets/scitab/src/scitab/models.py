from typing import Annotated

from uenv.sdk import Field, UEnvModel


class ScitabInput(UEnvModel):
    """Model-visible SciTab claim and table."""

    claim: Annotated[str, Field("Claim to verify", min_length=1)]
    paper: Annotated[str, Field("Paper title")]
    paper_id: Annotated[str, Field("Source paper identifier")]
    table_id: Annotated[str, Field("Source table identifier")]
    caption: Annotated[str, Field("Table caption")]
    columns: Annotated[list[str], Field("Table column names")]
    rows: Annotated[list[list[str]], Field("Table rows")]


class ScitabPrivateData(UEnvModel):
    """Scorer-only SciTab label."""

    answer: Annotated[str, Field("Reference label", min_length=1)]
