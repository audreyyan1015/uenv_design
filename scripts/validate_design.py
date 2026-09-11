"""Validate contracts, Python extension examples, and the Rust control reference."""
from pathlib import Path
import ast
import copy
import hashlib
import importlib
import inspect
import json
import math
import subprocess
import shutil
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / ".dependencies"))
sys.path.insert(0, str(ROOT / "reference"))
sys.path.insert(0, str(ROOT / "reference/sdk/src"))
sys.path.insert(0, str(ROOT / "reference/shared/src"))
for package_source in sorted((ROOT / "reference/datasets").glob("*/src")):
    sys.path.insert(0, str(package_source))

from jsonschema import Draft202012Validator, ValidationError
from package_loader import build_manifest, discover_packages, expand_run, load_package, load_yaml, validate_author_package
from uenv.sdk import AgentRuntimeError, Observation, AgentContext, DatasetAdapter, Environment, EnvironmentContext, ScoreInput, ScoreResult, Scorer, ScoringContext, ToolExecutor, UEnvModel, model_from_envelope, model_json_schema, task_from_wire, text_part, to_wire
from uenv.sdk.schema_registry import SchemaRegistry, canonical_bytes
from gsm8k.scorer import Gsm8kScorer
from olymmath.scorer import OlymmathScorer
from pubmedqa.scorer import PubmedqaScorer
from scitab.scorer import ScitabScorer

SCHEMA = json.loads((ROOT / "contracts/uenv.schema.json").read_text(encoding="utf-8"))
REGISTRY = SchemaRegistry.bundled()
PACKAGE_ROOT = ROOT / "reference/datasets"
GENERATED_ROOT = ROOT / "reference/generated"
PACKAGES = {path.name: load_package(path) for path in discover_packages(PACKAGE_ROOT)}
CATALOG = load_yaml(ROOT / "reference/catalog/components.yaml")


def load(path):
    return json.loads(path.read_text(encoding="utf-8"))


def validate(name, value):
    REGISTRY.validate(name, value)


def generated(name, filename):
    return load(GENERATED_ROOT / "episodes" / name / filename)


def component_task(name):
    return task_from_wire(generated(name, "task.json"), PACKAGES[name]["input_model"])


def component_private_data(name):
    value = generated(name, "episode_request.json")["private_data"]
    return model_from_envelope(PACKAGES[name]["private_data_model"], value)


class MemoryFileStore:
    """Test-only artifact helper; production storage is a Rust port."""

    def __init__(self):
        self.objects = {}

    def put_json(self, value):
        content = canonical_bytes(value)
        digest = "sha256:" + hashlib.sha256(content).hexdigest()
        reference = {"uri": "memory://" + digest, "digest": digest, "size_bytes": len(content), "media_type": "application/json"}
        self.objects[reference["uri"]] = content
        return reference


def score_candidate(scorer, answer, private_data, context=None):
    """Unit-test one Python scoring rule; Rust completes the public ScoreResult."""
    dataset_name = scorer.__class__.__module__.split(".", 1)[0]
    if isinstance(private_data, dict):
        private_data = PACKAGES[dataset_name]["private_data_model"](**private_data)
    request = ScoreInput(
        component_task(dataset_name),
        [text_part(answer)],
        MemoryFileStore().put_json([]),
        private_data=private_data,
    )
    candidate = scorer.score(
        request,
        context or ScoringContext(deadline=time.monotonic() + 120, registry=REGISTRY, cancelled=lambda: False),
    )
    if not isinstance(candidate, ScoreResult):
        raise TypeError("Scorer must return ScoreResult")
    if any(getattr(candidate, field) is not None for field in ("status", "scorer", "error")):
        raise ValueError("Scorer cannot set Rust-owned fields")
    if candidate.reward is None or not math.isfinite(candidate.reward):
        raise ValueError("Scorer must return a finite episode reward")
    return candidate


