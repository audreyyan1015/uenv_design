use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Value, json};
use uenv_reference_control::contracts::ContractSchema;
use uenv_reference_control::plan::{seal_plan, validate_result_for_plan};
use uenv_reference_control::ports::{
    AgentHost, Backend, Cancellation, Clock, EnvironmentHost, FileStore, MemoryFileStore,
    ModelProvider, ScorerHost, ScoringContext, ToolHost,
};
use uenv_reference_control::runtime::{AgentRuntime, BudgetEnforcer, Usage};
use uenv_reference_control::scoring::run_score;
use uenv_reference_control::supervisor::EpisodeSupervisor;
use uenv_reference_control::{ControlError, Result};

struct FixedClock(Cell<u64>);

impl Clock for FixedClock {
    fn monotonic_ms(&self) -> u64 {
        self.0.get()
    }

    fn unix_time_ms(&self) -> u64 {
        self.0.get()
    }
}

type Lifecycle = Rc<RefCell<Vec<&'static str>>>;

fn mark(lifecycle: &Option<Lifecycle>, event: &'static str) {
    if let Some(lifecycle) = lifecycle {
        lifecycle.borrow_mut().push(event);
    }
}

fn hidden_tool() -> Value {
    json!({
        "name": "hidden",
        "implementation": {"id": "tools/hidden", "version": "1", "digest": format!("sha256:{}", "0".repeat(64))},
        "config": {"schema_ref": "uenv://schemas/vnext/EmptyConfig", "data": {}},
        "execution_scope": "agent_state",
        "required_capabilities": []
    })
}

#[derive(Default)]
struct BackendProbe {
    opened: bool,
    frozen: bool,
    closed: bool,
    open_timeout_ms: Option<u64>,
    freeze_timeout_ms: Option<u64>,
    lifecycle: Option<Lifecycle>,
}

impl Backend for BackendProbe {
    fn open(&mut self, _: &Value, _: Option<&Value>, _: bool, timeout_ms: u64) -> Result<Value> {
        mark(&self.lifecycle, "backend.open");
        self.opened = true;
        self.open_timeout_ms = Some(timeout_ms);
        Ok(json!({"session_id": "session-1"}))
    }

    fn freeze(&mut self, timeout_ms: u64) -> Result<()> {
        mark(&self.lifecycle, "backend.freeze");
        self.frozen = true;
        self.freeze_timeout_ms = Some(timeout_ms);
        Ok(())
    }

    fn run_harness(&mut self, _: &Value) -> Result<Value> {
        Err(ControlError::new("HARNESS_NOT_USED"))
    }

    fn close(&mut self) -> Result<()> {
        mark(&self.lifecycle, "backend.close");
        self.closed = true;
        Ok(())
    }
}

#[derive(Default)]
struct ToolHostProbe {
    actual_tools: Vec<Value>,
    frozen: bool,
    closed: bool,
    report_hidden_tool: bool,
    fail_call: bool,
    executions: usize,
    terminal_result: bool,
    prepare_timeout_ms: Option<u64>,
    freeze_timeout_ms: Option<u64>,
    lifecycle: Option<Lifecycle>,
}

impl ToolHost for ToolHostProbe {
    fn prepare(&mut self, tools: &[Value], _: &Value, timeout_ms: u64) -> Result<Vec<Value>> {
        mark(&self.lifecycle, "tools.prepare");
        self.prepare_timeout_ms = Some(timeout_ms);
        self.actual_tools = tools.to_vec();
        let mut tools = self.actual_tools.clone();
        if self.report_hidden_tool {
            tools.push(hidden_tool());
        }
        Ok(tools)
    }

    fn validate_call(&self, _: &Value, call: &Value) -> Result<()> {
        if !call["arguments"].is_object() || call["arguments"].get("invalid").is_some() {
            return Err(ControlError::new("INVALID_TOOL_ARGUMENTS"));
        }
        Ok(())
    }

    fn call_tool(&mut self, _: &Value, call: &Value) -> Result<Value> {
        self.executions += 1;
        if self.fail_call {
            return Err(ControlError::new("TOOL_PROCESS_FAILED"));
        }
        Ok(json!({
            "tool_call_id": call["tool_call_id"],
            "status": "ok",
            "observation": {"content": [{"kind": "text", "text": "ok"}], "terminated": self.terminal_result, "episode_truncated": false},
            "output_truncated": false
        }))
    }

    fn freeze(&mut self, timeout_ms: u64) -> Result<()> {
        mark(&self.lifecycle, "tools.freeze");
        self.frozen = true;
        self.freeze_timeout_ms = Some(timeout_ms);
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        mark(&self.lifecycle, "tools.close");
        self.closed = true;
        Ok(())
    }
}

#[derive(Default)]
struct ModelProbe {
    requests: Vec<Value>,
    wrong_identity: bool,
    length_limited: bool,
}

impl ModelProvider for ModelProbe {
    fn generate(&mut self, request: &Value) -> Result<Value> {
        self.requests.push(request.clone());
        Ok(json!({
            "generation_id": request["generation_id"],
            "model_id": if self.wrong_identity { json!("wrong-model") } else { request["model"]["model_id"].clone() },
            "source": request["model"]["source"],
            "messages": request["messages"],
            "response": [{"kind": "text", "text": "5"}],
            "output_token_count": 1,
            "input_token_ids": [1, 2],
            "output_token_ids": [3],
            "finish_reason": if self.length_limited { "length" } else { "stop" },
            "duration_ms": 1
        }))
    }
}

#[derive(Default)]
struct AgentProbe {
    closed: bool,
    report_hidden_tool: bool,
    prepare_timeout_ms: Option<u64>,
    run_timeout_ms: Option<u64>,
    lifecycle: Option<Lifecycle>,
    finish_at: Option<(Rc<Cell<u64>>, u64)>,
    claim_environment_terminal: bool,
}

impl AgentHost for AgentProbe {
    fn prepare(
        &mut self,
        _: &Value,
        tools: &[Value],
        _: &str,
        timeout_ms: u64,
    ) -> Result<Vec<Value>> {
        mark(&self.lifecycle, "agent.prepare");
        self.prepare_timeout_ms = Some(timeout_ms);
        let mut visible_tools = tools.to_vec();
        if self.report_hidden_tool {
            visible_tools.push(hidden_tool());
        }
        Ok(visible_tools)
    }

    fn run_agent(
        &mut self,
        _: &Value,
        observation: &Value,
        runtime: &mut AgentRuntime<'_>,
        timeout_ms: u64,
    ) -> Result<Value> {
        mark(&self.lifecycle, "agent.run");
        self.run_timeout_ms = Some(timeout_ms);
        let generation = runtime.generate(&json!([{
            "role": "user",
            "content": observation["content"]
        }]))?;
        if let Some((clock, finish_at)) = &self.finish_at {
            clock.set(*finish_at);
        }
        if self.claim_environment_terminal {
            return Ok(
                json!({"final_answer": generation["response"], "termination_reason": "environment_terminal"}),
            );
        }
        Ok(generation["response"].clone())
    }

    fn close(&mut self) -> Result<()> {
        mark(&self.lifecycle, "agent.close");
        self.closed = true;
        Ok(())
    }
}

#[derive(Default)]
struct EnvironmentProbe {
    closed: bool,
    reset_terminal: bool,
    reset_truncated: bool,
    structured_observation: bool,
    snapshot_calls: usize,
    prepare_timeout_ms: Option<u64>,
    reset_timeout_ms: Option<u64>,
    snapshot_timeout_ms: Option<u64>,
    lifecycle: Option<Lifecycle>,
}

impl EnvironmentHost for EnvironmentProbe {
    fn prepare(&mut self, _: &Value, _: &Value, _: &Value, timeout_ms: u64) -> Result<()> {
        mark(&self.lifecycle, "environment.prepare");
        self.prepare_timeout_ms = Some(timeout_ms);
        Ok(())
    }

    fn reset(&mut self, _: &Value, _: u64, timeout_ms: u64) -> Result<Value> {
        mark(&self.lifecycle, "environment.reset");
        self.reset_timeout_ms = Some(timeout_ms);
        let mut observation = json!({"content": [{"kind": "text", "text": "2+3?"}],
            "terminated": self.reset_terminal, "episode_truncated": self.reset_truncated});
        if self.structured_observation {
            observation["content"].as_array_mut().unwrap().push(json!({
                "kind": "structured", "structured": {
                    "schema_ref": "uenv://schemas/vnext/EmptyConfig", "data": {}
                }
            }));
        }
        Ok(observation)
    }

