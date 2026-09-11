"""Generic loader for reference dataset packages and public run files."""
from __future__ import annotations

from copy import deepcopy
import hashlib
import importlib
import inspect
import json
from pathlib import Path
import sys
import tomllib
from typing import get_type_hints

import yaml

from uenv.sdk import DatasetAdapter, Environment, Scorer, UEnvModel, bind_schema, model_json_schema
from uenv.sdk.schema_registry import SchemaRegistry


ALLOWED_DECLARATION_FIELDS = {
    "id", "version", "entrypoints", "models",
    "internet_access", "required_capabilities", "runtime",
}
MODEL_ROLES = {"input", "private_data", "environment_config", "scorer_config", "observation", "state"}
RESERVED_MODEL_FIELDS = {
    "episode_id", "attempt_id", "lease", "schema_ref", "plan_digest",
    "input_digest", "run_id", "agent", "backend", "model", "tools",
    "scorer", "dataset_package", "scoring", "limits", "purpose", "timeout",
}
SUSPECT_MODEL_FIELDS = {"allowed_time": "RunSpec.limits.total_timeout_ms"}


class _UniqueKeyLoader(yaml.SafeLoader):
    def construct_mapping(self, node, deep=False):
        self.flatten_mapping(node)
        keys = set()
        for key_node, _ in node.value:
            key = self.construct_object(key_node, deep=deep)
            if key in keys:
                raise ValueError(f"Duplicate YAML key {key!r} at line {key_node.start_mark.line + 1}")
            keys.add(key)
        return super().construct_mapping(node, deep=deep)


def load_yaml(path: Path) -> dict:
    value = yaml.load(path.read_text(encoding="utf-8"), Loader=_UniqueKeyLoader)
    if not isinstance(value, dict):
        raise ValueError(f"Expected YAML object: {path}")
    return value


def load_symbol(reference: str):
    module_name, symbol_name = reference.split(":", 1)
    return getattr(importlib.import_module(module_name), symbol_name)


def discover_packages(root: Path) -> list[Path]:
    return sorted(path.parent for path in root.glob("*/dataset.yaml"))


def package_schema_id(declaration: dict, model: type[UEnvModel]) -> str:
    existing = model.__dict__.get('__schema_id__')
    if existing and existing.startswith('uenv://schemas/vnext/'):
        return existing
    return f"uenv://packages/{declaration['id']}/{declaration['version']}/{model.__name__}"


def load_package(package_dir: Path) -> dict:
    declaration = load_yaml(package_dir / "dataset.yaml")
    validate_declaration(declaration)
    project = tomllib.loads((package_dir / "pyproject.toml").read_text(encoding="utf-8"))["project"]
    if project.get("version") != declaration["version"]:
        raise ValueError("pyproject.toml project.version must match dataset.yaml version")
    source = package_dir / "src"
    if str(source) not in sys.path:
        sys.path.insert(0, str(source))
    result = {"directory": package_dir, "declaration": declaration}
    result["adapter"] = load_symbol(declaration["entrypoints"]["dataset_adapter"])
    result["environment"] = load_symbol(declaration["entrypoints"]["environment"])
    if "scorer" in declaration["entrypoints"]:
        result["scorer"] = load_symbol(declaration["entrypoints"]["scorer"])
    if "input" not in declaration["models"]:
        raise ValueError("dataset.yaml models.input is required")
    for role, reference in declaration["models"].items():
        model = load_symbol(reference)
        if not isinstance(model, type) or not issubclass(model, UEnvModel):
            raise TypeError(f"{reference} must inherit UEnvModel")
        bind_schema(model, package_schema_id(declaration, model))
        result[role + "_model"] = model
    validate_author_package(result)
    # tools.py is discovered from the existing package root, not another YAML
    # list which authors would have to keep in sync with their functions.
    module_name = result['environment'].__module__.rsplit('.', 1)[0] + '.tools'
    tool_file = source / Path(*module_name.split('.')).with_suffix('.py')
    result['tools'] = {}
    if tool_file.is_file():
        from uenv.sdk.tools import ToolDefinition
        module = importlib.import_module(module_name)
        result['tools'] = {name: value for name, value in vars(module).items()
                           if isinstance(value, ToolDefinition)
                           and value.entrypoint == f'{module_name}:{name}'}
    for entry, key in (("dataset_adapter", "adapter"), ("environment", "environment"), ("scorer", "scorer")):
        if key in result:
            module, name = declaration["entrypoints"][entry].split(":")
            cls = result[key]
            if cls.__module__ != module or cls.__name__ != name:
                raise ValueError(f"{entry} must reference a class declared in its own module, not an alias")
    return result


