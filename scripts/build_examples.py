"""Build reference artifacts from real package declarations and Python models.

This development command contains no dataset names, fields, entrypoints, or
scoring rules. A reference package is added under ``reference/datasets``; its
separate executable demonstration uses one public file under ``reference/runs``.
"""
from __future__ import annotations

from copy import deepcopy
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / ".dependencies"))
sys.path.insert(0, str(ROOT / "reference/sdk/src"))
sys.path.insert(0, str(ROOT / "reference/shared/src"))
sys.path.insert(0, str(ROOT / "reference"))

from fixture_plan import fixture_plan
from package_loader import build_manifest, discover_packages, expand_run, load_package, load_yaml, validate_author_package
from uenv.sdk import model_to_envelope
from uenv.sdk.schema_registry import SchemaRegistry, canonical_bytes


def digest(value) -> str:
    return "sha256:" + hashlib.sha256(canonical_bytes(value)).hexdigest()


def write_json(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def load_case(package_dir: Path) -> dict:
    lines = [line for line in (package_dir / "tests/cases.jsonl").read_text(encoding="utf-8").splitlines() if line.strip()]
    if not lines:
        raise ValueError(f"Package has no test case: {package_dir}")
    return json.loads(lines[0])


def prepare_wire(package: dict, raw: dict) -> tuple[dict, dict | None, dict | None, str]:
    sample = package["adapter"]().normalize(deepcopy(raw))
    if not isinstance(sample.input, package["input_model"]):
        raise TypeError("Adapter returned the wrong input model")
    if sample.private_data is not None:
        private_model = package.get("private_data_model")
        if private_model is None or not isinstance(sample.private_data, private_model):
            raise TypeError("Adapter returned private data that dataset.yaml does not declare")
    return (
        model_to_envelope(sample.input),
        model_to_envelope(sample.private_data) if sample.private_data is not None else None,
        deepcopy(sample.runtime),
        sample.sample_id,
    )


def main() -> None:
    dataset_root = ROOT / "reference/datasets"
    generated_root = ROOT / "reference/generated"
    catalog = load_yaml(ROOT / "reference/catalog/components.yaml")
    packages = [load_package(path) for path in discover_packages(dataset_root)]
    if not packages:
        raise RuntimeError("No dataset packages found")

    manifests = {}
    for package in packages:
        name = package["directory"].name
        for warning in validate_author_package(package):
            print(f"warning: {warning}")
        manifests[name] = build_manifest(package, generated_root / "packages" / name)

    registry = SchemaRegistry.bundled()
    for run_path in sorted((ROOT / "reference/runs").glob("*.yaml")):
        public_run = load_yaml(run_path)
        selected = public_run["environment"]["implementation"]
        package = next((p for p in packages if all(
            p["declaration"][key] == selected[key] for key in ("id", "version"))), None)
        if package is None:
            raise ValueError(f"No registered environment package for {run_path}")
        name = run_path.stem
        declaration = package["declaration"]
        manifest = manifests[package["directory"].name]
        registry.validate("PackageManifest", manifest)

        raw = load_case(package["directory"])
        input_value, private_data, runtime, sample_id = prepare_wire(package, raw)
        dataset_id = declaration["id"].removeprefix("datasets/")
        task = {
            "schema_version": "vnext.3",
            "task_id": f"{dataset_id}/{sample_id}",
            "dataset": {
                "id": dataset_id,
                "revision": "synthetic-design-fixture-v1",
                "split": "example",
                "subset": "",
            },
            "sample_id": sample_id,
            "input": input_value,
            "input_digest": digest(input_value),
        }
        if runtime is not None:
            task["runtime"] = runtime
        episode = {
            "request_id": f"request-{name}",
            "episode_id": f"episode-{name}",
            "task": task,
            "seed": 0,
            "sample_index": 0,
            "batch_id": f"batch-{name}",
        }
        if private_data is not None:
            episode["private_data"] = private_data

        run = expand_run(public_run, manifest, catalog)
        if "scorer" not in run:
            episode.pop("private_data", None)
        batch = {"batch_id": episode["batch_id"], "run_spec": run, "episodes": [episode]}
        registry.validate("BatchRequest", batch)
        registry.validate("RunSpec", run)
        registry.validate("TaskSpec", task)
        registry.validate("EpisodeRequest", episode)
        plan = fixture_plan(episode, run, manifest, registry, catalog)

        output = generated_root / "episodes" / name
        write_json(output / "batch_request.json", batch)
        write_json(output / "task.json", task)
        write_json(output / "episode_request.json", episode)
        write_json(output / "run_spec.json", run)
        write_json(output / "execution_plan.json", plan)

    print(f"Generated {len(packages)} packages from dataset.yaml and models.py; all public runs expanded without author-supplied schema_ref.")


if __name__ == "__main__":
    main()