    fn state_snapshot(&mut self, timeout_ms: u64) -> Result<Option<Value>> {
        mark(&self.lifecycle, "environment.state_snapshot");
        self.snapshot_calls += 1;
        self.snapshot_timeout_ms = Some(timeout_ms);
        Ok(Some(
            json!({"schema_ref": "uenv://schemas/vnext/EmptyConfig", "data": {}}),
        ))
    }

    fn close(&mut self) -> Result<()> {
        mark(&self.lifecycle, "environment.close");
        self.closed = true;
        Ok(())
    }
}

#[derive(Default)]
struct ScorerProbe {
    process_score_case: Option<&'static str>,
    calls: usize,
    saw_private_data: bool,
    final_answer: Value,
    trajectory_root_is_manifest: bool,
    closed: bool,
    lifecycle: Option<Lifecycle>,
}

impl ScorerHost for ScorerProbe {
    fn score(
        &mut self,
        _: &Value,
        _: &Value,
        request: &Value,
        context: &mut dyn ScoringContext,
    ) -> Result<Value> {
        mark(&self.lifecycle, "scorer.score");
        self.calls += 1;
        self.saw_private_data = request.get("private_data").is_some();
        self.final_answer = request["final_answer"].clone();
        assert!(request.get("final_answer").unwrap().is_array());
        assert!(request.get("artifacts").is_none());
        assert!(request.get("state").is_some());
        assert!(request.get("outcome").is_none());
        let bytes = context.read_artifact(&request["trajectory_ref"])?;
        let root: Value = serde_json::from_slice(&bytes).unwrap();
        self.trajectory_root_is_manifest = root.get("event_segments").is_some();
        let mut result = json!({
            "success": true,
            "metrics": [{"name": "accuracy", "value": 1.0, "unit": "ratio", "direction": "higher"}],
            "reward": 1.0,
            "evidence": []
        });
        if let Some(case) = self.process_score_case {
            let bytes = context.read_artifact(&root["event_segments"][0])?;
            let events: Vec<Value> = String::from_utf8(bytes)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let generation = events
                .iter()
                .find(|event| event["kind"] == "generation")
                .unwrap();
            let mut item =
                json!({"generation_id": generation["payload"]["generation_id"], "reward": -0.25});
            match case {
                "unknown" => item["generation_id"] = json!("other-attempt/generation-1"),
                "null" => item["reward"] = Value::Null,
                "extra" => item["step"] = json!(1),
                _ => (),
            }
            result["generation_rewards"] = if case == "duplicate" {
                json!([item.clone(), item])
            } else {
                json!([item])
            };
        }
        Ok(result)
    }

    fn close(&mut self) -> Result<()> {
        mark(&self.lifecycle, "scorer.close");
        self.closed = true;
        Ok(())
    }
}

fn plan() -> Value {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    serde_json::from_slice(
        &fs::read(root.join("reference/generated/episodes/gsm8k/execution_plan.json")).unwrap(),
    )
    .unwrap()
}

