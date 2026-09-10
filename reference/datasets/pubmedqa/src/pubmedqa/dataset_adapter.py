from uenv.sdk import DatasetAdapter, PreparedSample

from .models import PubmedqaInput, PubmedqaPrivateData


class PubmedqaAdapter(DatasetAdapter):
    """Dataset-owned normalization; runtime resources are prepared by Environment."""

    def normalize(self, row: dict) -> PreparedSample:
        return PreparedSample(
            sample_id=str(row["id"]),
            input=PubmedqaInput(instruction=row["QUESTION"], contexts=row["CONTEXTS"]),
            private_data=PubmedqaPrivateData(answer=row["final_decision"]),
        )