class ContractTests(unittest.TestCase):
    def test_process_scorer_reads_verified_generation_records(self):
        from dataclasses import replace
        from examples.generation_scoring import ExactAnswerProcessScorer
        from uenv.sdk import read_scoring_trajectory
        store = MemoryFileStore()
        identity = {"run_id": "run", "episode_id": "episode", "attempt_id": 1, "task_id": component_task("gsm8k").task_id}
        events = []
        for i, answer in enumerate(["4", "5"]):
            events.append({"schema_version": "vnext.3", **identity, "event_id": f"event-{i}",
                "sequence": i, "occurred_at_ms": 0, "kind": "generation", "payload": {
                    "generation_id": f"g{i}", "model_id": "test", "source": "simulated", "messages": [],
                    "response": [text_part(answer)], "output_token_count": 1, "finish_reason": "stop", "duration_ms": 0}})
        content = b"".join(canonical_bytes(event) + b"\n" for event in events)
        digest = "sha256:" + hashlib.sha256(content).hexdigest()
        segment = {"uri": "memory://" + digest, "digest": digest, "size_bytes": len(content), "media_type": "application/x-ndjson"}
        store.objects[segment["uri"]] = content
        manifest = {"schema_version": "vnext.3", **identity, "event_segments": [segment],
            "event_count": 2, "trajectory_status": "scoring_checkpoint", "created_at_ms": 0}
        request = ScoreInput(component_task("gsm8k"), [text_part("5")], store.put_json(manifest),
            private_data=component_private_data("gsm8k"))
        context = ScoringContext(deadline=time.monotonic()+10, registry=REGISTRY, cancelled=lambda: False,
            read_artifact=lambda ref: store.objects[ref["uri"]])
        result = ExactAnswerProcessScorer({}).score(request, context)
        self.assertEqual(result.reward, 1.0)
        self.assertEqual(result.generation_rewards, [{"generation_id":"g0", "reward":0.0}, {"generation_id":"g1", "reward":1.0}])
        wire = to_wire(result)
        wire.update(status="ok", scorer=generated("gsm8k", "execution_plan.json")["dataset_package"])
        validate("ScoreResult", wire)
        invalid = copy.deepcopy(wire)
        invalid["generation_rewards"][0]["reward"] = None
        with self.assertRaises(ValidationError): validate("ScoreResult", invalid)
        invalid["generation_rewards"][0]["reward"] = float("nan")
        with self.assertRaises(ValueError): validate("ScoreResult", invalid)
        manifest["event_count"] = 3
        request = replace(request, trajectory_ref=store.put_json(manifest))
        with self.assertRaisesRegex(ValueError, "count mismatch"): read_scoring_trajectory(request, context)
        manifest["event_count"] = 2
        manifest["attempt_id"] = 2
        request = replace(request, trajectory_ref=store.put_json(manifest))
        with self.assertRaisesRegex(ValueError, "identity mismatch"): read_scoring_trajectory(request, context)
        manifest["attempt_id"] = 1
        request = replace(request, trajectory_ref=store.put_json(manifest))
        store.objects[segment["uri"]] = b"tampered"
        with self.assertRaisesRegex(ValueError, "integrity mismatch"): read_scoring_trajectory(request, context)


    def test_yaml_rejects_duplicate_configuration_keys(self):
        for content in ["limits: {}\nlimits: {}\n", "limits:\n  max_generations: 1\n  max_generations: 2\n"]:
            with tempfile.TemporaryDirectory(prefix="uenv-yaml-") as folder:
                path = Path(folder) / "run.yaml"
                path.write_text(content, encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "Duplicate YAML key"):
                    load_yaml(path)

    def test_dataset_package_is_the_only_role_selection(self):
        from fixture_plan import fixture_plan
        manifest = load(GENERATED_ROOT / "packages/gsm8k/manifest.json")
        public = load_yaml(ROOT / "reference/runs/gsm8k.yaml")
        run = expand_run(public, [manifest], CATALOG)
        plan = fixture_plan(generated("gsm8k", "episode_request.json"), run, [manifest], REGISTRY, CATALOG)
        self.assertEqual(plan["dataset_package"]["id"], public["dataset_package"]["id"])
        self.assertNotIn("implementation", plan["environment"])
        self.assertNotIn("scorer", plan)
        for field in ("scorer", "environment", "scoring"):
            invalid = copy.deepcopy(public)
            selection = {"implementation": {"id": "other/package", "version": "2.0.0"}}
            if field == "scoring":
                invalid[field].update(selection)
            else:
                invalid[field] = selection
            with self.assertRaises((ValueError, ValidationError)):
                expand_run(invalid, [manifest], CATALOG)
        for enabled in (None, "false", 0):
            invalid = copy.deepcopy(public)
            invalid["scoring"]["enabled"] = enabled
            with self.assertRaises(ValueError):
                expand_run(invalid, [manifest], CATALOG)
        unscored = load_yaml(ROOT / "reference/runs/gsm8k_collection.yaml")
        unscored["scoring"]["config"] = {}
        with self.assertRaisesRegex(ValueError, "forbidden"):
            expand_run(unscored, [manifest], CATALOG)

    def test_one_run_selects_python_and_native_tools_with_early_compatibility_checks(self):
        public = load_yaml(ROOT / 'reference/runs/gsm8k.yaml')
        public['agent'] = {'implementation':{'id':'agents/openhands','version':'1.0.0'}}
        public['tools'] = [
            {'name':'terminal','implementation':{'id':'agents/openhands/tools/terminal','version':'1.0.0'}},
            {'name':'echo','implementation':{'id':'tools/echo','version':'1.0.0'}},
        ]
        catalog = copy.deepcopy(CATALOG)
        catalog['components']['tools/echo'] = {'role':'tool','version':'1.0.0',
            'config_schema':'uenv://schemas/vnext/EmptyConfig', 'execution_scope':'sandbox',
            'required_capabilities':[]}
        manifests = [load(GENERATED_ROOT / 'packages/gsm8k/manifest.json')]
        result = expand_run(public, manifests, catalog)
        self.assertEqual([v['name'] for v in result['tools']], ['terminal','echo'])
        wrong = copy.deepcopy(public)
        wrong['agent']['implementation']['id'] = 'agents/plain'
        with self.assertRaisesRegex(ValueError, 'NATIVE_TOOL_AGENT_MISMATCH'):
            expand_run(wrong, manifests, catalog)
        wrong = copy.deepcopy(public)
        wrong['tools'][0]['implementation']['version'] = '2'
        with self.assertRaisesRegex(ValueError, 'NATIVE_TOOL_VERSION_MISMATCH'):
            expand_run(wrong, manifests, catalog)
        wrong = copy.deepcopy(public)
        wrong['tools'][1]['name'] = 'terminal'
        with self.assertRaisesRegex(ValueError, 'DUPLICATE_TOOL_NAME'):
            expand_run(wrong, manifests, catalog)
        wrong = copy.deepcopy(public)
        wrong['agent']['config'] = {'tools':['hidden']}
        with self.assertRaises(ValidationError): expand_run(wrong, manifests, catalog)

    def test_dataset_tool_publishes_schema_from_function_without_duplicate_yaml(self):
        from uenv.sdk import tool
        from package_loader import lookup_tool
        @tool
        def add(left: int, right: int) -> int:
            return left + right
        package = {**PACKAGES['gsm8k'], 'tools':{'add':add}}
        with tempfile.TemporaryDirectory(prefix='uenv-tool-package-') as folder:
            output = Path(folder) / 'generated/packages/gsm8k'
            manifest = build_manifest(package, output)
            validate('PackageManifest', manifest)
            spec = manifest['provided_tools'][0]
            self.assertEqual(spec['implementation']['id'], 'datasets/gsm8k/tools/add')
            self.assertEqual(set(load(output / 'schemas/add.input.schema.json')['properties']), {'left','right'})
            definition = lookup_tool(CATALOG, spec['implementation'], {'id':'agents/plain','version':'1.0.0'}, [manifest])
            self.assertNotIn('native_agent', definition)
            self.assertNotIn('interfaces', spec)

    def test_unregistered_component_versions_are_rejected(self):
        manifest = load(GENERATED_ROOT / "packages/gsm8k/manifest.json")
        for role in ("agent", "backend"):
            public = load_yaml(ROOT / "reference/runs/gsm8k.yaml")
            public[role]["implementation"]["version"] = "999.0.0"
            with self.assertRaisesRegex(ValueError, "Unknown"):
                expand_run(public, [manifest], CATALOG)

    def test_contract_authoring_sources_are_unambiguous(self):
        template = (ROOT / "docs/guides/dataset_package_template.md").read_text(encoding="utf-8")
        module_map = (ROOT / "docs/generated/module_map.md").read_text(encoding="utf-8")
        main_design = (ROOT / "docs/uenv_design.md").read_text(encoding="utf-8")
        user_guide = (ROOT / "docs/guides/user_guide.md").read_text(encoding="utf-8")
        self.assertNotIn("  schemas/", template)
        self.assertIn("models.py", template)
        self.assertIn("contracts/proto/uenv/v1/", module_map)
        self.assertIn("系统协议的唯一可编辑来源", main_design)
        self.assertIn("用户不创建 `schemas/`", main_design)
        self.assertIn("uenv describe PreparedSample", template)
        self.assertNotIn("uenv describe system", template + user_guide + main_design)
        self.assertIn("不手写 `TypedConfig` 或 `schema_ref`", template)
        self.assertIn("contract_inspector.py", module_map)
        self.assertIn("包 metadata 已删除", main_design)
        self.assertIn("无法百分之百判断", user_guide)

    def test_schema_is_valid_and_uses_user_facing_names(self):
        Draft202012Validator.check_schema(SCHEMA)
        banned_values = {"done", "complete", "canceled", "enabled", "disabled", "on", "off", "normal", "standard"}
        banned_fields = {
            "sandbox", "sandbox_mode", "network_policy", "system_access_level",
            "syscall_profile", "command_mode", "env_type", "pool_id",
            "agent_pool_id", "task_view", "score_facts", "frozen_outcome",
            "trace", "complete", "last_sequence", "sealed_at_ms",
        }

        def check(value):
            if isinstance(value, dict):
                for field in value.get("properties", {}):
                    self.assertNotIn(field, banned_fields)
                for item in value.get("enum", []):
                    if isinstance(item, str):
                        self.assertEqual(item, item.lower())
                        self.assertNotIn(item, banned_values)
                for child in value.values():
                    check(child)
            elif isinstance(value, list):
                for child in value:
                    check(child)

        for document in (SCHEMA, *REGISTRY.extensions.values()):
            check(document)

        self.assertNotIn("PackageMetadata", SCHEMA["$defs"])
        self.assertNotIn("metadata", SCHEMA["$defs"]["PackageManifest"]["properties"])
        self.assertNotIn("schema_version", SCHEMA["$defs"]["PackageManifest"]["properties"])
        self.assertNotIn("metadata", SCHEMA["$defs"]["ExecutionPlan"]["properties"])

    def test_nine_packages_share_one_contract(self):
        packages = discover_packages(PACKAGE_ROOT)
        self.assertEqual(len(packages), 9)
        for package in packages:
            with self.subTest(package=package.name):
                manifest = load(GENERATED_ROOT / "packages" / package.name / "manifest.json")
                task = generated(package.name, "task.json")
                episode = generated(package.name, "episode_request.json")
                public_run = load_yaml(ROOT / "reference/runs" / f"{package.name}.yaml")
                run = generated(package.name, "run_spec.json")
                validate("PackageManifest", manifest)
                validate("TaskSpec", task)
                validate("RunSpec", run)
                validate("EpisodeRequest", episode)
                validate("ExecutionPlan", generated(package.name, "execution_plan.json"))
                self.assertEqual(episode["task"], task)
                self.assertEqual(task["input"]["schema_ref"], manifest["task_schema"])
                self.assertEqual(episode["private_data"]["schema_ref"], manifest["private_schema"])
                self.assertNotIn("private_data", task)
                self.assertNotIn("backend", manifest)
                self.assertNotIn("agent", manifest)
                self.assertNotIn("dependencies", manifest)
                self.assertNotIn("metadata", manifest)
                self.assertNotIn("schema_version", manifest)
                self.assertNotIn("schema_ref", json.dumps(public_run))
                self.assertNotIn("schema_version", public_run)
                self.assertEqual(expand_run(public_run, [manifest], CATALOG), run)
                self.assertFalse((package / "manifest.json").exists())
                self.assertFalse((package / "episode_request.json").exists())
                self.assertFalse((package / "execution_plan.json").exists())

    def test_batches_carry_one_config_without_episode_overrides(self):
        self.assertEqual(set(SCHEMA["$defs"]["BatchRequest"]["properties"]), {"batch_id", "run_spec", "episodes"})
        self.assertNotIn("run_id", SCHEMA["$defs"]["EpisodeRequest"]["properties"])
        for name in PACKAGES:
            batch = generated(name, "batch_request.json")
            with self.subTest(dataset=name):
                validate("BatchRequest", batch)
                self.assertEqual(batch["run_spec"], generated(name, "run_spec.json"))
                self.assertEqual(batch["episodes"], [generated(name, "episode_request.json")])
                self.assertEqual(batch["run_spec"]["run_id"], generated(name, "execution_plan.json")["run_id"])
                for field, value in (("run_spec", batch["run_spec"]), ("run_id", "override"), ("agent", batch["run_spec"]["agent"])):
                    invalid = copy.deepcopy(batch)
                    invalid["episodes"][0][field] = value
                    with self.subTest(field=field), self.assertRaises(ValidationError):
                        validate("BatchRequest", invalid)
                empty = copy.deepcopy(batch)
                empty["episodes"] = []
                with self.assertRaises(ValidationError):
                    validate("BatchRequest", empty)
                legacy = copy.deepcopy(batch)
                legacy["run_id"] = legacy.pop("run_spec")["run_id"]
                with self.assertRaises(ValidationError):
                    validate("BatchRequest", legacy)

    def test_nine_reference_fixtures_use_the_documented_author_layout(self):
        central_contract_source = (ROOT / "scripts/build_contracts.py").read_text(encoding="utf-8")
        generic_builder_source = (ROOT / "scripts/build_examples.py").read_text(encoding="utf-8")
        for package_dir in discover_packages(PACKAGE_ROOT):
            with self.subTest(package=package_dir.name):
                declaration = load_yaml(package_dir / "dataset.yaml")
                source = package_dir / "src" / package_dir.name
                self.assertTrue((package_dir / "pyproject.toml").exists())
                self.assertTrue((source / "models.py").exists())
                self.assertTrue((package_dir / "tests/cases.jsonl").exists())
                self.assertTrue((package_dir / "tests/test_contract.py").exists())
                self.assertEqual(set(declaration["entrypoints"]), {"dataset_adapter", "environment", "scorer"})
                self.assertEqual(set(declaration["models"]), {"input", "private_data"})
                self.assertNotIn('action', declaration['models'])
                self.assertNotIn(PACKAGES[package_dir.name]["input_model"].__name__, central_contract_source)
                self.assertNotIn(PACKAGES[package_dir.name]["private_data_model"].__name__, central_contract_source)
                self.assertNotIn(package_dir.name, generic_builder_source)
                author_files = "".join(path.read_text(encoding="utf-8") for path in package_dir.rglob("*.py"))
                self.assertNotIn("schema_ref", author_files)

    def test_package_models_are_the_schema_source(self):
        for name, package in PACKAGES.items():
            manifest = load(GENERATED_ROOT / "packages" / name / "manifest.json")
            for role, manifest_field in (("input", "task_schema"), ("private_data", "private_schema")):
                model = package[role + "_model"]
                expected = model_json_schema(model, manifest[manifest_field])
                actual = load(GENERATED_ROOT / "packages" / name / "schemas" / f"{model.__name__}.schema.json")
                self.assertEqual(actual, expected)
                if name.startswith("swe_") and role == "input":
                    self.assertNotIn("variant", actual["properties"])

    def test_no_duplicate_environment_action_interface(self):
        class LegacyEnvironment(Environment):
            def reset(self, task, context): return Observation()
            def step(self, action, context): return Observation()
        with self.assertRaisesRegex(ValueError, 'tools.py'):
            validate_author_package({**PACKAGES['gsm8k'], 'environment': LegacyEnvironment})
        for name in PACKAGES:
            self.assertNotIn('action_schema', load(GENERATED_ROOT / 'packages' / name / 'manifest.json'))
        self.assertNotIn('AnswerAction', SCHEMA['$defs'])


    def test_package_validation_distinguishes_errors_from_semantic_warnings(self):
        class DuplicateTimeout(UEnvModel):
            timeout: int

        class SuspectAllowedTime(UEnvModel):
            allowed_time: int

        declaration = copy.deepcopy(PACKAGES["gsm8k"]["declaration"])
        declaration["id"] = "datasets/validation-example"
        with self.assertRaisesRegex(ValueError, "timeout"):
            validate_author_package({"declaration": declaration, "input_model": DuplicateTimeout})
        warnings = validate_author_package({"declaration": declaration, "input_model": SuspectAllowedTime})
        self.assertEqual(len(warnings), 1)
        self.assertIn("cannot determine semantic equivalence", warnings[0])

    def test_package_config_and_nested_models_cannot_shadow_execution_limits(self):
        class DuplicateBudget(UEnvModel):
            max_generations: int
        class NestedConfig(UEnvModel):
            options: list[DuplicateBudget]
        class SuspectConfig(UEnvModel):
            allowed_time: int
        class Weather(UEnvModel):
            temperature: float
        declaration = PACKAGES["gsm8k"]["declaration"]
        for role in ("environment_config", "scorer_config", "observation", "state"):
            with self.subTest(role=role), self.assertRaisesRegex(ValueError, "max_generations"):
                validate_author_package({"declaration": declaration, role + "_model": NestedConfig})
        warnings = validate_author_package({"declaration": declaration, "environment_config_model": SuspectConfig})
        self.assertEqual(len(warnings), 1)
        self.assertEqual(validate_author_package({"declaration": declaration, "input_model": Weather}), [])

    def test_package_entrypoints_reject_nonclasses_abstract_and_indirect_classes(self):
        declaration = PACKAGES["gsm8k"]["declaration"]
        class IndirectEnvironment(PACKAGES["gsm8k"]["environment"]):
            pass
        for value in (lambda: None, Environment, IndirectEnvironment):
            with self.subTest(value=value), self.assertRaisesRegex(TypeError, "concrete direct subclass"):
                validate_author_package({"declaration": declaration, "environment": value})
        with tempfile.TemporaryDirectory(prefix="uenv-contract-") as folder:
            package_dir = Path(folder) / "author"
            shutil.copytree(PACKAGE_ROOT / "gsm8k", package_dir,
                            ignore=shutil.ignore_patterns("tests", "__pycache__"))
            declaration = copy.deepcopy(declaration)
            declaration["entrypoints"]["environment"] = "extension_templates:BorrowedEnvironment"
            module = importlib.import_module("extension_templates")
            module.BorrowedEnvironment = PACKAGES["gsm8k"]["environment"]
            try:
                (package_dir / "dataset.yaml").write_text(json.dumps(declaration), encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "not an alias"):
                    load_package(package_dir)
            finally:
                del module.BorrowedEnvironment

    def test_each_dataset_owns_three_direct_subclasses(self):
        prefixes = {"gsm8k": "Gsm8k", "pubmedqa": "Pubmedqa", "scitab": "Scitab", "olymmath": "Olymmath", "dscodebench": "Dscodebench", "swe_verified": "SweVerified", "swe_lite": "SweLite", "swe_pro": "SwePro", "swe_smith": "SweSmith"}
        seen = set()
        for name, prefix in prefixes.items():
            folder = PACKAGE_ROOT / name
            manifest = load(GENERATED_ROOT / "packages" / name / "manifest.json")
            for filename, suffix, base, entry in (("dataset_adapter", "Adapter", DatasetAdapter, "dataset_adapter"), ("environment", "Environment", Environment, "environment"), ("scorer", "Scorer", Scorer, "scorer")):
                module_name = f"{name}.{filename}"
                cls = getattr(importlib.import_module(module_name), prefix + suffix)
                declared = {node.name for node in ast.parse((folder / "src" / name / f"{filename}.py").read_text(encoding="utf-8")).body if isinstance(node, ast.ClassDef)}
                self.assertIn(prefix + suffix, declared)
                self.assertEqual(cls.__module__, module_name)
                self.assertEqual(cls.__bases__, (base,))
                self.assertFalse(inspect.isabstract(cls))
                self.assertEqual(manifest["entrypoints"][entry], f"{module_name}:{prefix}{suffix}")
                self.assertNotIn(cls, seen)
                seen.add(cls)
        self.assertEqual(len(seen), 27)

    def test_every_environment_resets_through_same_interface(self):
        for package in discover_packages(PACKAGE_ROOT):
            manifest = load(GENERATED_ROOT / "packages" / package.name / "manifest.json")
            module, symbol = manifest["entrypoints"]["environment"].split(":")
            environment = getattr(importlib.import_module(module), symbol)({})
            observation = environment.reset(
                component_task(package.name),
                EnvironmentContext(
                    REGISTRY,
                    MemoryFileStore(),
                    cancelled=lambda: False,
                    deadline=time.monotonic() + 120,
                    seed=0,
                    session={"session_id": "test-session"},
                ),
            )
            validate("Observation", to_wire(observation))

    def test_unknown_control_fields_are_rejected(self):
        for kind, value in (
            ("PackageManifest", load(GENERATED_ROOT / "packages/gsm8k/manifest.json")),
            ("ExecutionPlan", generated("gsm8k", "execution_plan.json")),
        ):
            value["dependencies"] = []
            with self.subTest(kind=kind), self.assertRaises(ValidationError):
                validate(kind, value)
        for field, value in (("dependencies", []), ("metadata", {}), ("schema_version", "vnext.3")):
            declaration = copy.deepcopy(PACKAGES["gsm8k"]["declaration"])
            declaration[field] = value
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "Unknown dataset.yaml fields"):
                validate_author_package({"declaration": declaration})
            manifest = load(GENERATED_ROOT / "packages/gsm8k/manifest.json")
            manifest[field] = value
            with self.subTest(field=field), self.assertRaises(ValidationError):
                validate("PackageManifest", manifest)
        task = generated("gsm8k", "task.json")
        for field, value in (("env_type", "math"), ("backend", "docker"), ("scoring_ref", "duplicate")):
            changed = copy.deepcopy(task)
            changed[field] = value
            with self.subTest(field=field), self.assertRaises(ValidationError):
                validate("TaskSpec", changed)

    def test_package_declaration_rejects_coercion_and_unknown_roles(self):
        for field, value in (("internet_access", "false"), ("internet_access", 0),
                             ("required_capabilities", ["exec.v1", "exec.v1"]),
                             ("required_capabilities", [1]), ("required_capabilities", "exec.v1"),
                             ("models", {"input": "a:B", "typo": "a:C"})):
            declaration = copy.deepcopy(PACKAGES["gsm8k"]["declaration"])
            declaration[field] = value
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                validate_author_package({"declaration": declaration})
        manifest = load(GENERATED_ROOT / "packages/gsm8k/manifest.json")
        public_run = load_yaml(ROOT / "reference/runs/gsm8k.yaml")
        public_run["schema_version"] = "vnext.3"
        with self.assertRaisesRegex(ValueError, "schema_version is generated"):
            expand_run(public_run, [manifest], CATALOG)
        public_run.pop("schema_version")
        public_run["dataset_package"]["version"] = "2.0.0"
        with self.assertRaisesRegex(ValueError, "Expected one registered manifest"):
            expand_run(public_run, [manifest], CATALOG)

    def test_package_can_publish_without_tests_and_rejects_version_drift(self):
        with tempfile.TemporaryDirectory(prefix="uenv-contract-") as folder:
            package_dir = Path(folder) / "author"
            shutil.copytree(PACKAGE_ROOT / "gsm8k", package_dir,
                            ignore=shutil.ignore_patterns("tests", "__pycache__"))
            package = load_package(package_dir)
            self.assertFalse((package_dir / "tests").exists())
            manifest = build_manifest(package, Path(folder) / "generated/packages/gsm8k")
            validate("PackageManifest", manifest)
            pyproject = package_dir / "pyproject.toml"
            pyproject.write_text(pyproject.read_text(encoding="utf-8").replace('version = "1.0.0"', 'version = "2.0.0"'), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "project.version must match"):
                load_package(package_dir)

    def test_nested_models_round_trip_and_validate_inner_fields(self):
        from uenv.sdk import ContentPart, Field, model_to_envelope
        from typing import Annotated

        class Details(UEnvModel):
            count: Annotated[int, Field("Visible count", minimum=1)]

        class NestedInput(UEnvModel):
            items: list[Details]
            preview: ContentPart
            note: str | None = None

        identifier = "urn:example:nested:input"
        schema = model_json_schema(NestedInput, identifier)
        registry = SchemaRegistry.bundled()
        registry.register(schema)
        original = NestedInput(items=[Details(count=2)], preview=text_part("hello"))
        wire = model_to_envelope(original)
        registry.validate("TypedConfig", wire)
        restored = model_from_envelope(NestedInput, wire)
        self.assertIsInstance(restored.items[0], Details)
        self.assertEqual(restored.items[0].count, 2)
        self.assertEqual(schema["properties"]["note"]["default"], None)
        wire["data"]["items"][0]["count"] = 0
        with self.assertRaises(ValidationError):
            registry.validate("TypedConfig", wire)

        class BadDetails(UEnvModel):
            timeout: int

        class BadInput(UEnvModel):
            details: BadDetails

        with self.assertRaisesRegex(ValueError, "details.timeout"):
            validate_author_package({"declaration": PACKAGES["gsm8k"]["declaration"], "input_model": BadInput})

    def test_declared_config_and_interaction_models_are_published(self):
        from uenv.sdk import Observation, structured_part

        class CustomObservation(UEnvModel):
            remaining: int

        class CustomConfig(UEnvModel):
            prefix: str = "Question: "

        class ConfiguredEnvironment(Environment):
            def reset(self, task, context):
                return Observation([text_part(self.config["prefix"] + task.input.instruction),
                                    structured_part(CustomObservation(remaining=2))])

        package = {**PACKAGES["gsm8k"], "declaration": copy.deepcopy(PACKAGES["gsm8k"]["declaration"])}
        package["declaration"]["models"].update(environment_config="test:CustomConfig", observation="test:CustomObservation")
        package.update(environment_config_model=CustomConfig, observation_model=CustomObservation, environment=ConfiguredEnvironment)
        with tempfile.TemporaryDirectory(prefix="uenv-contract-") as folder:
            output = Path(folder) / "generated/packages/gsm8k"
            manifest = build_manifest(package, output)
            registry = SchemaRegistry.bundled()
            for path in (output / "schemas").glob("*.json"):
                registry.register(load(path))
            registry.validate("PackageManifest", manifest)
            public_run = load_yaml(ROOT / "reference/runs/gsm8k.yaml")
            public_run["environment"] = {"prefix": "Question: "}
            run = expand_run(public_run, [manifest], CATALOG, registry)
            public_run.pop("environment")
            self.assertEqual(expand_run(public_run, [manifest], CATALOG, registry), run)
            registry.validate("RunSpec", run)
            environment = ConfiguredEnvironment(run["environment"]["data"])
            observation = environment.reset(component_task("gsm8k"), None)
            self.assertEqual(observation.content[0]["text"], "Question: " + component_task("gsm8k").input.instruction)
            registry.validate("Observation", to_wire(observation))
            self.assertTrue((output / "schemas/CustomObservation.schema.json").exists())
            self.assertEqual(observation.content[1]['structured']['data'], {'remaining': 2})
            run["environment"]["data"]["prefix"] = 42
            with self.assertRaises(ValidationError):
                registry.validate("RunSpec", run)

    def test_public_run_defaults_preserve_explicit_values_and_input(self):
        for name in PACKAGES:
            with self.subTest(dataset=name):
                manifest = load(GENERATED_ROOT / "packages" / name / "manifest.json")
                public = load_yaml(ROOT / "reference/runs" / f"{name}.yaml")
                before = copy.deepcopy(public)
                expanded = expand_run(public, [manifest], CATALOG)
                self.assertEqual(public, before)
                explicit = copy.deepcopy(expanded)
                explicit.pop("schema_version")
                explicit["environment"] = explicit["environment"]["data"]
                explicit["scoring"]["config"] = explicit["scoring"]["config"]["data"]
                for role in ("agent", "backend"):
                    explicit[role]["config"] = explicit[role]["config"]["data"]
                for tool in explicit["tools"]:
                    tool["config"] = tool["config"]["data"]
                self.assertEqual(expand_run(explicit, [manifest], CATALOG), expanded)
                explicit["model"]["generation"]["temperature"] = 0.5
                explicit["model"]["max_transport_retries"] = 0
                explicit["limits"]["max_tool_calls"] = 0
                explicit["retry"]["max_attempts"] = 2
                changed = expand_run(explicit, [manifest], CATALOG)
                self.assertEqual(changed["model"]["generation"]["temperature"], 0.5)
                self.assertEqual(changed["model"]["max_transport_retries"], 0)
                self.assertEqual(changed["limits"]["max_tool_calls"], 0)
                self.assertEqual(changed["retry"]["max_attempts"], 2)

    def test_public_run_rejects_removed_fields_and_invalid_values(self):
        manifest = load(GENERATED_ROOT / "packages/swe_verified/manifest.json")
        baseline = load_yaml(ROOT / "reference/runs/swe_verified.yaml")
        for role, config in (("agent", {"sdk_iteration_limit": 30}),
                             ("agent", {"history_policy": "sdk"}),
                             ("agent", {"max_rounds": 30}),
                             ("environment", {"total_timeout_ms": 10}),
                             ("backend", {"runtime_profile": "legacy"}),
                             ("agent", None)):
            changed = copy.deepcopy(baseline)
            if role == "environment":
                changed[role] = config
            else:
                changed[role]["config"] = config
            with self.subTest(role=role, config=config), self.assertRaises(ValidationError):
                expand_run(changed, [manifest], CATALOG)
        for value in (None, False, -1, "0.5"):
            changed = copy.deepcopy(baseline)
            changed["model"]["generation"] = {"temperature": value}
            with self.subTest(temperature=value), self.assertRaises(ValidationError):
                expand_run(changed, [manifest], CATALOG)

    def test_wire_validation_does_not_apply_submission_defaults(self):
        run = generated("gsm8k", "run_spec.json")
        run["model"]["generation"].pop("temperature")
        before = copy.deepcopy(run)
        with self.assertRaises(ValidationError):
            validate("RunSpec", run)
        self.assertEqual(run, before)

    def test_agent_pool_fields_are_rejected(self):
        for field in ("pool_id", "agent_pool_id", "placement"):
            run = generated("gsm8k", "run_spec.json")
            run["agent"][field] = "legacy"
            with self.subTest(field=field), self.assertRaises(ValidationError):
                validate("RunSpec", run)
        self.assertNotIn("AgentPlacement", SCHEMA["$defs"])

    def test_backend_and_agent_are_dataset_independent_configuration(self):
        qa = generated("gsm8k", "run_spec.json")
        swe = generated("swe_verified", "run_spec.json")
        for name in ("gsm8k", "swe_verified"):
            run = generated(name, "run_spec.json")
            run["backend"] = copy.deepcopy(qa["backend"])
            run["agent"] = copy.deepcopy(qa["agent"])
            validate("RunSpec", run)
        self.assertNotEqual(qa["backend"], swe["backend"])

    def test_collection_scoring_is_optional_without_configuration_aliases(self):
        for name, scored in (("gsm8k_collection", False), ("gsm8k_collection_scored", True)):
            batch = generated(name, "batch_request.json")
            plan = generated(name, "execution_plan.json")
            validate("BatchRequest", batch)
            validate("ExecutionPlan", plan)
            self.assertEqual(batch["run_spec"]["purpose"], "trajectory_collection")
            self.assertEqual(plan["scoring"]["enabled"], scored)
            self.assertEqual("private_data" in plan, scored)
            self.assertNotIn("training", plan)
            for invalid_purpose in ("evaluation", "training"):
                invalid = copy.deepcopy(batch["run_spec"])
                invalid["purpose"] = invalid_purpose
                invalid["scoring"] = {"enabled": False}
                with self.assertRaises(ValidationError):
                    validate("RunSpec", invalid)
            invalid = copy.deepcopy(plan)
            invalid["scoring"] = None
            with self.assertRaises(ValidationError):
                validate("ExecutionPlan", invalid)
            invalid = copy.deepcopy(batch["run_spec"])
            invalid["training"] = {}
            with self.assertRaises(ValidationError):
                validate("RunSpec", invalid)
        no_score = generated("gsm8k_collection", "execution_plan.json")
        no_score["private_data"] = generated("gsm8k", "episode_request.json")["private_data"]
        with self.assertRaises(ValidationError):
            validate("ExecutionPlan", no_score)
        limits = SCHEMA["$defs"]["Limits"]["properties"]
        self.assertIn("finalize_reserve_ms", limits)
        self.assertNotIn("score_" + "reserve_ms", limits)

    def test_collection_package_does_not_require_a_scorer_entrypoint(self):
        package = copy.deepcopy(PACKAGES["gsm8k"])
        package["declaration"]["entrypoints"].pop("scorer")
        package["declaration"]["models"].pop("private_data", None)
        package.pop("scorer", None)
        package.pop("private_data_model", None)
        validate_author_package(package)
        with tempfile.TemporaryDirectory(prefix="uenv-collection-package-") as folder:
            manifest = build_manifest(package, Path(folder) / "generated/packages/collection")
            validate("PackageManifest", manifest)
            self.assertNotIn("scorer", manifest["entrypoints"])
            self.assertNotIn("scorer", manifest["config_schemas"])
            self.assertNotIn("private_schema", manifest)
            public = load_yaml(ROOT / "reference/runs/gsm8k_collection.yaml")
            run = expand_run(public, [manifest], CATALOG)
            validate("RunSpec", run)
            public["scoring"]["enabled"] = True
            with self.assertRaisesRegex(ValueError, "Expected one registered manifest for selected scorer"):
                expand_run(public, [manifest], CATALOG)
            invalid = copy.deepcopy(manifest)
            invalid["config_schemas"]["scorer"] = "uenv://schemas/vnext/EmptyConfig"
            with self.assertRaises(ValidationError):
                validate("PackageManifest", invalid)

    def test_purpose_has_exactly_one_matching_configuration_shape(self):
        run = generated("gsm8k", "run_spec.json")
        training = {
            "parallel_mode": "sync",
            "require_token_trace": False,
            "version_policy": "per_generation",
            "requested_policy_version": "",
            "max_policy_lag": 0,
        }
        unexpected = copy.deepcopy(run)
        unexpected["training"] = training
        with self.assertRaises(ValidationError):
            validate("RunSpec", unexpected)
        missing = copy.deepcopy(run)
        missing["purpose"] = "training"
        with self.assertRaises(ValidationError):
            validate("RunSpec", missing)
        training_run = copy.deepcopy(missing)
        training_run["training"] = training
        validate("RunSpec", training_run)

    def test_one_meaning_has_one_public_field(self):
        defs = SCHEMA["$defs"]
        self.assertEqual(list(defs["TaskSpec"]["properties"]).count("input"), 1)
        self.assertEqual(list(defs["EpisodeRequest"]["properties"]).count("private_data"), 1)
        self.assertEqual(list(defs["ExecutionPlan"]["properties"]).count("runtime"), 1)
        self.assertEqual(list(defs["EpisodeResult"]["properties"]).count("score"), 1)
        self.assertEqual(set(defs["ScoreResult"]["properties"]), {"status", "success", "metrics", "reward", "evidence", "scorer", "error", "generation_rewards"})
        self.assertNotIn("EnvironmentTransition", defs)
        self.assertEqual(set(defs["ToolBinding"]["properties"]), {"name", "implementation", "config"})
        self.assertNotIn("adapter", defs["ResolvedToolBinding"]["properties"])
        self.assertNotIn("interface", defs["ResolvedToolBinding"]["properties"])
        self.assertNotIn("EpisodeOutput", defs)
        self.assertEqual(
            defs["EpisodeResult"]["properties"]["termination_reason"]["enum"],
            ["final_answer", "environment_terminal", "environment_truncated", "budget_exhausted"],
        )
        self.assertNotIn("outcome", defs["EpisodeResult"]["properties"])
        self.assertNotIn("state", defs["EpisodeResult"]["properties"])
        self.assertEqual(set(defs["ScoreInput"]["properties"]),
                         {"task", "final_answer", "trajectory_ref", "state", "private_data"})
        self.assertNotIn("termination_reason", defs["TerminalEvent"]["properties"])
        self.assertNotIn("remaining_timeout_ms", defs["ScoreInput"]["properties"])
        self.assertEqual(set(defs["StateEvent"]["properties"]), {"phase", "session"})
        self.assertIn("environment_variables", defs["ExecRequest"]["properties"])
        self.assertNotIn("environment", defs["ExecRequest"]["properties"])
        self.assertIn("provided_tools", defs["PackageManifest"]["properties"])
        self.assertNotIn("tools", defs["PackageManifest"]["properties"])
        self.assertEqual(
            set(defs["WorkerRegistration"]["properties"]),
            {"worker_id", "endpoint", "capacity", "capabilities", "components", "resource_capacity"},
        )
        self.assertNotIn("result_schema", REGISTRY.extensions["uenv://schemas/vnext/EvaluationPlan"]["properties"])
        wire_schema = json.dumps(SCHEMA, ensure_ascii=False)
        for removed in ("max_model_turns", "model_turn_count", "per_turn", "last_turn", "turn_id"):
            self.assertNotIn(removed, wire_schema)
        self.assertIn("max_generations", defs["Limits"]["properties"])
        self.assertIn("generation_count", defs["Usage"]["properties"])
        self.assertIn("output_token_count", defs["GenerationEvent"]["required"])
        self.assertIn("operation_id", defs["ErrorRecord"]["properties"])

        run_fields = defs["RunSpec"]["properties"]
        plan_fields = defs["ExecutionPlan"]["properties"]
        dispatch_fields = defs["DispatchRequest"]["properties"]
        manifest_fields = defs["TrajectoryManifest"]["properties"]
        self.assertIn("trajectory_retention_days", run_fields)
        self.assertNotIn("trajectory_retention_days", plan_fields)
        self.assertNotIn("trace", run_fields)
        self.assertNotIn("trace", plan_fields)
        self.assertEqual(
            set(dispatch_fields),
            {"plan", "lease", "remaining_timeout_ms", "consumed_usage"},
        )
        self.assertEqual(
            set(manifest_fields),
            {
                "schema_version", "run_id", "episode_id", "attempt_id",
                "task_id", "event_segments", "event_count",
                "trajectory_status", "created_at_ms",
            },
        )
        self.assertEqual(
            defs["TrajectoryManifest"]["properties"]["trajectory_status"]["enum"],
            ["scoring_checkpoint", "final_complete", "final_partial"],
        )
        self.assertNotIn("request_id", defs["BatchRequest"]["properties"])
        self.assertEqual(
            set(defs["BatchReceipt"]["properties"]),
            {"batch_id", "episode_ids"},
        )

    def test_observation_content_unifies_typed_text_and_file_items(self):
        from examples.counter_environment import CounterState, register_schemas
        from uenv.sdk import structured_part
        registry = SchemaRegistry.bundled()
        register_schemas(registry)
        model = CounterState(value=2, goal=5)
        part = structured_part(model)
        reference = MemoryFileStore().put_json({'image': 'fixture'})
        observation = Observation([text_part('Position'),
                                   {'kind': 'artifact', 'artifact': reference}, part])
        wire = to_wire(observation)
        self.assertEqual(set(wire), {'content', 'terminated', 'episode_truncated'})
        registry.validate('Observation', wire)
        registry.validate('Message', {'role': 'user', 'content': wire['content']})
        restored = model_from_envelope(CounterState, part['structured'])
        self.assertEqual(restored.value, 2)
        model.value = 99
        self.assertEqual(part['structured']['data']['value'], 2)
        with self.assertRaises(TypeError):
            Observation(content=[], data=model)
        with self.assertRaises(TypeError):
            structured_part({'value': 2})
        with self.assertRaises(ValidationError):
            registry.validate('Observation', dict(wire, data=part['structured']))
        invalids = [
            {'kind': 'structured'},
            {**part, 'text': 'duplicate'},
            {'kind': 'text', 'text': 'x', 'structured': part['structured']},
            {'kind': 'structured', 'structured': {'schema_ref': 'unknown', 'data': {}}},
            {'kind': 'structured', 'structured': {'schema_ref': part['structured']['schema_ref'],
                                                'data': {'value': -1, 'goal': 5}}},
            {'kind': 'structured', 'structured': {'schema_ref': part['structured']['schema_ref'],
                                                'data': {'value': '2', 'goal': 5}}},
        ]
        for invalid in invalids:
            with self.subTest(part=invalid), self.assertRaises(ValidationError):
                registry.validate('Observation', {**wire, 'content': [invalid]})

    def test_tool_result_structured_data_uses_registered_schema(self):
        result = {"tool_call_id":"call-1", "status":"ok", "observation":to_wire(Observation()), "output_truncated":False}
        validate("ToolResult", result)
        result["observation"]["content"] = [{"kind":"structured", "structured":{
            "schema_ref":"uenv://schemas/vnext/EmptyConfig", "data":{}}}]
        validate("ToolResult", result)
        result["observation"]["content"][0]["structured"]["data"]["undeclared"] = True
        with self.assertRaises(ValidationError): validate("ToolResult", result)
        self.assertNotIn("data", SCHEMA["$defs"]["ToolResult"]["properties"])


    def test_agent_exports_native_tools_without_interface_negotiation(self):
        defs = SCHEMA["$defs"]
        component = {"id": "agents/example", "version": "1.0.0"}
        artifact = MemoryFileStore().put_json({"type": "object"})
        validate("AgentManifest", {
            "implementation": component,
            "entrypoint": "example.agent:ExampleAgent",
            "config_schema": "uenv://schemas/vnext/EmptyConfig",
            "provided_tools": [],
            "required_tool_names": [],
        })
        validate("ToolSpec", {
            "implementation": {"id": "tools/example", "version": "1.0.0"},
            "entrypoint": "example.tool:execute",
            "config_schema": "uenv://schemas/vnext/EmptyConfig",
            "description": "Example tool",
            "execution_scope": "sandbox",
            "required_capabilities": [],
            "input_schema": artifact,
            "output_schema": artifact,
            "side_effect": "read_only",
        })
        self.assertNotIn("supported_bindings", defs["AgentManifest"]["properties"])
        self.assertNotIn("supported_tools", defs["AgentManifest"]["properties"])

    def test_python_reference_contains_only_extension_interfaces(self):
        sdk = importlib.import_module("uenv.sdk")
        templates = importlib.import_module("extension_templates")
        self.assertFalse(hasattr(sdk, "evaluate_score"))
        self.assertFalse(hasattr(templates, "SandboxBackend"))
        self.assertIs(sdk.AgentContext, AgentContext)
        self.assertIs(sdk.ToolExecutor, ToolExecutor)
        self.assertIs(templates.ToolExecutor, ToolExecutor)
        self.assertFalse(hasattr(sdk, "ImportContext"))
        self.assertFalse(hasattr(sdk, "MetricReducer"))
        self.assertNotIn("metric_reducer", SCHEMA["$defs"]["EntryPoints"]["properties"])
        self.assertFalse(hasattr(sdk.Scorer, "aggregate"))
        for cls, required in (
            (sdk.EnvironmentContext, ("cancelled", "deadline", "seed", "session")),
            (sdk.ScoringContext, ("deadline", "registry", "cancelled")),
            (sdk.Environment, ("config",)),
            (sdk.AgentRunner, ("config",)),
            (sdk.ToolExecutor, ("config",)),
            (sdk.Scorer, ("config",)),
        ):
            signature = inspect.signature(cls)
            for name in required:
                self.assertIs(signature.parameters[name].default, inspect.Parameter.empty)
        self.assertNotIn("remaining_timeout_ms", inspect.signature(sdk.ScoreInput).parameters)
        self.assertNotIn("ToolExecutor", {
            node.name
            for node in ast.parse((ROOT / "reference/extension_templates.py").read_text(encoding="utf-8")).body
            if isinstance(node, ast.ClassDef)
        })
        for removed in ("episode_runtime.py", "execution_plan.py", "tool_resolution.py", "plan_fixtures.py"):
            self.assertFalse((ROOT / "reference" / removed).exists())
        self.assertTrue((ROOT / "reference-control/src/supervisor.rs").exists())

    def test_invalid_final_score_state_is_rejected(self):
        score = {"status": "error", "success": False, "metrics": [], "reward": 0, "evidence": [], "scorer": {"id": "x", "version": "1", "digest": "sha256:" + "0" * 64}, "error": {"code": "SCORER_FAILED", "phase": "score", "message": "failure", "retryable": False}}
        with self.assertRaises(ValidationError):
            validate("ScoreResult", score)

    def test_event_kind_binds_payload(self):
        event = {"schema_version": "vnext.3", "event_id": "e", "run_id": "r", "episode_id": "ep", "attempt_id": 1, "task_id": "t", "sequence": 0, "occurred_at_ms": 1, "kind": "generation", "payload": {"state": "running"}}
        with self.assertRaises(ValidationError):
            validate("TrajectoryEvent", event)

    def test_generation_usage_does_not_require_token_trace(self):
        generation = {
            "generation_id": "generation-1",
            "model_id": "example",
            "source": "simulated",
            "messages": [],
            "response": [text_part("answer")],
            "output_token_count": 1,
            "finish_reason": "stop",
            "duration_ms": 1,
        }
        validate("GenerationEvent", generation)
        missing_count = copy.deepcopy(generation)
        missing_count.pop("output_token_count")
        with self.assertRaises(ValidationError):
            validate("GenerationEvent", missing_count)
        unaligned_detail = copy.deepcopy(generation)
        unaligned_detail["output_logprobs"] = [-0.1]
        with self.assertRaises(ValidationError):
            validate("GenerationEvent", unaligned_detail)

    def test_structured_tool_requests_have_one_shape_and_match_message_role(self):
        call = {
            "tool_call_id": "c1", "generation_id": "g1", "name": "lookup",
            "implementation": {"id": "tools/lookup", "version": "1", "digest": "sha256:" + "0" * 64},
            "arguments": {},
            "timeout_ms": 1000,
        }
        event = {"generation_id": "g1", "model_id": "test", "source": "simulated",
                 "messages": [], "response": [], "tool_calls": [call],
                 "finish_reason": "tool_calls", "output_token_count": 1, "duration_ms": 0}
        validate("GenerationEvent", event)
        for replacement in (None, []):
            invalid = copy.deepcopy(event)
            if replacement is None:
                invalid.pop("tool_calls")
            else:
                invalid["tool_calls"] = replacement
            with self.assertRaises(ValidationError):
                validate("GenerationEvent", invalid)
        event["finish_reason"] = "stop"
        with self.assertRaises(ValidationError):
            validate("GenerationEvent", event)
        message = {"role": "assistant", "content": [], "tool_calls": [call]}
        validate("Message", message)
        message["role"] = "user"
        with self.assertRaises(ValidationError):
            validate("Message", message)
        with self.assertRaises(ValidationError):
            validate("Message", {"role": "tool", "content": []})


