//! Real Python role processes, HTTP model transport, Rust supervision and disk
//! trajectory. The Backend below is a test fixture, not isolation evidence.
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeMap, path::Path, process::Command, rc::Rc};
use uenv_reference_control::contracts::ContractSchema;
use uenv_reference_control::plan::seal_plan;
use uenv_reference_control::ports::{Backend, Clock, FileStore, SystemClock, ToolHost};
use uenv_reference_control::rpc::*;
use uenv_reference_control::storage::LocalFileStore;
use uenv_reference_control::supervisor::EpisodeSupervisor;
use uenv_reference_control::{ControlError, Result};

struct FixtureBackend;
impl Backend for FixtureBackend {
    fn open(&mut self, backend: &Value, _: Option<&Value>, _: bool, _: u64) -> Result<Value> {
        Ok(json!({"session_id":"rpc-pipeline","backend":backend["implementation"]}))
    }
    fn freeze(&mut self, _: u64) -> Result<()> {
        Ok(())
    }
    fn run_harness(&mut self, _: &Value) -> Result<Value> {
        Err(ControlError::new("UNEXPECTED_HARNESS"))
    }
    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

#[test]
fn python_roles_complete_scored_episode_and_seal_durable_trajectory() {
    run_episode(false);
}

#[test]
fn sandbox_tool_runs_in_environment_process_and_returns_same_observation() {
    run_episode(true);
}

fn run_episode(with_tools: bool) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let mut plan: Value = serde_json::from_slice(
        &std::fs::read(root.join("reference/generated/episodes/gsm8k/execution_plan.json"))
            .unwrap(),
    )
    .unwrap();
    let paths = std::env::join_paths([
        root.join("reference/sdk/src"),
        root.join("reference/sdk/tests"),
        root.join("reference/shared/src"),
        root.join("reference"),
    ])
    .unwrap();
    let spawn = |role: &str| {
        let mut command = Command::new(
            std::env::var("UENV_INTEGRATION_PYTHON").unwrap_or_else(|_| "python".into()),
        );
        command
            .args([
                "-m",
                "uenv.sdk.component_host",
                &format!("pipeline_components:create_{role}"),
            ])
            .env("PYTHONPATH", &paths)
            .env("UENV_TEST_COMPONENT", plan["dataset_package"].to_string())
            .env("UENV_TEST_WORKSPACE", workspace.path())
            .env(
                "UENV_TEST_BACKEND",
                plan["backend"]["implementation"].to_string(),
            );
        RpcProcess::spawn(&mut command).unwrap()
    };
    let agent = spawn("agent");
    let environment = spawn("environment");
    let scorer = spawn("scorer");
    let model = spawn("model");
    plan["model"]["endpoint"] = model.call("fixture.endpoint", json!({}), 30000).unwrap();
    plan["limits"]["finalize_reserve_ms"] = json!(5000);
    if with_tools {
        plan["limits"]["max_generations"] = json!(2);
        plan["limits"]["max_tool_calls"] = json!(1);
        plan["tools"] = json!([{"name":"add","implementation":{"id":"tools/add","version":"1.0.0","digest":format!("sha256:{}","a".repeat(64))},
            "config":{"schema_ref":"uenv://schemas/vnext/EmptyConfig","data":{}},"execution_scope":"sandbox","required_capabilities":[]}]);
    }
    plan["deadline_at_ms"] = json!(SystemClock.unix_time_ms() + 60000);
    plan = seal_plan(&plan).unwrap();
    let mut schema = ContractSchema::bundled();
    for entry in std::fs::read_dir(root.join("reference/generated/packages/gsm8k/schemas")).unwrap()
    {
        schema
            .register_extension(
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap(),
            )
            .unwrap();
    }
    let dispatch = json!({"plan":plan,"lease":{"lease_id":"test","epoch":1,"expires_at_ms":plan["deadline_at_ms"],"token":"test"},
        "remaining_timeout_ms":60000,"consumed_usage":{"generation_count":0,"tool_call_count":0,"output_token_count":0}});
    let store_path = workspace.path().join("files");
    let shared: Rc<RefCell<dyn FileStore>> =
        Rc::new(RefCell::new(LocalFileStore::open(&store_path).unwrap()));
    let mut files = LocalFileStore::open(&store_path).unwrap();
    let mut hosts: BTreeMap<String, Box<dyn ToolHost>> = BTreeMap::new();
    hosts.insert("agent_state".into(), Box::new(RpcToolHost(agent.clone())));
    hosts.insert("sandbox".into(), Box::new(RpcToolHost(environment.clone())));
    let result = EpisodeSupervisor::new(&schema, &SystemClock, Default::default())
        .execute(
            &dispatch,
            &mut RpcAgentHost(agent),
            &mut RpcEnvironmentHost {
                process: environment,
                files: shared,
            },
            Some(&mut RpcScorerHost(scorer)),
            &mut FixtureBackend,
            &mut RpcToolRouter::new(hosts),
            &mut RpcModelProvider(model.clone()),
            &mut files,
        )
        .unwrap();
    model.close();
    assert_eq!(result["execution_status"], "completed", "{result}");
    assert_eq!(result["cleanup_status"], "completed", "{result}");
    assert_eq!(result["score"]["reward"], 1.0, "{result}");
    assert_eq!(
        result["usage"]["generation_count"],
        if with_tools { 2 } else { 1 }
    );
    assert_eq!(
        result["usage"]["tool_call_count"],
        if with_tools { 1 } else { 0 }
    );
    if with_tools {
        assert_eq!(
            std::fs::read(workspace.path().join("sum.txt")).unwrap(),
            b"5"
        );
    }
    schema.validate("EpisodeResult", &result).unwrap();
    drop(files);
    let files = LocalFileStore::open(&store_path).unwrap();
    let manifest: Value =
        serde_json::from_slice(&files.read(&result["trajectory_ref"]).unwrap()).unwrap();
    assert_eq!(manifest["trajectory_status"], "final_complete");
    for segment in manifest["event_segments"].as_array().unwrap() {
        let bytes = files.read(segment).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("private_data"));
        for line in bytes.split(|b| *b == b'\n').filter(|b| !b.is_empty()) {
            let event = serde_json::from_slice(line).unwrap();
            assert!(
                schema.validate("TrajectoryEvent", &event).is_ok(),
                "invalid event: {event}"
            );
        }
    }
}
