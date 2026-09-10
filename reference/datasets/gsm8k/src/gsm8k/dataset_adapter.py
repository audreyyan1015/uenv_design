from uenv.sdk import DatasetAdapter, PreparedSample

from .models import Gsm8kInput, Gsm8kPrivateData


class Gsm8kAdapter(DatasetAdapter):
    """Dataset-owned normalization; runtime resources are prepared by Environment."""

    def normalize(self, row: dict) -> PreparedSample:
        return PreparedSample(
            sample_id=str(row["id"]),
            input=Gsm8kInput(instruction=row["question"]),
            private_data=Gsm8kPrivateData(answer=row["answer"].rsplit("####", 1)[-1].strip()),
        )
