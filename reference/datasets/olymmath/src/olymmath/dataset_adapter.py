from uenv.sdk import DatasetAdapter, PreparedSample

from .models import OlymmathInput, OlymmathPrivateData


class OlymmathAdapter(DatasetAdapter):
    """Dataset-owned normalization; runtime resources are prepared by Environment."""

    def normalize(self, row: dict) -> PreparedSample:
        return PreparedSample(
            sample_id=str(row["unique_id"]),
            input=OlymmathInput(
                instruction=row["problem"], language=row["language"],
                difficulty=row["difficulty"], subject=row.get("subject", ""),
            ),
            private_data=OlymmathPrivateData(answer=row["answer"]),
        )
