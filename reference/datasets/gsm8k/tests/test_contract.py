import json
from pathlib import Path

from gsm8k.dataset_adapter import Gsm8kAdapter
from gsm8k.models import Gsm8kInput, Gsm8kPrivateData


def test_adapter_contract():
    row = json.loads((Path(__file__).parent / "cases.jsonl").read_text(encoding="utf-8").splitlines()[0])
    sample = Gsm8kAdapter().normalize(row)
    assert isinstance(sample.input, Gsm8kInput)
    assert isinstance(sample.private_data, Gsm8kPrivateData)
    assert sample.sample_id