def validate_declaration(declaration: dict) -> None:
    unknown = set(declaration) - ALLOWED_DECLARATION_FIELDS
    if unknown:
        raise ValueError(f"Unknown dataset.yaml fields: {sorted(unknown)}")
    for field in ("id", "version"):
        if not isinstance(declaration.get(field), str) or not declaration[field]:
            raise ValueError(f"dataset.yaml {field} must be a non-empty string")
    entries = declaration.get("entrypoints", {})
    if (not isinstance(entries, dict)
            or not {"dataset_adapter", "environment"} <= set(entries)
            or set(entries) - {"dataset_adapter", "environment", "scorer"}
            or any(not isinstance(value, str) or len(value.split(":")) != 2
                   or not all(value.split(":")) for value in entries.values())):
        raise ValueError("entrypoints require dataset_adapter and environment; scorer is optional; use module:Class")
    if type(declaration.get("internet_access", False)) is not bool:
        raise ValueError("internet_access must be a boolean")
    capabilities = declaration.get("required_capabilities", [])
    if (not isinstance(capabilities, list)
            or any(not isinstance(item, str) or not item for item in capabilities)
            or len(set(capabilities)) != len(capabilities)):
        raise ValueError("required_capabilities must be unique non-empty strings")
    models = declaration.get("models", {})
    if (not isinstance(models, dict) or "input" not in models or set(models) - MODEL_ROLES
            or any(not isinstance(value, str) or not value for value in models.values())):
        raise ValueError("Unknown dataset.yaml model roles")
    if "scorer_config" in models and "scorer" not in entries:
        raise ValueError("scorer_config requires a scorer entrypoint")


def validate_author_package(package: dict) -> list[str]:
    """Return semantic warnings; deterministic contract conflicts are errors."""
    declaration = package["declaration"]
    validate_declaration(declaration)
    for role, base in (("adapter", DatasetAdapter), ("environment", Environment), ("scorer", Scorer)):
        cls = package.get(role)
        if cls is not None and (not isinstance(cls, type) or cls.__bases__ != (base,) or inspect.isabstract(cls)):
            raise TypeError(f"{role} must be a concrete direct subclass of {base.__name__}")
    environment = package.get('environment')
    if environment is not None and any(name in environment.__dict__ for name in ('step', 'parse_action')):
        raise ValueError('Define operations in tools.py; Environment has no step or parser')
    warnings = []
    core = SchemaRegistry().core["$defs"]
    # Read canonical control names from the contract, not a second limits table.
    for role in sorted(MODEL_ROLES):
        control_fields = RESERVED_MODEL_FIELDS | set(core["Limits"]["properties"])
        if role.endswith('_config'):
            control_fields |= set(core["GenerationConfig"]["properties"])
        model = package.get(role + "_model")
        if model is None:
            continue
        if package_schema_id(declaration, model).startswith('uenv://schemas/vnext/'):
            continue
        def check_fields(schema: dict, path: str) -> None:
            for name, child in schema.get("properties", {}).items():
                if name in control_fields:
                    raise ValueError(f"{path}.{name} shadows system fields")
                if name in SUSPECT_MODEL_FIELDS:
                    warnings.append(
                        f"{path}.{name} may duplicate {SUSPECT_MODEL_FIELDS[name]}; "
                        "automatic validation cannot determine semantic equivalence"
                    )
                check_fields(child, f"{path}.{name}")
            for keyword in ("items", "additionalProperties"):
                if isinstance(schema.get(keyword), dict):
                    check_fields(schema[keyword], path + "[]")
            for child in schema.get("anyOf", []):
                check_fields(child, path)

        check_fields(model_json_schema(model, package_schema_id(declaration, model)), model.__name__)
    return warnings


