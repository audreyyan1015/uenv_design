use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use uenv_reference_control::contracts::ContractSchema;
use uenv_reference_control::plan::{
    AgentToolProfile, ComponentCatalog, ComponentMetadata, PlanResolver, RuntimeKind,
    ToolInterfaceSupport, retry_execution_plan, seal_plan, validate_execution_plan,
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
    tool_interfaces: Vec<ToolInterfaceSupport>,
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
            tool_interfaces,
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
        vec![],
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
        let run = read(episode_folder.join("run_spec.json"));
        let episode = read(episode_folder.join("episode_request.json"));
        let package = read(folder.join("manifest.json"));
        let expected = read(episode_folder.join("execution_plan.json"));
        let task_schema = package["task_schema"].as_str().unwrap();
        let mut catalog = ComponentCatalog::default();

        assert_eq!(
            expected["environment"]["implementation"], expected["scorer"]["implementation"],
            "one dataset package must export both roles"
        );
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
                component: expected["environment"]["implementation"].clone(),
                roles: BTreeSet::from(["environment".to_owned(), "scorer".to_owned()]),
                role_config_schemas,
                required_capabilities: strings(&package, "required_capabilities"),
                task_schemas: BTreeSet::from([task_schema.to_owned()]),
                private_schemas,
                native_profiles: BTreeSet::from(["example-only".to_owned()]),
                tool_interfaces: vec![],
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
            vec![],
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
            vec![],
        );

        for tool in expected["tools"].as_array().unwrap() {
            insert_component(
                &mut catalog,
                tool["implementation"].clone(),
                "tool",
                tool["config"]["schema_ref"].as_str().unwrap(),
                task_schema,
                None,
                vec![ToolInterfaceSupport {
                    interface: tool["interface"].as_str().unwrap().to_owned(),
                    adapter: tool["adapter"].clone(),
                    execution_scope: tool["execution_scope"].as_str().unwrap().to_owned(),
                    required_capabilities: strings(tool, "required_capabilities"),
                }],
            );
            insert_component(
                &mut catalog,
                tool["adapter"].clone(),
                "tool_adapter",
                "uenv://schemas/vnext/EmptyConfig",
                task_schema,
                None,
                vec![],
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
                vec![],
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
            supported_interfaces: vec!["openhands_native.v1".to_owned(), "mcp.v1".to_owned()],
        };
        let resolver = PlanResolver {
            schema: &schema,
            catalog: &catalog,
            agent_profile: &profile,
        };
        let rebuilt = resolver
            .resolve(&episode, &run, 1_800_000_000_000, &|image, _, _| {
                Ok(image.clone())
            })
            .unwrap_or_else(|error| panic!("{}: {}", folder.display(), error.code));
        assert_eq!(rebuilt, expected, "{}", folder.display());
        validate_execution_plan(&rebuilt, &schema).unwrap();
    }
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
            "metrics".to_owned(),
            "reward".to_owned(),
            "scorer".to_owned(),
            "status".to_owned(),
            "success".to_owned(),
        ])
    );
}