class PlainAgentContextProbe(AgentContext):
    """Deterministic SDK-side probe; Rust tests independently check real budgets."""
    def __init__(self, finishes, max_generations=10, tool_error=False, denied=None,
                 terminate_after=1, max_steps=10, terminal_tool=False):
        self.observation = Observation([text_part("Task")])
        self.tools = ()
        self.registry = REGISTRY
        self.finishes = finishes
        self.max_generations = max_generations
        self.tool_error = tool_error
        self.denied = denied
        self.requests = []
        self.calls = []
        self.actions = []
        self.parsed = []
        self.terminate_after = terminate_after
        self.max_steps = max_steps
        self.terminal_tool = terminal_tool

    async def generate(self, messages):
        if self.denied:
            raise AgentRuntimeError({"code": self.denied})
        if len(self.requests) >= self.max_generations:
            raise AgentRuntimeError({"code": "GENERATION_LIMIT"})
        self.registry.validate("Message", messages[-1])
        self.requests.append(copy.deepcopy(messages))
        index = len(self.requests)
        reason = self.finishes[index - 1]
        event = {
            "generation_id": f"g{index}", "model_id": "test", "source": "simulated",
            "messages": copy.deepcopy(messages), "response": [text_part(f"response{index}")],
            "finish_reason": reason, "output_token_count": 1, "duration_ms": 0,
        }
        if reason == "tool_calls":
            event["tool_calls"] = [{
                "tool_call_id": f"call{index}", "generation_id": f"g{index}", "name": "lookup",
                "implementation": {"id": "tools/lookup", "version": "1", "digest": "sha256:" + "0" * 64},
                "arguments": {},
                "timeout_ms": 1000,
            }]
        self.registry.validate("GenerationEvent", event)
        return event

    async def call_tool(self, call):
        self.calls.append(copy.deepcopy(call))
        self.observation = Observation([text_part("tool output")], terminated=self.terminal_tool)
        result = {"tool_call_id":call["tool_call_id"], "status":"ok",
                  "observation":to_wire(self.observation), "output_truncated":False}
        if self.tool_error:
            result.update(status="error", error={"code":"LOOKUP_FAILED", "phase":"tool",
                "message":"lookup failed", "retryable":False})
        if self.tool_error:
            result['observation']['content'].append(text_part('Tool error: LOOKUP_FAILED'))
        validate("ToolResult", result)
        self.observation = Observation(**result['observation'])
        return self.observation


