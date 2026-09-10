import json
from pathlib import Path

from swe_smith.dataset_adapter import SweSmithAdapter
from swe_smith.models import SweSmithInput, SweSmithPrivateData


def test_adapter_contract():
    row = json.loads((Path(__file__).parent / "cases.jsonl").read_text(encoding="utf-8").splitlines()[0])
    sample = SweSmithAdapter().normalize(row)
    assert isinstance(sample.input, SweSmithInput)
    assert isinstance(sample.private_data, SweSmithPrivateData)
    assert sample.sample_id
