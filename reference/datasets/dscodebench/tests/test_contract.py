import json
from pathlib import Path

from dscodebench.dataset_adapter import DscodebenchAdapter
from dscodebench.models import DscodebenchInput, DscodebenchPrivateData


def test_adapter_contract():
    row = json.loads((Path(__file__).parent / "cases.jsonl").read_text(encoding="utf-8").splitlines()[0])
    sample = DscodebenchAdapter().normalize(row)
    assert isinstance(sample.input, DscodebenchInput)
    assert isinstance(sample.private_data, DscodebenchPrivateData)
    assert sample.sample_id
