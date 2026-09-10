"""Generate stable JSON fixtures for docs and contract tests.

This module is development tooling. Production plan resolution is implemented by
the Rust `reference-control` crate and validated against every generated fixture.
"""
from copy import deepcopy
import hashlib

from uenv.sdk.schema_registry import canonical_bytes


def digest(value):
    return "sha256:" + hashlib.sha256(canonical_bytes(value)).hexdigest()


def _resolved(reference):
    result = deepcopy(reference)
    result.setdefault("digest", digest({"fixture_component": reference}))
    return result


def seal_fixture_plan(plan):
    result = deepcopy(plan)
    result.pop("plan_digest", None)
    result["plan_digest"] = digest(result)
    return result


def fixture_plan(episode, run, environment_manifest, registry, catalog):
    """Build a deterministic sample from registered component definitions.

    This development helper receives a manifest only to simulate Hub catalog
    registration. The production PlanResolver receives no manifest side input:
    RunSpec selects components and the catalog supplies their installed definitions.
    """
    registry.validate("EpisodeRequest", episode)
    registry.validate("RunSpec", run)
    registry.validate("PackageManifest", environment_manifest)
    if episode["run_id"] != run["run_id"]:
        raise ValueError("RUN_ID_MISMATCH")
    package_ref = {
        "id": environment_manifest["id"],
        "version": environment_manifest["version"],
    }
    for role in ("environment", "scorer"):
        selected = run[role]["implementation"]
        if any(selected.get(field) != package_ref[field] for field in package_ref):
            raise ValueError(f"{role.upper()}_PACKAGE_MISMATCH")
        if (run[role]["config"]["schema_ref"]
                != environment_manifest["config_schemas"][role]):
            raise ValueError("COMPONENT_CONFIG_SCHEMA_MISMATCH")
    if episode["task"]["input"]["schema_ref"] != environment_manifest["task_schema"]:
        raise ValueError("COMPONENT_TASK_SCHEMA_MISMATCH")
    if "private_data" in episode and (
            episode["private_data"]["schema_ref"]
            != environment_manifest.get("private_schema")):
        raise ValueError("SCORER_PRIVATE_SCHEMA_MISMATCH")

    plan = {key: deepcopy(episode[key]) for key in ("run_id", "episode_id", "task", "seed")}
    if "private_data" in episode:
        plan["private_data"] = deepcopy(episode["private_data"])
    for key in ("purpose", "model", "limits", "training"):
        if key in run:
            plan[key] = deepcopy(run[key])
    for role in ("environment", "agent", "scorer", "backend"):
        plan[role] = deepcopy(run[role])
        plan[role]["implementation"] = _resolved(run[role]["implementation"])

    required = set(environment_manifest["required_capabilities"])
    if environment_manifest["internet_access"]:
        required.add("internet_access.v1")
    plan["tools"] = []
    for binding in run["tools"]:
        component = catalog["components"][binding["implementation"]["id"]]
        if component["role"] != "tool":
            raise ValueError("TOOL_ROLE_MISMATCH")
        scope = component["execution_scope"]
        capabilities = component["required_capabilities"]
        required.update(capabilities)
        plan["tools"].append({
            "name": binding["name"],
            "implementation": _resolved(binding["implementation"]),
            "adapter": _resolved(component["adapter"]),
            "config": deepcopy(binding["config"]),
            "interface": component["interface"],
            "execution_scope": scope,
            "required_capabilities": capabilities,
        })

    backend = catalog["components"][run["backend"]["implementation"]["id"]]
    if backend["role"] != "backend":
        raise ValueError("BACKEND_ROLE_MISMATCH")
    is_container = backend["consumes_image"]
    if is_container:
        candidates = (("run", run), ("task", episode["task"]),
                      ("package", environment_manifest))
        choice = next(((source, value["runtime"]["image"]) for source, value in candidates if "runtime" in value), None)
        if choice is None:
            raise ValueError("MISSING_IMAGE")
        source, image = choice
        plan["runtime"] = {"image": deepcopy(image), "image_source": source}
    elif "runtime" in run:
        raise ValueError("PROCESS_IMAGE_CONFLICT")

    plan.update(
        attempt_id=1,
        required_capabilities=sorted(required),
        internet_access=environment_manifest["internet_access"],
        deadline_at_ms=1_800_000_000_000 + run["limits"]["total_timeout_ms"],
    )
    plan = seal_fixture_plan(plan)
    registry.validate("ExecutionPlan", plan)
    return plan
