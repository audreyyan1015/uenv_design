import json
from pathlib import Path

from swe_pro.dataset_adapter import SweProAdapter
from swe_pro.models import SweProInput, SweProPrivateData


def test_adapter_contract():
    row = json.loads((Path(__file__).parent / "cases.jsonl").read_text(encoding="utf-8").splitlines()[0])
    sample = SweProAdapter().normalize(row)
    assert isinstance(sample.input, SweProInput)
    assert isinstance(sample.private_data, SweProPrivateData)
    assert sample.sample_id
