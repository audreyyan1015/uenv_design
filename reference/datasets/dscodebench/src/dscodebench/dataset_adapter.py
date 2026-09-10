from uenv.sdk import DatasetAdapter, PreparedSample

from .models import DscodebenchInput, DscodebenchPrivateData


class DscodebenchAdapter(DatasetAdapter):
    """Dataset-owned normalization; runtime resources are prepared by Environment."""

    def normalize(self, row: dict) -> PreparedSample:
        return PreparedSample(
            sample_id=str(row["problem_id"]),
            input=DscodebenchInput(
                instruction=row["code_problem"], library=row["library"], entry_point=row["entry_point"]
            ),
            private_data=DscodebenchPrivateData(
                tests=row["tests"], num_tests=row["num_tests"], test_seed=row["test_seed"],
                evaluation_plan=row["evaluation_plan"],
            ),
        )
