import json
from pathlib import Path

from pubmedqa.dataset_adapter import PubmedqaAdapter
from pubmedqa.models import PubmedqaInput, PubmedqaPrivateData


def test_adapter_contract():
    row = json.loads((Path(__file__).parent / "cases.jsonl").read_text(encoding="utf-8").splitlines()[0])
    sample = PubmedqaAdapter().normalize(row)
    assert isinstance(sample.input, PubmedqaInput)
    assert isinstance(sample.private_data, PubmedqaPrivateData)
    assert sample.sample_id
