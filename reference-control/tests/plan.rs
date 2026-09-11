use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use uenv_reference_control::contracts::ContractSchema;
use uenv_reference_control::plan::{
    AgentToolProfile, ComponentCatalog, ComponentMetadata, PlanResolver, RuntimeKind, ToolMetadata,
    retry_execution_plan, seal_plan, validate_batch_submission, validate_execution_plan,
};

fn design_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn read(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn strings(value: &Value, field: &str) -> BTreeSet<String> {
    value[field]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_owned())
        .collect()
}

fn insert_component(
    catalog: &mut ComponentCatalog,
    component: Value,
    role: &str,
    config_schema: &str,
    task_schema: &str,
    runtime_kind: Option<RuntimeKind>,
    tool: Option<ToolMetadata>,
) {
    catalog
        .insert(ComponentMetadata {
            component,
            roles: BTreeSet::from([role.to_owned()]),
            role_config_schemas: [(role.to_owned(), config_schema.to_owned())]
                .into_iter()
                .collect(),
            required_capabilities: BTreeSet::new(),
            task_schemas: BTreeSet::from([task_schema.to_owned()]),
            private_schemas: BTreeSet::new(),
            native_profiles: BTreeSet::from(["example-only".to_owned()]),
            tool,
            default_runtime: None,
            requires_internet_access: false,
            runtime_kind,
            supports_internet_access: true,
        })
        .unwrap();
}

#[test]
fn catalog_rejects_a_conflicting_requested_digest() {
    let mut catalog = ComponentCatalog::default();
    let component = json!({
        "id": "agents/example",
        "version": "1.0.0",
        "digest": format!("sha256:{}", "0".repeat(64)),
    });
    insert_component(
        &mut catalog,
        component,
        "agent",
        "uenv://schemas/vnext/EmptyConfig",
        "uenv://schemas/vnext/InstructionInput",
        None,
        None,
    );
    let conflicting = json!({
        "id": "agents/example",
        "version": "1.0.0",
        "digest": format!("sha256:{}", "1".repeat(64)),
    });
    assert_eq!(
        catalog.get(&conflicting).unwrap_err().code,
        "COMPONENT_DIGEST_MISMATCH"
    );
}

#[test]
fn native_tools_must_match_selected_agent_and_owner_version() {
    let schema = ContractSchema::bundled();
    let plan =
        read(design_root().join("reference/generated/episodes/swe_verified/execution_plan.json"));
    validate_execution_plan(&plan, &schema).unwrap();
    let mut wrong_agent = plan.clone();
    wrong_agent["agent"]["implementation"]["id"] = json!("agents/plain");
    assert_eq!(
        validate_execution_plan(&seal_plan(&wrong_agent).unwrap(), &schema)
            .unwrap_err()
            .code,
        "NATIVE_TOOL_AGENT_MISMATCH"
    );
    let mut wrong_version = plan.clone();
    wrong_version["tools"][0]["implementation"]["version"] = json!("other");
    assert_eq!(
        validate_execution_plan(&seal_plan(&wrong_version).unwrap(), &schema)
            .unwrap_err()
            .code,
        "NATIVE_TOOL_VERSION_MISMATCH"
    );
    let mut old_interface = plan;
    old_interface["tools"][0]["interface"] = json!("mcp.v1");
    assert_eq!(
        validate_execution_plan(&seal_plan(&old_interface).unwrap(), &schema)
            .unwrap_err()
            .code,
        "UNKNOWN_FIELD:ResolvedToolBinding.interface"
    );
}

#[test]
fn purpose_accepts_only_its_matching_training_configuration() {
    let schema = ContractSchema::bundled();
    let path = design_root().join("reference/generated/episodes/gsm8k/execution_plan.json");
    let mut evaluation = read(path);
    evaluation.as_object_mut().unwrap().insert(
        "training".to_owned(),
        json!({
            "parallel_mode": "sync",
            "require_token_trace": false,
            "version_policy": "per_generation",
            "requested_policy_version": "",
            "max_policy_lag": 0,
        }),
    );
    let evaluation = seal_plan(&evaluation).unwrap();
    assert_eq!(
        validate_execution_plan(&evaluation, &schema)
            .unwrap_err()
            .code,
        "UNEXPECTED_TRAINING_CONFIGURATION"
    );

    let mut training = evaluation.as_object().unwrap().clone();
    training.insert("purpose".to_owned(), Value::String("training".to_owned()));
    training.remove("training");
    let training = seal_plan(&Value::Object(training)).unwrap();
    assert_eq!(
        validate_execution_plan(&training, &schema)
            .unwrap_err()
            .code,
        "MISSING_TRAINING_CONFIGURATION"
    );
}