class PlainAgentTests(unittest.IsolatedAsyncioTestCase):
    async def test_reset_end_flags_stop_without_generation_and_use_flat_observation(self):
        for terminated, truncated, reason in [
            (True, False, 'environment_terminal'),
            (False, True, 'budget_exhausted'),
        ]:
            context = PlainAgentContextProbe([])
            context.observation = Observation(
                [text_part('already stopped')], terminated=terminated,
                episode_truncated=truncated,
            )
            wire = to_wire(context.observation)
            validate('Observation', wire)
            self.assertNotIn('observation', wire)
            with self.assertRaises(ValidationError):
                validate('Observation', {'observation': wire, 'terminated': terminated,
                                         'episode_truncated': truncated})
            final_answer = await self.agent().run(context)
            self.assertEqual(final_answer, [])
            self.assertEqual(context.requests, [])
            self.assertEqual(context.actions, [])

    def agent(self, history="full"):
        from extension_templates import PlainAgent
        return PlainAgent({"history_policy": history, "system_prompt": "System"})

    async def test_tool_feedback_drives_multiple_generations_with_complete_history(self):
        context = PlainAgentContextProbe(["tool_calls", "tool_calls", "stop"])
        result = await self.agent().run(context)
        self.assertEqual(''.join(part['text'] for part in result), "response3")
        self.assertFalse(context.observation.terminated)
        self.assertEqual(len(context.requests), 3)
        self.assertEqual(len(context.calls), 2)
        self.assertEqual([len(messages) for messages in context.requests], [2, 4, 6])
        self.assertEqual(context.requests[1][-1]["tool_call_id"], "call1")
        self.assertEqual(context.requests[2][-1]["tool_call_id"], "call2")
        self.assertEqual(context.requests[1][-2]["tool_calls"], [context.calls[0]])

    async def test_last_generation_keeps_initial_task_and_complete_latest_exchange(self):
        context = PlainAgentContextProbe(["tool_calls", "tool_calls", "stop"])
        await self.agent("last_generation").run(context)
        self.assertEqual([len(messages) for messages in context.requests], [2, 4, 4])
        self.assertEqual(context.requests[2][:2], context.requests[0])
        self.assertNotIn("call1", json.dumps(context.requests[2]))
        self.assertEqual(context.requests[2][-2]["tool_calls"][0]["tool_call_id"], "call2")

    async def test_run_budget_one_remains_one_and_terminal_answer_stops_early(self):
        cap = load_yaml(ROOT / "reference/runs/gsm8k.yaml")["limits"]["max_generations"]
        self.assertEqual(cap, 1)
        context = PlainAgentContextProbe(["tool_calls"], max_generations=cap)
        result = await self.agent().run(context)
        self.assertIsInstance(result, list)
        self.assertEqual(len(context.requests), 1)
        self.assertEqual(len(context.calls), 1)
        early = PlainAgentContextProbe(["stop"], max_generations=10)
        self.assertEqual(await self.agent().run(early), [text_part("response1")])
        self.assertEqual(len(early.requests), 1)

    async def test_tool_error_is_feedback_but_runtime_failure_is_not_a_budget_end(self):
        context = PlainAgentContextProbe(["tool_calls", "stop"], tool_error=True)
        result = await self.agent().run(context)
        self.assertEqual(''.join(part['text'] for part in result), "response2")
        self.assertIn("LOOKUP_FAILED", json.dumps(context.requests[1][-1]))
        for code in ("EPISODE_CANCELLED", "MODEL_NETWORK_FAILED", "TOOL_NOT_SELECTED"):
            with self.subTest(code=code), self.assertRaises(AgentRuntimeError):
                await self.agent().run(PlainAgentContextProbe([], denied=code))
        partial = PlainAgentContextProbe(["length"])
        final_answer = await self.agent().run(partial)
        self.assertEqual(''.join(part['text'] for part in final_answer), "response1")
        self.assertEqual(final_answer, [text_part("response1")])

    async def test_final_answer_does_not_require_environment_action(self):
        context = PlainAgentContextProbe(['stop'])
        self.assertEqual(await self.agent().run(context), [text_part('response1')])
        self.assertEqual(context.calls, [])
        self.assertEqual(context.actions, [])


    async def test_terminal_tool_stops_without_reexecuting_an_environment_action(self):
        context = PlainAgentContextProbe(['tool_calls'], terminal_tool=True)
        self.assertEqual(await self.agent().run(context), [text_part('response1')])
        self.assertTrue(context.observation.terminated)
        self.assertEqual(len(context.calls), 1)
        self.assertEqual(context.actions, [])


    async def test_every_dataset_submits_original_content_without_scoring(self):
        for name, package in PACKAGES.items():
            environment = package['environment']({})
            context = EnvironmentContext(REGISTRY, MemoryFileStore(), lambda:False, time.monotonic()+60, 0, {})
            observation = environment.reset(component_task(name), context)
            validate('Observation', to_wire(observation))
            self.assertFalse(hasattr(environment, 'step'))
            self.assertFalse(hasattr(environment, 'parse_action'))


    async def test_plain_agent_uses_tools_for_stateful_interaction(self):
        from examples.counter_environment import CounterEnvironment, CounterInput, register_schemas
        from examples.tools import increment
        from uenv.sdk.tools import ToolHost
        from uenv.sdk import TaskSpec
        from types import SimpleNamespace
        registry = SchemaRegistry.bundled()
        register_schemas(registry)
        class Artifacts(MemoryFileStore):
            def put(self, content, media_type):
                return {'uri':'memory://image', 'digest':'sha256:'+hashlib.sha256(content).hexdigest(),
                        'size_bytes':len(content), 'media_type':media_type}
        environment = CounterEnvironment({})
        env_context = EnvironmentContext(registry, Artifacts(), lambda:False, time.monotonic()+60, 0, {})
        initial = environment.reset(TaskSpec('counter', {}, '1', CounterInput(goal=2)), env_context)
        implementation = {'id':'tools/lookup', 'version':'1', 'digest':'sha256:' + '0'*64}
        binding = {'name':'lookup', 'implementation':implementation,
                   'config':{'schema_ref':'uenv://schemas/vnext/EmptyConfig', 'data':{}},
                   'execution_scope':'sandbox', 'required_capabilities':[]}
        host = ToolHost(registry)
        host.prepare([binding], {'tools/lookup':increment},
                     {'lookup':SimpleNamespace(config={}, environment=environment)}, {})
        class Context(PlainAgentContextProbe):
            async def generate(self, messages):
                event = await super().generate(messages)
                event['tool_calls'][0]['arguments'] = {'amount':1}
                return event
            async def call_tool(self, call):
                result = await host.execute(call)  # Test transport; Rust gate verified separately.
                self.observation = Observation(**result['observation'])
                return self.observation
        context = Context(['tool_calls', 'tool_calls'])
        context.registry, context.observation = registry, initial
        await self.agent().run(context)
        self.assertEqual(environment.value, 2)
        self.assertEqual(len(context.requests), 2)
        self.assertTrue(context.observation.terminated)
        self.assertEqual(context.requests[1][-1]['content'][-1]['structured']['data']['value'], 1)