fn capabilities(plan: &Value) -> BTreeSet<String> {
    plan["required_capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect()
}

fn dispatch(plan: &Value) -> Value {
    let deadline_at_ms = plan["deadline_at_ms"].as_u64().unwrap();
    let remaining_timeout_ms = deadline_at_ms - 1_800_000_000_100u64;
    json!({
        "plan": plan,
        "lease": {
            "lease_id": "lease-1",
            "epoch": 1,
            "expires_at_ms": deadline_at_ms,
            "token": "test-token"
        },
        "remaining_timeout_ms": remaining_timeout_ms,
        "consumed_usage": {
            "generation_count": 0,
            "tool_call_count": 0,

            "output_token_count": 0
        }
    })
}

#[test]
fn supervisor_runs_one_uniform_path_and_completes_system_fields() {
    let schema = ContractSchema::bundled();
    let plan = plan();
    let clock = FixedClock(Cell::new(1_800_000_000_100));
    let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
    let lifecycle = Rc::new(RefCell::new(Vec::new()));
    let mut agent = AgentProbe {
        lifecycle: Some(lifecycle.clone()),
        ..Default::default()
    };
    let mut environment = EnvironmentProbe {
        lifecycle: Some(lifecycle.clone()),
        ..Default::default()
    };
    let mut scorer = ScorerProbe {
        lifecycle: Some(lifecycle.clone()),
        ..Default::default()
    };
    let mut backend = BackendProbe {
        lifecycle: Some(lifecycle.clone()),
        ..Default::default()
    };
    let mut tools = ToolHostProbe {
        lifecycle: Some(lifecycle.clone()),
        ..Default::default()
    };
    let mut model = ModelProbe::default();
    let mut artifacts = MemoryFileStore::default();

    let result = supervisor
        .execute(
            &dispatch(&plan),
            &mut agent,
            &mut environment,
            Some(&mut scorer),
            &mut backend,
            &mut tools,
            &mut model,
            &mut artifacts,
        )
        .unwrap();

    assert_eq!(result["execution_status"], "completed");
    assert_eq!(result["score"]["status"], "ok");
    assert_eq!(result["score"]["reward"], 1.0);
    assert_eq!(result["score"]["scorer"], plan["dataset_package"]);
    assert_eq!(scorer.calls, 1);
    assert!(result.get("state").is_none());
    assert!(result.get("private_data").is_none());
    assert!(result.get("outcome").is_none());
    assert_eq!(
        result["final_answer"],
        json!([{"kind": "text", "text": "5"}])
    );
    assert_eq!(result["termination_reason"], "final_answer");
    assert!(scorer.saw_private_data);
    assert!(scorer.trajectory_root_is_manifest);
    assert!(scorer.closed);
    assert!(agent.closed && environment.closed && backend.closed && tools.closed);
    assert!(backend.frozen && tools.frozen);
    assert!(
        [
            backend.open_timeout_ms,
            backend.freeze_timeout_ms,
            tools.prepare_timeout_ms,
            tools.freeze_timeout_ms,
            agent.prepare_timeout_ms,
            agent.run_timeout_ms,
            environment.prepare_timeout_ms,
            environment.reset_timeout_ms,
            environment.snapshot_timeout_ms,
        ]
        .into_iter()
        .all(|timeout| timeout.is_some_and(|value| value > 0))
    );
    assert_eq!(
        lifecycle.borrow().as_slice(),
        [
            "backend.open",
            "environment.prepare",
            "tools.prepare",
            "agent.prepare",
            "environment.reset",
            "agent.run",
            "tools.freeze",
            "backend.freeze",
            "environment.state_snapshot",
            "scorer.score",
            "scorer.close",
            "agent.close",
            "tools.close",
            "environment.close",
            "backend.close",
        ]
    );
    assert_eq!(model.requests.len(), 1);
    assert_eq!(model.requests[0]["model"], plan["model"]);
    assert_eq!(model.requests[0]["remaining_timeout_ms"], 179_900);

    let manifest_bytes = artifacts.read(&result["trajectory_ref"]).unwrap();
    let manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest["trajectory_status"], "final_complete");
    let events_bytes = artifacts.read(&manifest["event_segments"][0]).unwrap();
    assert_eq!(
        manifest["event_segments"][0]["media_type"],
        "application/x-ndjson"
    );
    let events: Vec<Value> = std::str::from_utf8(&events_bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let text = serde_json::to_string(&events).unwrap();
    assert!(!text.contains("private_data"));
    let sequences: Vec<u64> = events
        .iter()
        .map(|event| event["sequence"].as_u64().unwrap())
        .collect();
    assert_eq!(sequences, (0..sequences.len() as u64).collect::<Vec<_>>());
}

struct SteppingAgent;

impl AgentHost for SteppingAgent {
    fn prepare(&mut self, _: &Value, tools: &[Value], _: &str, _: u64) -> Result<Vec<Value>> {
        Ok(tools.to_vec())
    }

    fn run_agent(
        &mut self,
        _: &Value,
        _: &Value,
        runtime: &mut AgentRuntime<'_>,
        _: u64,
    ) -> Result<Value> {
        let generation = runtime.generate(&json!([{"role":"user","content":[]}]))?;
        Ok(generation["response"].clone())
    }

    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

struct ToolCallingAgent;

struct ToolGateAgent;

impl AgentHost for ToolGateAgent {
    fn prepare(&mut self, _: &Value, tools: &[Value], _: &str, _: u64) -> Result<Vec<Value>> {
        Ok(tools.to_vec())
    }
    fn run_agent(
        &mut self,
        _: &Value,
        _: &Value,
        runtime: &mut AgentRuntime<'_>,
        _: u64,
    ) -> Result<Value> {
        let event = runtime.generate(&json!([{"role":"user","content":[]}]))?;
        let call = event["tool_calls"][0].clone();
        let mut invalid = call.clone();
        invalid["arguments"] = json!({"invalid":true});
        assert_eq!(
            runtime.step(&invalid).unwrap_err().code,
            "TOOL_CALL_CHANGED"
        );
        assert_eq!(runtime.usage().tool_call_count, 0);
        runtime.step(&call)?;
        assert_eq!(runtime.usage().tool_call_count, 1);
        if runtime.observation()["terminated"] == true {
            assert_eq!(
                runtime.step(&call).unwrap_err().code,
                "ENVIRONMENT_ALREADY_FINISHED"
            );
            assert_eq!(
                runtime.generate(&json!([])).unwrap_err().code,
                "ENVIRONMENT_ALREADY_FINISHED"
            );
        } else {
            assert_eq!(
                runtime.step(&call).unwrap_err().code,
                "DUPLICATE_TOOL_CALL_ID"
            );
            let mut another = call;
            another["tool_call_id"] = json!("second-call");
            assert_eq!(
                runtime.step(&another).unwrap_err().code,
                "TOOL_CALL_NOT_GENERATED"
            );
        }
        Ok(json!([]))
    }
    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

#[test]
fn native_and_python_bindings_use_one_step_budget_and_event_pair() {
    for native in [false, true] {
        for terminal in [false, true] {
            let schema = ContractSchema::bundled();
            let mut plan = plan_with_test_tool();
            if native {
                let owner = plan["agent"]["implementation"].clone();
                plan["tools"][0]["native_agent"] = owner.clone();
                plan["tools"][0]["implementation"]["version"] = owner["version"].clone();
                plan["tools"][0]["implementation"]["digest"] = owner["digest"].clone();
            }
            let plan = seal_plan(&plan).unwrap();
            let clock = FixedClock(Cell::new(1_800_000_000_100));
            let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
            let mut agent = ToolGateAgent;
            let mut environment = EnvironmentProbe::default();
            let mut scorer = ScorerProbe::default();
            let mut backend = BackendProbe::default();
            let mut tools = ToolHostProbe {
                terminal_result: terminal,
                ..Default::default()
            };
            let mut model = ToolCallingModel::default();
            let mut artifacts = MemoryFileStore::default();
            let result = supervisor
                .execute(
                    &dispatch(&plan),
                    &mut agent,
                    &mut environment,
                    Some(&mut scorer),
                    &mut backend,
                    &mut tools,
                    &mut model,
                    &mut artifacts,
                )
                .unwrap();
            assert_eq!(result["execution_status"], "completed", "{result}");
            assert_eq!(result["usage"]["tool_call_count"], 1);
            assert_eq!(tools.executions, 1);
            let manifest: Value =
                serde_json::from_slice(&artifacts.read(&result["trajectory_ref"]).unwrap())
                    .unwrap();
            let events = artifacts.read(&manifest["event_segments"][0]).unwrap();
            let events: Vec<Value> = std::str::from_utf8(&events)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(
                events.iter().filter(|e| e["kind"] == "tool_call").count(),
                1
            );
            assert_eq!(
                events.iter().filter(|e| e["kind"] == "tool_result").count(),
                1
            );
        }
    }
}

impl AgentHost for ToolCallingAgent {
    fn prepare(&mut self, _: &Value, tools: &[Value], _: &str, _: u64) -> Result<Vec<Value>> {
        Ok(tools.to_vec())
    }

    fn run_agent(
        &mut self,
        _: &Value,
        _: &Value,
        runtime: &mut AgentRuntime<'_>,
        _: u64,
    ) -> Result<Value> {
        let generation = runtime.generate(&json!([{
            "role": "user",
            "content": [{"kind": "text", "text": "use the selected tool"}]
        }]))?;
        runtime.step(&generation["tool_calls"][0])?;
        unreachable!()
    }

    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

fn plan_with_test_tool() -> Value {
    let mut changed = plan();
    changed["limits"]["max_tool_calls"] = json!(1);
    changed["tools"] = json!([{
        "name": "test_tool",
        "implementation": {
            "id": "tools/test",
            "version": "1",
            "digest": format!("sha256:{}", "2".repeat(64))
        },
        "config": {
            "schema_ref": "uenv://schemas/vnext/ToolConfig",
            "data": {"timeout_ms": 1_000, "max_preview_bytes": 1024}
        },
        "execution_scope": "sandbox",
        "required_capabilities": []
    }]);
    seal_plan(&changed).unwrap()
}

#[test]
fn finalization_and_scoring_use_the_reserve_without_extending_the_deadline() {
    struct SharedClock(Rc<Cell<u64>>);
    impl Clock for SharedClock {
        fn monotonic_ms(&self) -> u64 {
            self.0.get()
        }
        fn unix_time_ms(&self) -> u64 {
            self.0.get()
        }
    }
    let schema = ContractSchema::bundled();
    let plan = plan();
    let now = Rc::new(Cell::new(1_800_000_000_100));
    let clock = SharedClock(now.clone());
    let deadline = plan["deadline_at_ms"].as_u64().unwrap();
    let reserve = plan["limits"]["finalize_reserve_ms"].as_u64().unwrap();
    let request = dispatch(&plan);
    let budget = BudgetEnforcer::from_dispatch(&request, &clock).unwrap();
    let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
    let mut agent = AgentProbe {
        finish_at: Some((now.clone(), deadline - reserve)),
        ..Default::default()
    };
    let mut environment = EnvironmentProbe::default();
    let mut scorer = ScorerProbe::default();
    let mut backend = BackendProbe::default();
    let mut tools = ToolHostProbe::default();
    let mut model = ModelProbe::default();
    let mut artifacts = MemoryFileStore::default();
    let result = supervisor
        .execute(
            &request,
            &mut agent,
            &mut environment,
            Some(&mut scorer),
            &mut backend,
            &mut tools,
            &mut model,
            &mut artifacts,
        )
        .unwrap();
    assert_eq!(result["execution_status"], "completed");
    assert_eq!(result["termination_reason"], "budget_exhausted");
    assert_eq!(environment.snapshot_timeout_ms, Some(reserve));
    assert_eq!(scorer.calls, 1);
    now.set(deadline);
    assert_eq!(budget.finalize_deadline_ms(), deadline);
    assert_eq!(
        budget.remaining_finalize_ms(&clock).unwrap_err().code,
        "EPISODE_TIMEOUT"
    );
}

#[test]
fn direct_answer_uses_generation_without_a_synthetic_action() {
    let schema = ContractSchema::bundled();
    let plan = plan();
    let clock = FixedClock(Cell::new(1_800_000_000_100));
    let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
    let mut agent = SteppingAgent;
    let mut environment = EnvironmentProbe::default();
    let mut scorer = ScorerProbe::default();
    let mut backend = BackendProbe::default();
    let mut tools = ToolHostProbe::default();
    let mut model = ModelProbe::default();
    let mut artifacts = MemoryFileStore::default();

    let retry_dispatch = dispatch(&plan);
    let result = supervisor
        .execute(
            &retry_dispatch,
            &mut agent,
            &mut environment,
            Some(&mut scorer),
            &mut backend,
            &mut tools,
            &mut model,
            &mut artifacts,
        )
        .unwrap();
    assert_eq!(result["execution_status"], "completed");
    assert_eq!(result["usage"]["tool_call_count"], 0);
    assert_eq!(result["usage"]["generation_count"], 1);

    let manifest: Value =
        serde_json::from_slice(&artifacts.read(&result["trajectory_ref"]).unwrap()).unwrap();
    let events = artifacts.read(&manifest["event_segments"][0]).unwrap();
    let events = std::str::from_utf8(&events).unwrap();
    assert!(!events.contains("environment_transition"));
    assert!(events.contains("generation"));
}

#[test]
fn failed_tool_call_still_records_a_paired_result() {
    let schema = ContractSchema::bundled();
    let plan = plan_with_test_tool();
    let clock = FixedClock(Cell::new(1_800_000_000_100));
    let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
    let mut agent = ToolCallingAgent;
    let mut environment = EnvironmentProbe::default();
    let mut scorer = ScorerProbe::default();
    let mut backend = BackendProbe::default();
    let mut tools = ToolHostProbe {
        fail_call: true,
        ..Default::default()
    };
    let mut model = ToolCallingModel::default();
    let mut artifacts = MemoryFileStore::default();

    let result = supervisor
        .execute(
            &dispatch(&plan),
            &mut agent,
            &mut environment,
            Some(&mut scorer),
            &mut backend,
            &mut tools,
            &mut model,
            &mut artifacts,
        )
        .unwrap();
    assert_eq!(result["execution_status"], "failed");
    assert_eq!(result["error"]["code"], "TOOL_PROCESS_FAILED");
    assert_eq!(result["error"]["phase"], "tool");
    assert_eq!(result["error"]["operation_id"], "model-call-1");

    let manifest: Value =
        serde_json::from_slice(&artifacts.read(&result["trajectory_ref"]).unwrap()).unwrap();
    assert_eq!(manifest["trajectory_status"], "final_partial");
    let events = artifacts.read(&manifest["event_segments"][0]).unwrap();
    let events: Vec<Value> = std::str::from_utf8(&events)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let tool_call = events
        .iter()
        .find(|event| event["kind"] == "tool_call")
        .unwrap();
    let tool_result = events
        .iter()
        .find(|event| event["kind"] == "tool_result")
        .unwrap();
    assert_eq!(
        tool_call["payload"]["tool_call_id"],
        tool_result["payload"]["tool_call_id"]
    );
    assert_eq!(tool_result["payload"]["status"], "error");
    assert_eq!(
        tool_result["payload"]["error"]["operation_id"],
        "model-call-1"
    );
}

struct FailingModel;

impl ModelProvider for FailingModel {
    fn generate(&mut self, _: &Value) -> Result<Value> {
        Err(ControlError::retryable("MODEL_TRANSPORT_UNAVAILABLE"))
    }
}

#[test]
fn accepted_model_failure_has_explicit_phase_operation_and_retryability() {
    let schema = ContractSchema::bundled();
    let plan = plan();
    let clock = FixedClock(Cell::new(10_000));
    let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
    let mut agent = AgentProbe::default();
    let mut environment = EnvironmentProbe::default();
    let mut scorer = ScorerProbe::default();
    let mut backend = BackendProbe::default();
    let mut tools = ToolHostProbe::default();
    let mut model = FailingModel;
    let mut artifacts = MemoryFileStore::default();

    let result = supervisor
        .execute(
            &dispatch(&plan),
            &mut agent,
            &mut environment,
            Some(&mut scorer),
            &mut backend,
            &mut tools,
            &mut model,
            &mut artifacts,
        )
        .unwrap();
    assert_eq!(result["execution_status"], "failed");
    assert_eq!(result["error"]["phase"], "model");
    assert_eq!(result["error"]["retryable"], true);
    assert_eq!(
        result["error"]["operation_id"],
        format!(
            "{}:{}:generation:0",
            plan["episode_id"].as_str().unwrap(),
            plan["attempt_id"].as_u64().unwrap()
        )
    );
    assert_eq!(result["usage"]["generation_count"], 1);
}

struct ForgingScorer;

impl ScorerHost for ForgingScorer {
    fn score(
        &mut self,
        _: &Value,
        _: &Value,
        _: &Value,
        _: &mut dyn ScoringContext,
    ) -> Result<Value> {
        Ok(json!({
            "status": "ok",
            "success": true,
            "metrics": [],
            "reward": 1.0,
            "evidence": [],
            "scorer": {"id": "forged", "version": "1", "digest": format!("sha256:{}", "0".repeat(64))}
        }))
    }

    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

struct NoServices;

impl ScoringContext for NoServices {
    fn run_harness(&mut self, _: &Value) -> Result<Value> {
        unreachable!()
    }
    fn read_artifact(&self, _: &Value) -> Result<Vec<u8>> {
        unreachable!()
    }
    fn remaining_timeout_ms(&self) -> Result<u64> {
        Ok(1000)
    }
}

struct TimedOutServices;

impl ScoringContext for TimedOutServices {
    fn run_harness(&mut self, _: &Value) -> Result<Value> {
        unreachable!()
    }
    fn read_artifact(&self, _: &Value) -> Result<Vec<u8>> {
        unreachable!()
    }
    fn remaining_timeout_ms(&self) -> Result<u64> {
        Err(ControlError::new("SCORER_TIMEOUT"))
    }
}

#[test]
fn scorer_timeout_is_not_relabelled_as_generic_failure() {
    let plan = plan();
    let request = json!({
        "task": plan["task"],
        "final_answer": [],
        "trajectory_ref": {"uri": "memory://x", "digest": format!("sha256:{}", "0".repeat(64)), "size_bytes": 0, "media_type": "application/json"},
        "private_data": plan["private_data"]
    });
    let score = run_score(
        &ContractSchema::bundled(),
        &mut ScorerProbe::default(),
        &plan["dataset_package"],
        &plan["scoring"]["config"],
        &request,
        &mut TimedOutServices,
    );
    assert_eq!(score["status"], "error");
    assert_eq!(score["error"]["code"], "SCORER_TIMEOUT");
}

#[test]
fn scorer_cannot_set_system_owned_fields() {
    let plan = plan();
    let request = json!({
        "task": plan["task"],
        "final_answer": [],
        "trajectory_ref": {"uri": "memory://x", "digest": format!("sha256:{}", "0".repeat(64)), "size_bytes": 0, "media_type": "application/json"},
        "private_data": plan["private_data"]
    });
    let score = run_score(
        &ContractSchema::bundled(),
        &mut ForgingScorer,
        &plan["dataset_package"],
        &plan["scoring"]["config"],
        &request,
        &mut NoServices,
    );
    assert_eq!(score["status"], "error");
    assert_eq!(score["reward"], Value::Null);
    assert_eq!(score["scorer"], plan["dataset_package"]);
}

#[test]
fn finalize_reserve_and_call_limits_are_enforced_in_rust() {
    let plan = plan();
    let clock = FixedClock(Cell::new(10_000));
    let mut reserve_dispatch = dispatch(&plan);
    reserve_dispatch["remaining_timeout_ms"] = plan["limits"]["finalize_reserve_ms"].clone();
    let mut budget = BudgetEnforcer::from_dispatch(&reserve_dispatch, &clock).unwrap();
    assert_eq!(
        budget.begin_model(&clock).unwrap_err().code,
        "FINALIZE_RESERVE_REACHED"
    );

    let mut plan = plan;
    plan["limits"]["max_generations"] = json!(1);
    let initial_dispatch = dispatch(&plan);
    let mut budget = BudgetEnforcer::from_dispatch(&initial_dispatch, &clock).unwrap();
    budget.begin_model(&clock).unwrap();
    budget.finish_model(1).unwrap();
    assert_eq!(
        budget.begin_model(&clock).unwrap_err().code,
        "GENERATION_LIMIT"
    );

    let mut retry_dispatch = dispatch(&plan);
    retry_dispatch["consumed_usage"] = Usage {
        generation_count: 1,
        ..Usage::default()
    }
    .to_value();
    let mut budget = BudgetEnforcer::from_dispatch(&retry_dispatch, &clock).unwrap();
    assert_eq!(
        budget.begin_model(&clock).unwrap_err().code,
        "GENERATION_LIMIT"
    );
}

struct FailingScorer {
    closed: bool,
}

impl ScorerHost for FailingScorer {
    fn score(
        &mut self,
        _: &Value,
        _: &Value,
        _: &Value,
        _: &mut dyn ScoringContext,
    ) -> Result<Value> {
        Err(ControlError::new("SCORER_PROCESS_CRASH"))
    }

    fn close(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }
}

#[test]
fn scorer_failure_keeps_the_final_answer_and_error_score() {
    let schema = ContractSchema::bundled();
    let plan = plan();
    let clock = FixedClock(Cell::new(1_800_000_000_100));
    let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
    let mut agent = AgentProbe::default();
    let mut environment = EnvironmentProbe::default();
    let mut scorer = FailingScorer { closed: false };
    let mut backend = BackendProbe::default();
    let mut tools = ToolHostProbe::default();
    let mut model = ModelProbe::default();
    let mut artifacts = MemoryFileStore::default();

    let result = supervisor
        .execute(
            &dispatch(&plan),
            &mut agent,
            &mut environment,
            Some(&mut scorer),
            &mut backend,
            &mut tools,
            &mut model,
            &mut artifacts,
        )
        .unwrap();
    assert_eq!(result["execution_status"], "failed");
    assert!(result.get("final_answer").is_some());
    assert_eq!(result["score"]["status"], "error");
    assert_eq!(result["score"]["error"]["code"], "SCORER_FAILED");
    assert!(scorer.closed);
}

struct FailingEnvironment {
    closed: bool,
}

impl EnvironmentHost for FailingEnvironment {
    fn prepare(&mut self, _: &Value, _: &Value, _: &Value, _: u64) -> Result<()> {
        Ok(())
    }
    fn reset(&mut self, _: &Value, _: u64, _: u64) -> Result<Value> {
        Err(ControlError::new("RESET_FAILED"))
    }

    fn close(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }
}

#[test]
fn environment_failure_still_closes_both_hosts_and_backend() {
    let schema = ContractSchema::bundled();
    let plan = plan();
    let clock = FixedClock(Cell::new(1_800_000_000_100));
    let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
    let mut agent = AgentProbe::default();
    let mut environment = FailingEnvironment { closed: false };
    let mut scorer = ScorerProbe::default();
    let mut backend = BackendProbe::default();
    let mut tools = ToolHostProbe::default();
    let mut model = ModelProbe::default();
    let mut artifacts = MemoryFileStore::default();
    let result = supervisor
        .execute(
            &dispatch(&plan),
            &mut agent,
            &mut environment,
            Some(&mut scorer),
            &mut backend,
            &mut tools,
            &mut model,
            &mut artifacts,
        )
        .unwrap();
    assert_eq!(result["execution_status"], "failed");
    assert!(agent.closed && environment.closed && backend.closed && tools.closed);
    assert_eq!(scorer.calls, 0);
    let manifest_bytes = artifacts.read(&result["trajectory_ref"]).unwrap();
    let manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest["trajectory_status"], "final_complete");
}

#[test]
fn tool_probe_failure_after_open_closes_all_started_resources() {
    let schema = ContractSchema::bundled();
    let plan = plan();
    let clock = FixedClock(Cell::new(1_800_000_000_100));
    let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
    let mut agent = AgentProbe::default();
    let mut environment = EnvironmentProbe::default();
    let mut scorer = ScorerProbe::default();
    let mut backend = BackendProbe::default();
    let mut tools = ToolHostProbe {
        report_hidden_tool: true,
        ..Default::default()
    };
    let mut model = ModelProbe::default();
    let mut artifacts = MemoryFileStore::default();
    let result = supervisor
        .execute(
            &dispatch(&plan),
            &mut agent,
            &mut environment,
            Some(&mut scorer),
            &mut backend,
            &mut tools,
            &mut model,
            &mut artifacts,
        )
        .unwrap();
    assert_eq!(result["execution_status"], "failed");
    assert!(backend.opened && backend.closed);
    assert!(agent.closed && environment.closed && tools.closed);
    assert_eq!(scorer.calls, 0);
}

#[test]
fn agent_visible_tool_mismatch_closes_all_started_resources() {
    let schema = ContractSchema::bundled();
    let plan = plan();
    let clock = FixedClock(Cell::new(1_800_000_000_100));
    let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
    let mut agent = AgentProbe {
        report_hidden_tool: true,
        ..Default::default()
    };
    let mut environment = EnvironmentProbe::default();
    let mut scorer = ScorerProbe::default();
    let mut backend = BackendProbe::default();
    let mut tools = ToolHostProbe::default();
    let mut model = ModelProbe::default();
    let mut artifacts = MemoryFileStore::default();
    let result = supervisor
        .execute(
            &dispatch(&plan),
            &mut agent,
            &mut environment,
            Some(&mut scorer),
            &mut backend,
            &mut tools,
            &mut model,
            &mut artifacts,
        )
        .unwrap();
    assert_eq!(result["execution_status"], "failed");
    assert!(backend.opened && backend.closed);
    assert!(agent.closed && environment.closed && tools.closed);
    assert_eq!(scorer.calls, 0);
}

struct ToggleCancellation(Rc<Cell<bool>>);

impl Cancellation for ToggleCancellation {
    fn is_cancelled(&self) -> bool {
        self.0.get()
    }
}

struct CancellingEnvironment {
    cancelled: Rc<Cell<bool>>,
    closed: bool,
}

impl EnvironmentHost for CancellingEnvironment {
    fn prepare(&mut self, _: &Value, _: &Value, _: &Value, _: u64) -> Result<()> {
        Ok(())
    }
    fn reset(&mut self, _: &Value, _: u64, _: u64) -> Result<Value> {
        self.cancelled.set(true);
        Ok(json!({"content": [], "terminated": false, "episode_truncated": false}))
    }

    fn close(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }
}

#[test]
fn cancellation_after_open_uses_the_same_cleanup_path() {
    let schema = ContractSchema::bundled();
    let plan = plan();
    let clock = FixedClock(Cell::new(1_800_000_000_100));
    let flag = Rc::new(Cell::new(false));
    let cancellation = ToggleCancellation(flag.clone());
    let supervisor =
        EpisodeSupervisor::with_cancellation(&schema, &clock, &cancellation, capabilities(&plan));
    let mut agent = AgentProbe::default();
    let mut environment = CancellingEnvironment {
        cancelled: flag,
        closed: false,
    };
    let mut scorer = ScorerProbe::default();
    let mut backend = BackendProbe::default();
    let mut tools = ToolHostProbe::default();
    let mut model = ModelProbe::default();
    let mut artifacts = MemoryFileStore::default();
    let result = supervisor
        .execute(
            &dispatch(&plan),
            &mut agent,
            &mut environment,
            Some(&mut scorer),
            &mut backend,
            &mut tools,
            &mut model,
            &mut artifacts,
        )
        .unwrap();
    assert_eq!(result["execution_status"], "cancelled");
    assert!(agent.closed && environment.closed && backend.closed && tools.closed);
    assert_eq!(scorer.calls, 0);
}

#[test]
fn collection_uses_the_same_lifecycle_with_optional_scoring() {
    let schema = ContractSchema::bundled();
    for scored in [false, true] {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let name = if scored {
            "gsm8k_collection_scored"
        } else {
            "gsm8k_collection"
        };
        let plan: Value = serde_json::from_slice(
            &fs::read(root.join(format!(
                "reference/generated/episodes/{name}/execution_plan.json"
            )))
            .unwrap(),
        )
        .unwrap();
        let clock = FixedClock(Cell::new(1_800_000_000_100));
        let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
        let mut agent = AgentProbe::default();
        let mut environment = EnvironmentProbe::default();
        let mut scorer = ScorerProbe::default();
        let mut backend = BackendProbe::default();
        let mut tools = ToolHostProbe::default();
        let mut model = ModelProbe::default();
        let mut artifacts = MemoryFileStore::default();
        let host = if scored {
            Some(&mut scorer as &mut dyn ScorerHost)
        } else {
            None
        };
        let result = supervisor
            .execute(
                &dispatch(&plan),
                &mut agent,
                &mut environment,
                host,
                &mut backend,
                &mut tools,
                &mut model,
                &mut artifacts,
            )
            .unwrap();
        assert_eq!(result["execution_status"], "completed");
        assert_eq!(result.get("score").is_some(), scored);
        assert_eq!(scorer.calls, usize::from(scored));
        assert_eq!(environment.snapshot_calls, usize::from(scored));
        assert_eq!(scorer.closed, scored);
        assert!(result.get("final_answer").is_some());
        assert!(agent.closed && environment.closed && tools.closed && backend.closed);
        assert!(backend.frozen && tools.frozen);
        let manifest: Value =
            serde_json::from_slice(&artifacts.read(&result["trajectory_ref"]).unwrap()).unwrap();
        assert_eq!(manifest["trajectory_status"], "final_complete");
        let mut events = Vec::<Value>::new();
        for segment in manifest["event_segments"].as_array().unwrap() {
            let bytes = artifacts.read(segment).unwrap();
            for line in std::str::from_utf8(&bytes).unwrap().lines() {
                events.push(serde_json::from_str(line).unwrap());
            }
        }
        assert_eq!(
            events.iter().filter(|e| e["kind"] == "score").count(),
            usize::from(scored)
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e.pointer("/payload/phase") == Some(&json!("scoring")))
                .count(),
            usize::from(scored)
        );
        assert!(events.iter().any(|e| e["kind"] == "generation"));
        assert_eq!(events.last().unwrap()["kind"], "terminal");
        let mut invalid = result.clone();
        if scored {
            invalid.as_object_mut().unwrap().remove("score");
            assert_eq!(
                validate_result_for_plan(&invalid, &plan, &schema)
                    .unwrap_err()
                    .code,
                "MISSING_SUCCESSFUL_SCORE"
            );
        } else {
            invalid["score"] = json!({"reward": 0});
            assert_eq!(
                validate_result_for_plan(&invalid, &plan, &schema)
                    .unwrap_err()
                    .code,
                "UNEXPECTED_SCORE"
            );
        }
    }
}

#[test]
fn collection_scoring_failure_preserves_trace_and_is_not_success() {
    let schema = ContractSchema::bundled();
    let mut plan = plan();
    plan["purpose"] = json!("trajectory_collection");
    let plan = seal_plan(&plan).unwrap();
    let clock = FixedClock(Cell::new(1_800_000_000_100));
    let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
    let mut agent = AgentProbe::default();
    let mut environment = EnvironmentProbe::default();
    let mut scorer = FailingScorer { closed: false };
    let mut backend = BackendProbe::default();
    let mut tools = ToolHostProbe::default();
    let mut model = ModelProbe::default();
    let mut artifacts = MemoryFileStore::default();
    let result = supervisor
        .execute(
            &dispatch(&plan),
            &mut agent,
            &mut environment,
            Some(&mut scorer),
            &mut backend,
            &mut tools,
            &mut model,
            &mut artifacts,
        )
        .unwrap();
    assert_eq!(result["execution_status"], "failed");
    assert_eq!(result["score"]["status"], "error");
    assert!(result["score"]["reward"].is_null());
    assert!(result.get("final_answer").is_some());
    let manifest: Value =
        serde_json::from_slice(&artifacts.read(&result["trajectory_ref"]).unwrap()).unwrap();
    assert_eq!(manifest["trajectory_status"], "final_complete");
    assert!(scorer.closed && backend.closed);
}

#[derive(Default)]
struct ToolCallingModel {
    requests: Vec<Value>,
    unselected: bool,
}

impl ModelProvider for ToolCallingModel {
    fn generate(&mut self, request: &Value) -> Result<Value> {
        self.requests.push(request.clone());
        let mut event = json!({
            "generation_id": request["generation_id"], "model_id": request["model"]["model_id"],
            "source": request["model"]["source"], "messages": request["messages"],
            "response": [], "output_token_count": 1, "finish_reason": "tool_calls", "duration_ms": 1,
        });
        if self.requests.len() == 1 {
            let tool = &request["tools"][0];
            event["tool_calls"] = json!([{
                "tool_call_id": "model-call-1", "generation_id": request["generation_id"],
                "name": if self.unselected { json!("unselected") } else { tool["name"].clone() },
                "implementation": tool["implementation"],
                "arguments": {},
                "timeout_ms": 1000,
            }]);
        } else {
            event["response"] = json!([{"kind": "text", "text": "done"}]);
            event["finish_reason"] = json!("stop");
        }
        Ok(event)
    }
}

struct ToolLoopAgent;
impl AgentHost for ToolLoopAgent {
    fn prepare(&mut self, _: &Value, tools: &[Value], _: &str, _: u64) -> Result<Vec<Value>> {
        Ok(tools.to_vec())
    }
    fn run_agent(
        &mut self,
        _: &Value,
        observation: &Value,
        runtime: &mut AgentRuntime<'_>,
        _: u64,
    ) -> Result<Value> {
        let mut messages = json!([{"role": "user", "content": observation["content"]}]);
        let first = runtime.generate(&messages)?;
        let result = runtime.step(&first["tool_calls"][0])?;
        messages.as_array_mut().unwrap().extend([
            json!({"role": "assistant", "content": first["response"], "tool_calls": first["tool_calls"]}),
            json!({"role": "tool", "content": result["observation"]["content"], "tool_call_id": result["tool_call_id"]}),
        ]);
        match runtime.generate(&messages) {
            Ok(second) => Ok(second["response"].clone()),
            Err(error) if error.code == "GENERATION_LIMIT" => Ok(json!([])),
            Err(error) => Err(error),
        }
    }
    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

#[test]
fn multigeneration_tools_use_one_worker_budget_and_selected_bindings() {
    let schema = ContractSchema::bundled();
    for (cap, unselected) in [(1, false), (2, false), (2, true)] {
        let mut plan = plan_with_test_tool();
        plan["limits"]["max_generations"] = json!(cap);
        let plan = seal_plan(&plan).unwrap();
        let clock = FixedClock(Cell::new(1_800_000_000_100));
        let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
        let mut agent = ToolLoopAgent;
        let mut environment = EnvironmentProbe::default();
        let mut scorer = ScorerProbe::default();
        let mut backend = BackendProbe::default();
        let mut tools = ToolHostProbe::default();
        let mut model = ToolCallingModel {
            unselected,
            ..Default::default()
        };
        let mut artifacts = MemoryFileStore::default();
        let result = supervisor
            .execute(
                &dispatch(&plan),
                &mut agent,
                &mut environment,
                Some(&mut scorer),
                &mut backend,
                &mut tools,
                &mut model,
                &mut artifacts,
            )
            .unwrap();
        assert_eq!(model.requests[0]["tools"], plan["tools"]);
        if unselected {
            assert_eq!(result["execution_status"], "failed");
            assert_eq!(result["error"]["code"], "TOOL_NOT_SELECTED");
            assert_eq!(result["usage"]["tool_call_count"], 0);
            assert_eq!(scorer.calls, 0);
        } else {
            assert_eq!(result["execution_status"], "completed");
            assert_eq!(model.requests.len(), cap as usize);
            assert_eq!(result["usage"]["generation_count"], cap);
            assert_eq!(result["usage"]["tool_call_count"], 1);
            assert_eq!(
                result["termination_reason"],
                if cap == 1 {
                    "budget_exhausted"
                } else {
                    "final_answer"
                }
            );
            assert_eq!(scorer.calls, 1);
            if cap == 2 {
                assert_eq!(
                    model.requests[1]["messages"][2]["tool_call_id"],
                    "model-call-1"
                );
            }
        }
        assert!(backend.closed && environment.closed && tools.closed);
    }
}

#[test]
fn inconsistent_completion_and_model_identity_fail_before_scoring() {
    for (agent_lies, model_changes, code) in [
        (true, false, "INVALID_FINAL_ANSWER"),
        (false, true, "MODEL_IDENTITY_MISMATCH"),
    ] {
        let plan = plan();
        let schema = ContractSchema::bundled();
        let clock = FixedClock(Cell::new(1));
        let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
        let mut agent = AgentProbe {
            claim_environment_terminal: agent_lies,
            ..Default::default()
        };
        let mut environment = EnvironmentProbe::default();
        let mut model = ModelProbe {
            wrong_identity: model_changes,
            ..Default::default()
        };
        let mut scorer = ScorerProbe::default();
        let mut backend = BackendProbe::default();
        let mut tools = ToolHostProbe::default();
        let mut files = MemoryFileStore::default();
        let result = supervisor
            .execute(
                &dispatch(&plan),
                &mut agent,
                &mut environment,
                Some(&mut scorer),
                &mut backend,
                &mut tools,
                &mut model,
                &mut files,
            )
            .unwrap();
        assert_eq!(result["execution_status"], "failed");
        assert_eq!(result["error"]["code"], code);
        let manifest: Value =
            serde_json::from_slice(&files.read(&result["trajectory_ref"]).unwrap()).unwrap();
        assert_eq!(
            manifest["trajectory_status"],
            if model_changes {
                "final_partial"
            } else {
                "final_complete"
            }
        );
        assert_eq!(scorer.calls, 0);
        assert!(agent.closed && environment.closed && backend.closed && tools.closed);
    }
}

#[test]
fn worker_owns_stop_reason_and_never_substitutes_a_model_response() {
    struct ExplicitAnswerAgent;
    impl AgentHost for ExplicitAnswerAgent {
        fn prepare(&mut self, _: &Value, tools: &[Value], _: &str, _: u64) -> Result<Vec<Value>> {
            Ok(tools.to_vec())
        }
        fn run_agent(
            &mut self,
            _: &Value,
            observation: &Value,
            runtime: &mut AgentRuntime<'_>,
            _: u64,
        ) -> Result<Value> {
            if observation["terminated"] != true && observation["episode_truncated"] != true {
                runtime.generate(&json!([{"role": "user", "content": observation["content"]}]))?;
            }
            // An explicit empty submission must stay empty even after a nonempty response.
            Ok(json!([]))
        }
        fn close(&mut self) -> Result<()> {
            Ok(())
        }
    }
    for (terminal, truncated, limited, expected) in [
        (true, false, false, "environment_terminal"),
        (false, true, false, "environment_truncated"),
        (false, false, true, "budget_exhausted"),
        (false, false, false, "final_answer"),
    ] {
        let plan = plan();
        let schema = ContractSchema::bundled();
        let clock = FixedClock(Cell::new(1_800_000_000_100));
        let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
        let mut environment = EnvironmentProbe {
            reset_terminal: terminal,
            reset_truncated: truncated,
            ..Default::default()
        };
        let mut scorer = ScorerProbe::default();
        let mut model = ModelProbe {
            length_limited: limited,
            ..Default::default()
        };
        let result = supervisor
            .execute(
                &dispatch(&plan),
                &mut ExplicitAnswerAgent,
                &mut environment,
                Some(&mut scorer),
                &mut BackendProbe::default(),
                &mut ToolHostProbe::default(),
                &mut model,
                &mut MemoryFileStore::default(),
            )
            .unwrap();
        assert_eq!(result["execution_status"], "completed");
        assert_eq!(result["termination_reason"], expected);
        assert_eq!(result["final_answer"], json!([]));
        assert_eq!(model.requests.len(), usize::from(!terminal && !truncated));
        assert!(result.get("state").is_none());
        assert_eq!(scorer.calls, 1);
    }
}

#[test]
fn text_and_patch_files_share_final_answer_without_an_artifacts_output() {
    struct SubmissionAgent(Value);
    impl AgentHost for SubmissionAgent {
        fn prepare(&mut self, _: &Value, tools: &[Value], _: &str, _: u64) -> Result<Vec<Value>> {
            Ok(tools.to_vec())
        }
        fn run_agent(
            &mut self,
            _: &Value,
            _: &Value,
            _: &mut AgentRuntime<'_>,
            _: u64,
        ) -> Result<Value> {
            Ok(self.0.clone())
        }
        fn close(&mut self) -> Result<()> {
            Ok(())
        }
    }
    for case in ["text", "file", "mixed", "invalid_file"] {
        let plan = plan();
        let schema = ContractSchema::bundled();
        let clock = FixedClock(Cell::new(1_800_000_000_100));
        let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
        let mut files = MemoryFileStore::default();
        let mut reference = files
            .put_bytes(b"--- a/file.py\n+++ b/file.py\n", "text/plain")
            .unwrap();
        if case == "invalid_file" {
            reference["size_bytes"] = json!(9999);
        }
        let text = json!({"kind": "text", "text": "--- a/file.py\n+++ b/file.py\n"});
        let file = json!({"kind": "artifact", "artifact": reference});
        let final_answer = match case {
            "text" => json!([text]),
            "mixed" => json!([text, file]),
            _ => json!([file]),
        };
        let mut agent = SubmissionAgent(final_answer.clone());
        let mut scorer = ScorerProbe::default();
        let mut environment = EnvironmentProbe::default();
        let mut backend = BackendProbe::default();
        let mut tools = ToolHostProbe::default();
        let result = supervisor
            .execute(
                &dispatch(&plan),
                &mut agent,
                &mut environment,
                Some(&mut scorer),
                &mut backend,
                &mut tools,
                &mut ModelProbe::default(),
                &mut files,
            )
            .unwrap();
        if case == "invalid_file" {
            assert_eq!(result["execution_status"], "failed");
            assert_eq!(result["error"]["code"], "ARTIFACT_INTEGRITY_MISMATCH");
            assert!(result.get("final_answer").is_none());
            assert_eq!(scorer.calls, 0);
        } else {
            assert_eq!(result["execution_status"], "completed");
            assert_eq!(result["final_answer"], final_answer);
            assert_eq!(scorer.final_answer, final_answer);
            let mut old_result = result.clone();
            old_result["artifacts"] = json!([]);
            assert!(validate_result_for_plan(&old_result, &plan, &schema).is_err());
        }
        assert!(result.get("artifacts").is_none());
        assert!(backend.closed && tools.closed && environment.closed);
    }
}

#[test]
fn structured_observation_is_preserved_while_model_trace_records_json_text() {
    let plan = plan();
    let schema = ContractSchema::bundled();
    let clock = FixedClock(Cell::new(1_800_000_000_100));
    let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
    let mut environment = EnvironmentProbe {
        structured_observation: true,
        ..Default::default()
    };
    let mut model = ModelProbe::default();
    let mut files = MemoryFileStore::default();
    let result = supervisor
        .execute(
            &dispatch(&plan),
            &mut AgentProbe::default(),
            &mut environment,
            Some(&mut ScorerProbe::default()),
            &mut BackendProbe::default(),
            &mut ToolHostProbe::default(),
            &mut model,
            &mut files,
        )
        .unwrap();
    assert_eq!(result["execution_status"], "completed");
    assert_eq!(
        model.requests[0]["messages"][0]["content"],
        json!([
            {"kind": "text", "text": "2+3?"}, {"kind": "text", "text": "{}"}
        ])
    );
    let manifest: Value =
        serde_json::from_slice(&files.read(&result["trajectory_ref"]).unwrap()).unwrap();
    let mut events = Vec::<Value>::new();
    for segment in manifest["event_segments"].as_array().unwrap() {
        let bytes = files.read(segment).unwrap();
        for line in std::str::from_utf8(&bytes).unwrap().lines() {
            events.push(serde_json::from_str(line).unwrap());
        }
    }
    let observation = &events
        .iter()
        .find(|event| event["kind"] == "observation")
        .unwrap()["payload"];
    assert_eq!(observation["content"][1]["kind"], "structured");
    assert!(observation.get("data").is_none());
    let generation = &events
        .iter()
        .find(|event| event["kind"] == "generation")
        .unwrap()["payload"];
    assert_eq!(generation["messages"], model.requests[0]["messages"]);
    let mut invalid = observation.clone();
    invalid["data"] = json!({});
    assert!(schema.validate_shape("Observation", &invalid).is_err());
    let mut mixed = observation["content"][1].clone();
    mixed["text"] = json!("duplicate");
    assert!(schema.validate_shape("ContentPart", &mixed).is_err());
}

#[test]
fn malformed_or_overlong_model_results_fail_before_scoring() {
    struct InvalidModel {
        mode: u8,
    }
    impl ModelProvider for InvalidModel {
        fn generate(&mut self, request: &Value) -> Result<Value> {
            let mut event = ModelProbe::default().generate(request)?;
            match self.mode {
                0 => {
                    event["output_token_count"] = json!(2);
                    event["output_token_ids"] = json!([3, 4]);
                }
                1 => event["response"] = json!([{"kind":"text", "text":42}]),
                2 => event["finish_reason"] = json!("invented"),
                3 => event["output_token_ids"] = json!([true]),
                4 => event["output_logprobs"] = json!(["wrong"]),
                _ => event["loss_mask"] = json!([2]),
            }
            Ok(event)
        }
    }
    for mode in 0..6 {
        let mut plan = plan();
        plan["model"]["generation"]["max_output_tokens"] = json!(1);
        let plan = seal_plan(&plan).unwrap();
        let schema = ContractSchema::bundled();
        let clock = FixedClock(Cell::new(1_800_000_000_100));
        let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
        let mut scorer = ScorerProbe::default();
        let mut files = MemoryFileStore::default();
        let mut backend = BackendProbe::default();
        let result = supervisor
            .execute(
                &dispatch(&plan),
                &mut AgentProbe::default(),
                &mut EnvironmentProbe::default(),
                Some(&mut scorer),
                &mut backend,
                &mut ToolHostProbe::default(),
                &mut InvalidModel { mode },
                &mut files,
            )
            .unwrap();
        assert_eq!(result["execution_status"], "failed");
        assert_eq!(scorer.calls, 0);
        assert!(backend.closed);
        let manifest: Value =
            serde_json::from_slice(&files.read(&result["trajectory_ref"]).unwrap()).unwrap();
        assert_eq!(manifest["trajectory_status"], "final_partial");
        if mode == 0 {
            assert_eq!(result["error"]["code"], "MODEL_OUTPUT_LIMIT_EXCEEDED");
            assert_eq!(result["usage"]["output_token_count"], 2);
        }
    }
}

#[test]
fn invalid_metric_values_never_become_successful_scores() {
    struct MetricScorer(Value);
    impl ScorerHost for MetricScorer {
        fn score(
            &mut self,
            _: &Value,
            _: &Value,
            _: &Value,
            _: &mut dyn ScoringContext,
        ) -> Result<Value> {
            Ok(json!({"success":true,"reward":1,"metrics":[self.0],"evidence":[]}))
        }
        fn close(&mut self) -> Result<()> {
            Ok(())
        }
    }
    let plan = plan();
    let request = json!({"task":plan["task"], "final_answer":[],
        "trajectory_ref":{"uri":"memory://x","digest":format!("sha256:{}","0".repeat(64)),"size_bytes":0,"media_type":"application/json"}});
    let good = json!({"name":"accuracy","value":1,"unit":"ratio","direction":"higher"});
    for (field, value) in [
        ("value", json!("1")),
        ("direction", json!("up")),
        ("unit", Value::Null),
        ("extra", json!(1)),
    ] {
        let mut metric = good.clone();
        metric[field] = value;
        let score = run_score(
            &ContractSchema::bundled(),
            &mut MetricScorer(metric),
            &plan["dataset_package"],
            &plan["scoring"]["config"],
            &request,
            &mut NoServices,
        );
        assert_eq!(score["status"], "error");
        assert_eq!(score["reward"], Value::Null);
        assert_eq!(score["metrics"], json!([]));
    }
    let score = run_score(
        &ContractSchema::bundled(),
        &mut MetricScorer(good),
        &plan["dataset_package"],
        &plan["scoring"]["config"],
        &request,
        &mut NoServices,
    );
    assert_eq!(score["status"], "ok");
}

#[test]
fn harness_uses_frozen_inputs_and_current_worker_budget() {
    use uenv_reference_control::ports::NEVER_CANCELLED;
    use uenv_reference_control::scoring::BackendScoringContext;
    struct HarnessBackend {
        calls: Vec<Value>,
    }
    impl Backend for HarnessBackend {
        fn open(&mut self, _: &Value, _: Option<&Value>, _: bool, _: u64) -> Result<Value> {
            unreachable!()
        }
        fn freeze(&mut self, _: u64) -> Result<()> {
            Ok(())
        }
        fn close(&mut self) -> Result<()> {
            Ok(())
        }
        fn run_harness(&mut self, request: &Value) -> Result<Value> {
            self.calls.push(request.clone());
            Ok(json!({}))
        }
    }
    let input = json!({"final_answer":[{"kind":"text","text":"candidate"}],
        "private_data":{"schema_ref":"urn:test:private", "data":{"evaluation_plan":{"harness":"official","timeout_ms":400}}}});
    let mut request = input.clone();
    request["remaining_timeout_ms"] = json!(9999);
    let mut backend = HarnessBackend { calls: vec![] };
    let clock = FixedClock(Cell::new(100));
    let files = MemoryFileStore::default();
    {
        let mut context = BackendScoringContext::new(
            &mut backend,
            &files,
            &clock,
            &NEVER_CANCELLED,
            1000,
            &input,
        );
        context.run_harness(&request).unwrap();
        clock.0.set(900);
        context.run_harness(&request).unwrap();
        for field in ["final_answer", "private_data", "state"] {
            let mut changed = request.clone();
            changed[field] = json!({});
            assert_eq!(
                context.run_harness(&changed).unwrap_err().code,
                "HARNESS_INPUT_MISMATCH"
            );
        }
        clock.0.set(1000);
        assert_eq!(
            context.run_harness(&request).unwrap_err().code,
            "SCORER_TIMEOUT"
        );
    }
    assert_eq!(backend.calls.len(), 2);
    assert_eq!(backend.calls[0]["remaining_timeout_ms"], 400);
    assert_eq!(backend.calls[1]["remaining_timeout_ms"], 100);
    assert_eq!(backend.calls[1]["final_answer"], input["final_answer"]);
}

#[test]
fn failed_trajectory_append_does_not_reuse_its_sequence() {
    use uenv_reference_control::runtime::TrajectoryWriter;
    let mut writer = TrajectoryWriter::from_plan(&plan(), &ContractSchema::bundled()).unwrap();
    writer
        .record("state", json!({"phase":"preparing"}), 1)
        .unwrap();
    assert!(
        writer
            .record(
                "observation",
                json!({"content":[],"terminated":false,"episode_truncated":false,"data":{}}),
                2
            )
            .is_err()
    );
    writer
        .record("state", json!({"phase":"cleaning"}), 3)
        .unwrap();
    assert_eq!(writer.events()[0]["sequence"], 0);
    assert_eq!(writer.events()[1]["sequence"], 2);
    let mut files = MemoryFileStore::default();
    let reference = writer.seal(4, &mut files).unwrap();
    let manifest: Value = serde_json::from_slice(&files.read(&reference).unwrap()).unwrap();
    assert_eq!(manifest["event_count"], 2);
    assert_eq!(manifest["trajectory_status"], "final_partial");
}

#[test]
fn process_scores_are_validated_once_and_preserved_in_result_and_trace() {
    for case in ["valid", "unknown", "duplicate", "null", "extra"] {
        let schema = ContractSchema::bundled();
        let plan = plan();
        let clock = FixedClock(Cell::new(1_800_000_000_100));
        let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
        let mut scorer = ScorerProbe {
            process_score_case: Some(case),
            ..Default::default()
        };
        let mut files = MemoryFileStore::default();
        let result = supervisor
            .execute(
                &dispatch(&plan),
                &mut AgentProbe::default(),
                &mut EnvironmentProbe::default(),
                Some(&mut scorer),
                &mut BackendProbe::default(),
                &mut ToolHostProbe::default(),
                &mut ModelProbe::default(),
                &mut files,
            )
            .unwrap();
        assert_eq!(scorer.calls, 1);
        if case == "valid" {
            assert_eq!(result["score"]["status"], "ok");
            assert_eq!(result["score"]["reward"], 1.0);
            assert_eq!(result["score"]["generation_rewards"][0]["reward"], -0.25);
        } else {
            assert_eq!(result["score"]["status"], "error", "{case}");
            assert!(result["score"]["reward"].is_null());
            assert!(result["score"].get("generation_rewards").is_none());
        }
        let manifest: Value =
            serde_json::from_slice(&files.read(&result["trajectory_ref"]).unwrap()).unwrap();
        let mut scores = vec![];
        for segment in manifest["event_segments"].as_array().unwrap() {
            for line in String::from_utf8(files.read(segment).unwrap())
                .unwrap()
                .lines()
            {
                let event: Value = serde_json::from_str(line).unwrap();
                if event["kind"] == "score" {
                    scores.push(event["payload"].clone());
                }
            }
        }
        assert_eq!(scores, vec![result["score"].clone()]);
    }
}

#[test]
fn training_model_must_return_the_requested_policy_and_valid_version() {
    struct VersionModel {
        wrong: bool,
    }
    impl ModelProvider for VersionModel {
        fn generate(&mut self, request: &Value) -> Result<Value> {
            assert_eq!(request["training"]["requested_policy_version"], "policy-1");
            let mut event = ModelProbe::default().generate(request)?;
            event["policy_version"] = json!(if self.wrong { "policy-2" } else { "policy-1" });
            event["parameter_version"] = json!(12);
            Ok(event)
        }
    }
    for wrong in [false, true] {
        let mut plan = plan();
        plan["purpose"] = json!("training");
        plan["training"] = json!({"parallel_mode":"sync","require_token_trace":false,"version_policy":"fixed_episode","requested_policy_version":"policy-1","max_policy_lag":0});
        let plan = seal_plan(&plan).unwrap();
        let schema = ContractSchema::bundled();
        let clock = FixedClock(Cell::new(1_800_000_000_100));
        let mut scorer = ScorerProbe::default();
        let mut files = MemoryFileStore::default();
        let result = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan))
            .execute(
                &dispatch(&plan),
                &mut AgentProbe::default(),
                &mut EnvironmentProbe::default(),
                Some(&mut scorer),
                &mut BackendProbe::default(),
                &mut ToolHostProbe::default(),
                &mut VersionModel { wrong },
                &mut files,
            )
            .unwrap();
        assert_eq!(
            result["execution_status"],
            if wrong { "failed" } else { "completed" },
            "{result}"
        );
        assert_eq!(scorer.calls, if wrong { 0 } else { 1 });
    }
}

#[test]
fn scorer_can_read_only_bound_artifacts_and_verified_child_references() {
    use uenv_reference_control::scoring::BackendScoringContext;
    let mut files = MemoryFileStore::default();
    let child = files
        .put_bytes(b"allowed segment", "application/x-ndjson")
        .unwrap();
    let root = files.put_json(&json!({"event_segments":[child]})).unwrap();
    let other = files
        .put_bytes(b"another episode secret", "text/plain")
        .unwrap();
    let input = json!({"trajectory_ref":root});
    let clock = FixedClock(Cell::new(100));
    let mut backend = BackendProbe::default();
    let context = BackendScoringContext::new(
        &mut backend,
        &files,
        &clock,
        &uenv_reference_control::ports::NEVER_CANCELLED,
        1000,
        &input,
    );
    assert_eq!(
        context.read_artifact(&other).unwrap_err().code,
        "SCORING_ARTIFACT_ACCESS_DENIED"
    );
    assert!(context.read_artifact(&child).is_err());
    context.read_artifact(&root).unwrap();
    assert_eq!(context.read_artifact(&child).unwrap(), b"allowed segment");
}
