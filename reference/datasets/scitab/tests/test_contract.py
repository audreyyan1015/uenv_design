import json
from pathlib import Path

from scitab.dataset_adapter import ScitabAdapter
from scitab.models import ScitabInput, ScitabPrivateData


def test_adapter_contract():
    row = json.loads((Path(__file__).parent / "cases.jsonl").read_text(encoding="utf-8").splitlines()[0])
    sample = ScitabAdapter().normalize(row)
    assert isinstance(sample.input, ScitabInput)
    assert isinstance(sample.private_data, ScitabPrivateData)
    assert sample.sample_id