def build_manifest(package: dict, output: Path) -> dict:
    validate_author_package(package)
    declaration = package["declaration"]
    schema_dir = output / "schemas"
    schema_dir.mkdir(parents=True, exist_ok=True)
    artifacts = []
    schema_ids = {}
    roles = [role for role in package["declaration"]["models"] if role + "_model" in package]
    for role in roles:
        model = package[role + "_model"]
        schema_id = package_schema_id(declaration, model)
        if schema_id.startswith('uenv://schemas/vnext/'):
            # Shared models retain the core definition; do not emit a package copy.
            schema_ids[role] = schema_id
            continue
        if schema_id in schema_ids.values():
            schema_ids[role] = schema_id
            continue
        schema = model_json_schema(model, schema_id)
        path = schema_dir / f"{model.__name__}.schema.json"
        content = (json.dumps(schema, ensure_ascii=False, indent=2) + "\n").encode()
        path.write_bytes(content)
        artifacts.append({
            "uri": path.relative_to(output.parents[2]).as_posix(),
            "digest": "sha256:" + hashlib.sha256(content).hexdigest(),
            "size_bytes": len(content),
            "media_type": "application/schema+json",
        })
        schema_ids[role] = schema_id
    manifest = {
        "id": declaration["id"],
        "version": declaration["version"],
        "entrypoints": deepcopy(declaration["entrypoints"]),
        "task_schema": schema_ids["input"],
        "config_schemas": {
            role: schema_ids.get(role + "_config", "uenv://schemas/vnext/EmptyConfig")
            for role in ("environment", "scorer") if role in declaration["entrypoints"]
        },
        "internet_access": declaration.get("internet_access", False),
        "required_capabilities": deepcopy(declaration.get("required_capabilities", [])),
        "artifacts": [],
        "provided_tools": [],
        "schemas": artifacts,
    }
    for name, definition in package.get('tools', {}).items():
        tool_schema = deepcopy(definition.input_schema)
        tool_schema['$id'] = f"uenv://packages/{declaration['id']}/{declaration['version']}/tools/{name}/input"
        path = schema_dir / f"{name}.input.schema.json"
        content = (json.dumps(tool_schema, ensure_ascii=False, indent=2) + '\n').encode()
        path.write_bytes(content)
        reference = {
            'uri': path.relative_to(output.parents[2]).as_posix(),
            'digest': 'sha256:' + hashlib.sha256(content).hexdigest(),
            'size_bytes': len(content), 'media_type': 'application/schema+json',
        }
        artifacts.append(reference)
        manifest['provided_tools'].append({
            'implementation': {'id': f"{declaration['id']}/tools/{name}", 'version': declaration['version']},
            'entrypoint': definition.entrypoint,
            'config_schema': 'uenv://schemas/vnext/EmptyConfig',
            'description': definition.description, 'input_schema': reference,
            'execution_scope': definition.execution_scope,
            'required_capabilities': list(definition.required_capabilities),
            'side_effect': definition.side_effect,
        })
    if "private_data" in schema_ids:
        manifest["private_schema"] = schema_ids["private_data"]
    if "runtime" in declaration:
        manifest["runtime"] = deepcopy(declaration["runtime"])
    (output / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    return manifest


def select_manifest(manifests: list[dict], selected: dict, role: str) -> dict:
    matches = [item for item in manifests
               if all(selected.get(key) == item[key] for key in ("id", "version"))
               and role in item["entrypoints"]]
    if len(matches) != 1:
        raise ValueError(f"Expected one registered manifest for selected {role}")
    return matches[0]


def expand_run(public_run: dict, manifests: list[dict], catalog: dict, registry=None) -> dict:
    """Shared submission example for YAML-loaded and SDK-provided mappings.

    Fill schema defaults once, then validate the complete wire RunSpec.
    The production Bridge/client transport is not implemented by this helper.
    """
    if "schema_version" in public_run:
        raise ValueError("schema_version is generated by the submission boundary")
    registry = registry or SchemaRegistry.bundled()
    run = registry.apply_defaults("RunSpec", public_run)
    run["schema_version"] = "vnext.3"

    def config_envelope(schema_ref, value):
        data = registry.apply_defaults(schema_ref, value)
        registry.validate(schema_ref, data)
        return {"schema_ref": schema_ref, "data": data}

    if "scorer" in run:
        raise ValueError("Select dataset_package once; scorer selection is forbidden")
    manifest = select_manifest(manifests, run["dataset_package"], "environment")
    run["environment"] = config_envelope(manifest["config_schemas"]["environment"], run.get("environment", {}))
    scoring = run.get("scoring")
    if not isinstance(scoring, dict) or type(scoring.get("enabled")) is not bool:
        raise ValueError("scoring.enabled must be an explicit boolean")
    if set(scoring) - {"enabled", "config"}:
        raise ValueError("scoring only accepts enabled and config; scorer selection is forbidden")
    if scoring["enabled"]:
        select_manifest(manifests, run["dataset_package"], "scorer")
        scoring["config"] = config_envelope(manifest["config_schemas"]["scorer"], scoring.get("config", {}))
    elif "config" in scoring:
        raise ValueError("scoring.config is forbidden when scoring is disabled")
    for role in ("agent", "backend"):
        component_id = run[role]["implementation"]["id"]
        entry = catalog["components"].get(component_id)
        if (not entry or entry["role"] != role
                or entry["version"] != run[role]["implementation"]["version"]):
            raise ValueError(f"Unknown {role} component: {component_id}")
        run[role]["config"] = config_envelope(entry["config_schema"], run[role].get("config", {}))
    names = set()
    for tool in run["tools"]:
        if tool["name"] in names:
            raise ValueError("DUPLICATE_TOOL_NAME")
        names.add(tool["name"])
        entry = lookup_tool(catalog, tool["implementation"], run["agent"]["implementation"], manifests)
        tool["config"] = config_envelope(entry["config_schema"], tool.get("config", {}))
    registry.validate("RunSpec", run)
    return run


def lookup_tool(catalog, reference, agent, manifests=()):
    """Resolve standalone tools or generated Agent exports; do not auto-enable tools."""
    entries = catalog["components"]
    entry = entries.get(reference["id"])
    if entry and entry["role"] == "tool" and entry["version"] == reference["version"]:
        return deepcopy(entry)
    matches = [item for manifest in manifests for item in manifest.get('provided_tools', [])
               if all(item['implementation'][key] == reference[key] for key in ('id', 'version'))]
    if len(matches) > 1:
        raise ValueError('CONFLICTING_TOOL_EXPORT')
    if matches:
        return deepcopy(matches[0])
    for owner_id, owner in entries.items():
        if owner.get("role") != "agent":
            continue
        for export, definition in owner.get("provided_tools", {}).items():
            if reference["id"] != f"{owner_id}/tools/{export}":
                continue
            owner_ref = {"id": owner_id, "version": owner["version"]}
            if owner_ref != {k: agent[k] for k in ("id", "version")}:
                raise ValueError("NATIVE_TOOL_AGENT_MISMATCH")
            if reference["version"] != owner["version"]:
                raise ValueError("NATIVE_TOOL_VERSION_MISMATCH")
            return {**deepcopy(definition), "native_agent": owner_ref}
    raise ValueError(f"Unknown tool component: {reference['id']}")
