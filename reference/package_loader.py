"""Generic loader for reference dataset packages and public run files."""
from __future__ import annotations

from copy import deepcopy
import hashlib
import importlib
import json
from pathlib import Path
import sys
import tomllib

import yaml

from uenv.sdk import UEnvModel, bind_schema, model_json_schema
from uenv.sdk.schema_registry import SchemaRegistry


ALLOWED_DECLARATION_FIELDS = {
    "id", "version", "entrypoints", "models",
    "internet_access", "required_capabilities", "runtime",
}
MODEL_ROLES = {"input", "private_data", "environment_config", "scorer_config", "action", "observation", "state"}
RESERVED_MODEL_FIELDS = {
    "episode_id", "attempt_id", "lease", "schema_ref", "plan_digest",
    "input_digest", "run_id", "agent", "backend", "model", "tools",
    "scorer", "limits", "purpose", "timeout",
}
SUSPECT_MODEL_FIELDS = {"allowed_time": "RunSpec.limits.total_timeout_ms"}


def load_yaml(path: Path) -> dict:
    value = yaml.safe_load(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"Expected YAML object: {path}")
    return value


def load_symbol(reference: str):
    module_name, symbol_name = reference.split(":", 1)
    return getattr(importlib.import_module(module_name), symbol_name)


def discover_packages(root: Path) -> list[Path]:
    return sorted(path.parent for path in root.glob("*/dataset.yaml"))


def package_schema_id(declaration: dict, model: type[UEnvModel]) -> str:
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
    warnings = []
    for role in ("input", "private_data"):
        model = package.get(role + "_model")
        if model is None:
            continue
        def check_fields(schema: dict, path: str) -> None:
            for name, child in schema.get("properties", {}).items():
                if name in RESERVED_MODEL_FIELDS:
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
    if "private_data" in schema_ids:
        manifest["private_schema"] = schema_ids["private_data"]
    if "runtime" in declaration:
        manifest["runtime"] = deepcopy(declaration["runtime"])
    (output / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    return manifest


def expand_run(public_run: dict, manifest: dict, catalog: dict, registry=None) -> dict:
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

    for role in ("environment", "scorer"):
        if role == "scorer" and role not in run:
            continue
        if role not in manifest["entrypoints"]:
            raise ValueError(f"Package does not provide {role}")
        selected = run[role]["implementation"]
        if any(selected.get(key) != manifest[key] for key in ("id", "version")):
            raise ValueError(f"No registered manifest for selected {role}")
        run[role]["config"] = config_envelope(
            manifest["config_schemas"][role], run[role].get("config", {}))
    for role in ("agent", "backend"):
        component_id = run[role]["implementation"]["id"]
        entry = catalog["components"].get(component_id)
        if not entry or entry["role"] != role:
            raise ValueError(f"Unknown {role} component: {component_id}")
        run[role]["config"] = config_envelope(entry["config_schema"], run[role].get("config", {}))
    for tool in run["tools"]:
        component_id = tool["implementation"]["id"]
        entry = catalog["components"].get(component_id)
        if not entry or entry["role"] != "tool":
            raise ValueError(f"Unknown tool component: {component_id}")
        tool["config"] = config_envelope(entry["config_schema"], tool.get("config", {}))
    registry.validate("RunSpec", run)
    return run