#[test]
fn nine_plans_are_rebuilt_by_one_rust_resolver() {
    let schema = ContractSchema::bundled();
    let generated = design_root().join("reference/generated");
    let mut packages: Vec<PathBuf> = fs::read_dir(generated.join("packages"))
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            path.join("manifest.json").exists().then_some(path)
        })
        .collect();
    packages.sort();
    assert_eq!(packages.len(), 9);

    for folder in packages {
        let name = folder.file_name().unwrap();
        let episode_folder = generated.join("episodes").join(name);
        let batch = read(episode_folder.join("batch_request.json"));
        validate_batch_submission(&batch, None, &schema).unwrap();
        validate_batch_submission(&batch, Some(&batch["run_spec"]), &schema).unwrap();
        let run = &batch["run_spec"];
        let episode = &batch["episodes"][0];
        assert_eq!(*run, read(episode_folder.join("run_spec.json")));
        assert_eq!(*episode, read(episode_folder.join("episode_request.json")));
        let package = read(folder.join("manifest.json"));
        let expected = read(episode_folder.join("execution_plan.json"));
        let task_schema = package["task_schema"].as_str().unwrap();
        let mut catalog = ComponentCatalog::default();

        let private_schemas = package
            .get("private_schema")
            .and_then(Value::as_str)
            .into_iter()
            .map(str::to_owned)
            .collect();
        let role_config_schemas = package["config_schemas"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(role, schema)| (role.clone(), schema.as_str().unwrap().to_owned()))
            .collect();
        catalog
            .insert(ComponentMetadata {
                component: expected["dataset_package"].clone(),
                roles: BTreeSet::from(["environment".to_owned(), "scorer".to_owned()]),
                role_config_schemas,
                required_capabilities: strings(&package, "required_capabilities"),
                task_schemas: BTreeSet::from([task_schema.to_owned()]),
                private_schemas,
                native_profiles: BTreeSet::from(["example-only".to_owned()]),
                tool: None,
                default_runtime: package.get("runtime").cloned(),
                requires_internet_access: package["internet_access"].as_bool().unwrap(),
                runtime_kind: None,
                supports_internet_access: false,
            })
            .unwrap();
        insert_component(
            &mut catalog,
            expected["agent"]["implementation"].clone(),
            "agent",
            run["agent"]["config"]["schema_ref"].as_str().unwrap(),
            task_schema,
            None,
            None,
        );
        let runtime_kind = if run
            .pointer("/backend/config/schema_ref")
            .unwrap()
            .as_str()
            .unwrap()
            .ends_with("/ContainerBackendConfig")
        {
            RuntimeKind::Container
        } else {
            RuntimeKind::Process
        };
        insert_component(
            &mut catalog,
            expected["backend"]["implementation"].clone(),
            "backend",
            run["backend"]["config"]["schema_ref"].as_str().unwrap(),
            task_schema,
            Some(runtime_kind),
            None,
        );

        for tool in expected["tools"].as_array().unwrap() {
            insert_component(
                &mut catalog,
                tool["implementation"].clone(),
                "tool",
                tool["config"]["schema_ref"].as_str().unwrap(),
                task_schema,
                None,
                Some(ToolMetadata {
                    native_agent: tool.get("native_agent").cloned(),
                    execution_scope: tool["execution_scope"].as_str().unwrap().to_owned(),
                    required_capabilities: strings(tool, "required_capabilities"),
                }),
            );
        }
        if let Some(harness) = expected.pointer("/private_data/data/evaluation_plan/harness") {
            insert_component(
                &mut catalog,
                harness.clone(),
                "harness",
                "uenv://schemas/vnext/EmptyConfig",
                task_schema,
                None,
                None,
            );
        }
        let required_names = if expected["tools"].as_array().unwrap().is_empty() {
            BTreeSet::new()
        } else {
            BTreeSet::from(["finish".to_owned()])
        };
        let profile = AgentToolProfile {
            agent: expected["agent"]["implementation"].clone(),
            required_names,
        };
        let resolver = PlanResolver {
            schema: &schema,
            catalog: &catalog,
            agent_profile: &profile,
        };
        let rebuilt = resolver
            .resolve(episode, run, 1_800_000_000_000, &|image, _, _| {
                Ok(image.clone())
            })
            .unwrap_or_else(|error| panic!("{}: {}", folder.display(), error.code));
        assert_eq!(rebuilt, expected, "{}", folder.display());
        validate_execution_plan(&rebuilt, &schema).unwrap();
        // Unscored collection must not resolve any private harness, even when
        // a reused source sample contains a reference to an unavailable one.
        let mut collection = run.clone();
        collection["purpose"] = json!("trajectory_collection");
        collection["run_id"] = json!("collection-test");
        collection["scoring"] = json!({"enabled": false});
        let mut source = episode.clone();
        source["private_data"] = json!({"schema_ref": "unavailable-private-schema", "data": {
            "evaluation_plan": {"harness": {"id": "unavailable/harness", "version": "1"}}
        }});
        let unscored = resolver
            .resolve(&source, &collection, 1_800_000_000_000, &|image, _, _| {
                Ok(image.clone())
            })
            .unwrap();
        assert_eq!(unscored["scoring"]["enabled"], false);
        assert!(unscored.get("private_data").is_none());
        validate_execution_plan(&unscored, &schema).unwrap();
        let mut wrong = unscored.clone();
        wrong["purpose"] = json!("evaluation");
        assert_eq!(
            validate_execution_plan(&seal_plan(&wrong).unwrap(), &schema)
                .unwrap_err()
                .code,
            "SCORING_REQUIRED"
        );
        wrong["purpose"] = json!("trajectory_collection");
        wrong["training"] = json!({});
        assert_eq!(
            validate_execution_plan(&seal_plan(&wrong).unwrap(), &schema)
                .unwrap_err()
                .code,
            "UNEXPECTED_TRAINING_CONFIGURATION"
        );
        let mut multi = batch.clone();
        let mut second = episode.clone();
        second["request_id"] = json!("second-request");
        second["episode_id"] = json!("second-episode");
        second["sample_index"] = json!(1);
        multi["episodes"].as_array_mut().unwrap().push(second);
        validate_batch_submission(&multi, Some(run), &schema).unwrap();
        let second_plan = resolver
            .resolve(
                &multi["episodes"][1],
                &multi["run_spec"],
                1_800_000_000_000,
                &|image, _, _| Ok(image.clone()),
            )
            .unwrap();
        let mut expected_second = expected.clone();
        expected_second["episode_id"] = json!("second-episode");
        assert_eq!(second_plan, seal_plan(&expected_second).unwrap());
    }
}

