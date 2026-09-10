import json
from pathlib import Path

from swe_lite.dataset_adapter import SweLiteAdapter
from swe_lite.models import SweLiteInput, SweLitePrivateData


def test_adapter_contract():
    row = json.loads((Path(__file__).parent / "cases.jsonl").read_text(encoding="utf-8").splitlines()[0])
    sample = SweLiteAdapter().normalize(row)
    assert isinstance(sample.input, SweLiteInput)
    assert isinstance(sample.private_data, SweLitePrivateData)
    assert sample.sample_id
