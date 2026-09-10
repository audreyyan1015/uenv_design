from uenv.sdk import DatasetAdapter, PreparedSample

from .models import SweProInput, SweProPrivateData


class SweProAdapter(DatasetAdapter):
    """Dataset-owned normalization; runtime resources are prepared by Environment."""

    def normalize(self, row: dict) -> PreparedSample:
        if row.get("benchmark_variant") != "pro":
            raise ValueError("SWE Pro package received another benchmark variant")
        return PreparedSample(
            sample_id=str(row["instance_id"]),
            input=SweProInput(
                instruction=row["problem_statement"], repo=row["repo"], base_commit=row["base_commit"],
                workspace_path=row["workspace_path"], runtime_assets=row["runtime_assets"],
            ),
            private_data=SweProPrivateData(
                test_patch=row["test_patch"], fail_to_pass=row["FAIL_TO_PASS"],
                pass_to_pass=row["PASS_TO_PASS"], evaluation_plan=row["evaluation_plan"],
            ),
        )