#[test]
fn batch_submission_checks_config_reuse_and_member_identity() {
    let schema = ContractSchema::bundled();
    let batch = read(design_root().join("reference/generated/episodes/gsm8k/batch_request.json"));
    let stored = batch["run_spec"].clone();
    let mut changed = batch.clone();
    changed["run_spec"]["limits"]["max_generations"] = json!(2);
    assert_eq!(
        validate_batch_submission(&changed, Some(&stored), &schema)
            .unwrap_err()
            .code,
        "RUN_CONFIG_CONFLICT"
    );
    assert_eq!(stored, batch["run_spec"]);
    let mut next = batch.clone();
    next["batch_id"] = json!("next-batch");
    next["episodes"][0]["batch_id"] = json!("next-batch");
    validate_batch_submission(&next, Some(&stored), &schema).unwrap();
    for (field, value, code) in [
        ("batch_id", json!("wrong-batch"), "BATCH_ID_MISMATCH"),
        ("sample_index", json!(1), "SAMPLE_INDEX_MISMATCH"),
        (
            "run_id",
            json!("override"),
            "UNKNOWN_FIELD:EpisodeRequest.run_id",
        ),
        (
            "run_spec",
            stored.clone(),
            "UNKNOWN_FIELD:EpisodeRequest.run_spec",
        ),
    ] {
        let mut invalid = batch.clone();
        invalid["episodes"][0][field] = value;
        assert_eq!(
            validate_batch_submission(&invalid, None, &schema)
                .unwrap_err()
                .code,
            code
        );
    }
    let mut empty = batch.clone();
    empty["episodes"] = json!([]);
    assert_eq!(
        validate_batch_submission(&empty, None, &schema)
            .unwrap_err()
            .code,
        "EMPTY_BATCH"
    );
    for field in ["request_id", "episode_id"] {
        let mut duplicate = batch.clone();
        let mut second = duplicate["episodes"][0].clone();
        second["sample_index"] = json!(1);
        second["request_id"] = json!("different-request");
        second["episode_id"] = json!("different-episode");
        second[field] = duplicate["episodes"][0][field].clone();
        duplicate["episodes"].as_array_mut().unwrap().push(second);
        assert_eq!(
            validate_batch_submission(&duplicate, None, &schema)
                .unwrap_err()
                .code,
            "DUPLICATE_BATCH_MEMBER"
        );
    }
}

