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
from uenv.sdk import AgentContext, DatasetAdapter, Environment, EnvironmentContext, Outcome, ScoreInput, ScoreResult, Scorer, ScoringContext, ToolExecutor, UEnvModel, model_from_envelope, model_json_schema, task_from_wire, text_part, to_wire
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


class MemoryArtifacts:
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
        Outcome([text_part(answer)], termination_reason="final_answer"),
        MemoryArtifacts().put_json([]),
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
                self.assertEqual(expand_run(public_run, manifest, CATALOG), run)
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
                    MemoryArtifacts(),
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
            expand_run(public_run, manifest, CATALOG)
        public_run.pop("schema_version")
        public_run["scorer"]["implementation"]["version"] = "2.0.0"
        with self.assertRaisesRegex(ValueError, "No registered manifest"):
            expand_run(public_run, manifest, CATALOG)

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
        from uenv.sdk import Observation

        class CustomConfig(UEnvModel):
            prefix: str = "Question: "

        class CustomAction(UEnvModel):
            increment: int

        class ConfiguredEnvironment(Environment):
            def reset(self, task, context):
                return Observation([text_part(self.config["prefix"] + task.input.instruction)])

        package = {**PACKAGES["gsm8k"], "declaration": copy.deepcopy(PACKAGES["gsm8k"]["declaration"])}
        package["declaration"]["models"].update(environment_config="test:CustomConfig", action="test:CustomAction")
        package.update(environment_config_model=CustomConfig, action_model=CustomAction)
        with tempfile.TemporaryDirectory(prefix="uenv-contract-") as folder:
            output = Path(folder) / "generated/packages/gsm8k"
            manifest = build_manifest(package, output)
            registry = SchemaRegistry.bundled()
            for path in (output / "schemas").glob("*.json"):
                registry.register(load(path))
            registry.validate("PackageManifest", manifest)
            public_run = load_yaml(ROOT / "reference/runs/gsm8k.yaml")
            public_run["environment"]["config"] = {"prefix": "Question: "}
            run = expand_run(public_run, manifest, CATALOG, registry)
            public_run["environment"].pop("config")
            self.assertEqual(expand_run(public_run, manifest, CATALOG, registry), run)
            registry.validate("RunSpec", run)
            environment = ConfiguredEnvironment(run["environment"]["config"]["data"])
            observation = environment.reset(component_task("gsm8k"), None)
            self.assertEqual(observation.content[0]["text"], "Question: " + component_task("gsm8k").input.instruction)
            registry.validate("Observation", to_wire(observation))
            self.assertTrue((output / "schemas/CustomAction.schema.json").exists())
            run["environment"]["config"]["data"]["prefix"] = 42
            with self.assertRaises(ValidationError):
                registry.validate("RunSpec", run)

    def test_public_run_defaults_preserve_explicit_values_and_input(self):
        for name in PACKAGES:
            with self.subTest(dataset=name):
                manifest = load(GENERATED_ROOT / "packages" / name / "manifest.json")
                public = load_yaml(ROOT / "reference/runs" / f"{name}.yaml")
                before = copy.deepcopy(public)
                expanded = expand_run(public, manifest, CATALOG)
                self.assertEqual(public, before)
                explicit = copy.deepcopy(expanded)
                explicit.pop("schema_version")
                for role in ("environment", "agent", "scorer", "backend"):
                    explicit[role]["config"] = explicit[role]["config"]["data"]
                for tool in explicit["tools"]:
                    tool["config"] = tool["config"]["data"]
                self.assertEqual(expand_run(explicit, manifest, CATALOG), expanded)
                explicit["model"]["generation"]["temperature"] = 0.5
                explicit["model"]["max_transport_retries"] = 0
                explicit["limits"]["max_tool_calls"] = 0
                explicit["retry"]["max_attempts"] = 2
                changed = expand_run(explicit, manifest, CATALOG)
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
            changed[role]["config"] = config
            with self.subTest(role=role, config=config), self.assertRaises(ValidationError):
                expand_run(changed, manifest, CATALOG)
        for value in (None, False, -1, "0.5"):
            changed = copy.deepcopy(baseline)
            changed["model"]["generation"] = {"temperature": value}
            with self.subTest(temperature=value), self.assertRaises(ValidationError):
                expand_run(changed, manifest, CATALOG)

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
        self.assertEqual(set(defs["ScoreResult"]["properties"]), {"status", "success", "metrics", "reward", "evidence", "scorer", "error"})
        self.assertEqual(set(defs["EnvironmentTransition"]["properties"]), {"environment_step_index", "observation_before", "action", "transition"})
        self.assertEqual(set(defs["ToolBinding"]["properties"]), {"name", "implementation", "config"})
        self.assertIn("adapter", defs["ResolvedToolBinding"]["properties"])
        self.assertIn("interface", defs["ResolvedToolBinding"]["properties"])
        self.assertIn("termination_reason", defs["Outcome"]["properties"])
        self.assertEqual(
            defs["Outcome"]["properties"]["termination_reason"]["enum"],
            ["in_progress", "final_answer", "environment_terminal", "budget_exhausted"],
        )
        self.assertNotIn("termination_reason", defs["EpisodeResult"]["properties"])
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

    def test_agent_and_tool_publish_common_interfaces(self):
        defs = SCHEMA["$defs"]
        component = {"id": "agents/example", "version": "1.0.0"}
        adapter = {"id": "adapters/python-mcp", "version": "1.0.0"}
        artifact = MemoryArtifacts().put_json({"type": "object"})
        validate("AgentManifest", {
            "implementation": component,
            "entrypoint": "example.agent:ExampleAgent",
            "config_schema": "uenv://schemas/vnext/EmptyConfig",
            "supported_interfaces": ["mcp.v1"],
            "required_tool_names": [],
        })
        validate("ToolSpec", {
            "implementation": {"id": "tools/example", "version": "1.0.0"},
            "entrypoint": "example.tool:execute",
            "config_schema": "uenv://schemas/vnext/EmptyConfig",
            "description": "Example tool",
            "interfaces": [{
                "interface": "mcp.v1",
                "adapter": adapter,
                "execution_scope": "sandbox",
                "required_capabilities": [],
            }],
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

    def test_harness_candidate_failure_is_valid_zero_reward(self):
        module = importlib.import_module("dscodebench.scorer")
        store = MemoryArtifacts()
        report = {"status": "ok", "success": False, "tests_run": 1, "tests_passed": 0, "report_ref": store.put_json({"success": False})}
        private_data = component_private_data("dscodebench")
        result = score_candidate(
            module.DscodebenchScorer({}),
            "candidate",
            private_data,
            ScoringContext(
                deadline=time.monotonic() + 120,
                registry=REGISTRY,
                cancelled=lambda: False,
                run_harness=lambda _: report,
            ),
        )
        self.assertFalse(result.success)
        self.assertEqual(result.reward, 0.0)


class RustControlTests(unittest.TestCase):
    def test_rust_control_reference(self):
        command = ["cargo", "test", "--offline", "--locked", "--manifest-path", str(ROOT / "Cargo.toml")]
        completed = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
        if completed.returncode:
            self.fail(completed.stdout + completed.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