class PythonScorerTests(unittest.TestCase):
    def check_cases(self, scorer, cases):
        for answer, target, expected in cases:
            with self.subTest(answer=answer, target=target):
                result = score_candidate(scorer, answer, {"answer": target})
                self.assertEqual(result.success, expected)
                self.assertEqual(result.reward, float(expected))

    def test_gsm8k_rules(self):
        self.check_cases(Gsm8kScorer({}), [("#### 072", "72", True), ("#### 1/2", "0.5", True), ("#### 6", "16", False)])

    def test_pubmedqa_rules(self):
        self.check_cases(PubmedqaScorer({}), [("The answer is yes.", "yes", True), ("finally maybe.", "maybe", True), ("unknown", "no", False)])

    def test_scitab_rules(self):
        self.check_cases(ScitabScorer({}), [("supports", "supports", True), ("refuted", "refutes", True), ("supports", "refutes", False)])

    def test_olymmath_rules(self):
        self.check_cases(OlymmathScorer({}), [(r"\boxed{\frac{1}{2}}", "0.5", True), (r"\boxed{x = 072}", "72", True), (r"\boxed{6}", "16", False)])

    def test_scoring_state_is_separate_from_observation_and_survives_cleanup(self):
        from examples.counter_environment import CounterEnvironment, CounterInput, ProgressScorer, register_schemas
        from uenv.sdk import TaskSpec
        registry = SchemaRegistry.bundled()
        register_schemas(registry)
        environment = CounterEnvironment({})
        environment.goal, environment.value = 2, 2
        context = EnvironmentContext(registry, MemoryFileStore(), lambda: False,
                                     time.monotonic() + 60, 0, {})
        task = TaskSpec('counter', {}, '1', CounterInput(goal=2))
        # Worker takes an independent snapshot before cleanup, without an output wrapper.
        state = copy.deepcopy(environment.state_snapshot(context))
        self.assertFalse(hasattr(environment, "finalize"))
        environment.close(context)
        request = ScoreInput(task, [], MemoryFileStore().put_json([]), state=state)
        score = ProgressScorer({}).score(request, ScoringContext(
            time.monotonic() + 60, registry, lambda: False))
        self.assertEqual(environment.value, -1)
        self.assertEqual(score.reward, 1.0)
        self.assertEqual(request.state.value, 2)
        self.assertEqual(request.final_answer, [])
        self.assertEqual(set(to_wire(request)), {'task', 'final_answer', 'trajectory_ref', 'state'})
        # The public result schema cannot carry the private snapshot or the old wrapper.
        fields = SCHEMA['$defs']['EpisodeResult']['properties']
        self.assertNotIn('state', fields)
        self.assertNotIn('outcome', fields)

    def test_harness_is_a_worker_service_not_dataset_dispatch(self):
        for name in ("dscodebench", "swe_verified", "swe_lite", "swe_pro", "swe_smith"):
            manifest = load(GENERATED_ROOT / "packages" / name / "manifest.json")
            module, symbol = manifest["entrypoints"]["scorer"].split(":")
            scorer = getattr(importlib.import_module(module), symbol)({})
            private_data = component_private_data(name)
            with self.subTest(name=name), self.assertRaises(RuntimeError):
                score_candidate(
                    scorer,
                    "candidate",
                    private_data,
                    ScoringContext(deadline=time.monotonic() + 120, registry=REGISTRY, cancelled=lambda: False),
                )

    def test_patch_file_submission_uses_final_answer_through_harness(self):
        from uenv.sdk import evaluate_harness, read_reference_text
        store = MemoryFileStore()
        reference = store.put_json({'patch': 'example'})
        content = [text_part('Patch attached'), {'kind': 'artifact', 'artifact': reference}]
        request = ScoreInput(component_task('dscodebench'), content, store.put_json([]),
                             private_data=component_private_data('dscodebench'))
        wire = to_wire(request)
        # Worker wire TaskSpec also carries internal version/digest, omitted from the SDK view.
        wire['task'] = generated('dscodebench', 'task.json')
        validate('ScoreInput', wire)
        with self.assertRaises(ValidationError):
            validate('ScoreInput', dict(wire, artifacts=[]))
        with self.assertRaises(ValueError):
            read_reference_text(request)  # A text-only rule must not silently drop the file.
        def run_harness(command):
            self.assertEqual(command['final_answer'], content)
            self.assertNotIn('artifacts', command)
            with self.assertRaises(ValidationError):
                validate('HarnessRequest', dict(command, artifacts=[]))
            return {'status': 'ok', 'success': True, 'tests_run': 1, 'tests_passed': 1,
                    'report_ref': store.put_json({'success': True})}
        result = evaluate_harness(request, ScoringContext(
            time.monotonic() + 60, REGISTRY, lambda: False, run_harness=run_harness))
        self.assertEqual(result.reward, 1.0)
        self.assertEqual(request.final_answer, content)

    def test_harness_candidate_failure_is_valid_zero_reward(self):
        module = importlib.import_module("dscodebench.scorer")
        store = MemoryFileStore()
        report = {"status": "ok", "success": False, "tests_run": 1, "tests_passed": 0, "report_ref": store.put_json({"success": False})}
        private_data = component_private_data("dscodebench")
        def run_harness(request):
            self.assertEqual(set(request), {'final_answer', 'private_data', 'remaining_timeout_ms'})
            self.assertEqual(request['final_answer'], [text_part('candidate')])
            self.assertNotIn('artifacts', request)
            return report
        result = score_candidate(
            module.DscodebenchScorer({}),
            "candidate",
            private_data,
            ScoringContext(
                deadline=time.monotonic() + 120,
                registry=REGISTRY,
                cancelled=lambda: False,
                run_harness=run_harness,
            ),
        )
        self.assertFalse(result.success)
        self.assertEqual(result.reward, 0.0)


class SDKToolTests(unittest.TestCase):
    def test_python_and_native_tool_host(self):
        import os
        env = dict(os.environ)
        env['PYTHONPATH'] = os.pathsep.join([str(ROOT / 'reference/sdk/src'), str(ROOT / '.dependencies')])
        completed = subprocess.run([sys.executable, '-m', 'unittest', 'discover',
            '-s', str(ROOT / 'reference/sdk/tests'), '-v'], cwd=ROOT, env=env, text=True, capture_output=True)
        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)


class RustControlTests(unittest.TestCase):
    def test_rust_control_reference(self):
        import os
        env = dict(os.environ)
        env.setdefault('UENV_INTEGRATION_PYTHON', sys.executable)
        command = ["cargo", "test", "--offline", "--locked", "--manifest-path", str(ROOT / "Cargo.toml")]
        completed = subprocess.run(command, cwd=ROOT, env=env, text=True, capture_output=True)
        if completed.returncode:
            self.fail(completed.stdout + completed.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