#[test]
fn scoring_switch_and_package_selection_have_no_overrides() {
    let schema = ContractSchema::bundled();
    let baseline =
        read(design_root().join("reference/generated/episodes/gsm8k/execution_plan.json"));
    for (field, value, expected) in [
        (
            "implementation",
            json!({"id": "other/package", "version": "2"}),
            "UNKNOWN_SCORING_FIELD",
        ),
        ("enabled", json!(false), "SCORING_CONFIG_MISMATCH"),
        ("enabled", json!("false"), "INVALID_SCORING_ENABLED"),
    ] {
        let mut invalid = baseline.clone();
        invalid["scoring"][field] = value;
        let error = validate_execution_plan(&seal_plan(&invalid).unwrap(), &schema).unwrap_err();
        assert_eq!(error.code, expected);
    }
    let mut invalid = baseline.clone();
    invalid["environment"]["implementation"] = json!({"id": "other/package", "version": "2"});
    assert_eq!(
        validate_execution_plan(&seal_plan(&invalid).unwrap(), &schema)
            .unwrap_err()
            .code,
        "UNKNOWN_FIELD:TypedConfig.implementation"
    );
    let mut invalid = baseline.clone();
    invalid["scorer"] = json!({"implementation": baseline["dataset_package"]});
    assert_eq!(
        validate_execution_plan(&seal_plan(&invalid).unwrap(), &schema)
            .unwrap_err()
            .code,
        "UNKNOWN_FIELD:ExecutionPlan.scorer"
    );
}

#[test]
fn retry_changes_only_attempt_and_digest() {
    let schema = ContractSchema::bundled();
    let path = design_root().join("reference/generated/episodes/gsm8k/execution_plan.json");
    let first = read(path);
    let second = retry_execution_plan(&first, &schema, 2).unwrap();
    assert_eq!(second["attempt_id"], 2);
    let mut first_without = first.as_object().unwrap().clone();
    let mut second_without = second.as_object().unwrap().clone();
    first_without.remove("attempt_id");
    second_without.remove("attempt_id");
    first_without.remove("plan_digest");
    second_without.remove("plan_digest");
    assert_eq!(first_without, second_without);
    assert_eq!(seal_plan(&second).unwrap(), second);
}

#[test]
fn contract_is_the_only_wire_field_dictionary() {
    let schema = ContractSchema::bundled();
    let plan_fields = schema.fields("ExecutionPlan").unwrap();
    assert!(plan_fields.contains("runtime"));
    for alias in [
        "dependencies",
        "env_type",
        "sandbox_mode",
        "agent_pool_id",
        "score_config",
        "task_view",
    ] {
        assert!(!plan_fields.contains(alias));
    }
    let score_fields = schema.fields("ScoreResult").unwrap();
    assert_eq!(
        score_fields,
        BTreeSet::from([
            "error".to_owned(),
            "evidence".to_owned(),
            "generation_rewards".to_owned(),
            "metrics".to_owned(),
            "reward".to_owned(),
            "scorer".to_owned(),
            "status".to_owned(),
            "success".to_owned(),
        ])
    );
}
