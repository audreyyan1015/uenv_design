from uenv.sdk import DatasetAdapter, PreparedSample

from .models import ScitabInput, ScitabPrivateData


class ScitabAdapter(DatasetAdapter):
    """Dataset-owned normalization; runtime resources are prepared by Environment."""

    def normalize(self, row: dict) -> PreparedSample:
        return PreparedSample(
            sample_id=str(row["id"]),
            input=ScitabInput(
                claim=row["claim"], paper=row.get("paper", ""), paper_id=row.get("paper_id", ""),
                table_id=row.get("table_id", ""), caption=row.get("table_caption", ""),
                columns=row["table_column_names"], rows=row["table_content_values"],
            ),
            private_data=ScitabPrivateData(answer=row["label"]),
        )
