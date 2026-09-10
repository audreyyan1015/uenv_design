import json
from pathlib import Path

from olymmath.dataset_adapter import OlymmathAdapter
from olymmath.models import OlymmathInput, OlymmathPrivateData


def test_adapter_contract():
    row = json.loads((Path(__file__).parent / "cases.jsonl").read_text(encoding="utf-8").splitlines()[0])
    sample = OlymmathAdapter().normalize(row)
    assert isinstance(sample.input, OlymmathInput)
    assert isinstance(sample.private_data, OlymmathPrivateData)
    assert sample.sample_id
