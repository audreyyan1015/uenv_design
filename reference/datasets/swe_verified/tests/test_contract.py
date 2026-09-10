import json
from pathlib import Path

from swe_verified.dataset_adapter import SweVerifiedAdapter
from swe_verified.models import SweVerifiedInput, SweVerifiedPrivateData


def test_adapter_contract():
    row = json.loads((Path(__file__).parent / "cases.jsonl").read_text(encoding="utf-8").splitlines()[0])
    sample = SweVerifiedAdapter().normalize(row)
    assert isinstance(sample.input, SweVerifiedInput)
    assert isinstance(sample.private_data, SweVerifiedPrivateData)
    assert sample.sample_id
