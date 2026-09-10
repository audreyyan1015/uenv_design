use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Value, json};
use uenv_reference_control::contracts::ContractSchema;
use uenv_reference_control::plan::{seal_plan, validate_result_for_plan};
use uenv_reference_control::ports::{
    AgentHost, ArtifactStore, Backend, Cancellation, Clock, EnvironmentHost, MemoryArtifactStore,
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
        "adapter": {"id": "adapters/hidden", "version": "1", "digest": format!("sha256:{}", "1".repeat(64))},
        "config": {"schema_ref": "uenv://schemas/vnext/EmptyConfig", "data": {}},
        "interface": "mcp.v1",
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

    fn call_tool(&mut self, _: &Value, call: &Value) -> Result<Value> {
        if self.fail_call {
            return Err(ControlError::new("TOOL_PROCESS_FAILED"));
        }
        Ok(json!({
            "tool_call_id": call["tool_call_id"],
            "status": "ok",
            "content": [{"kind": "text", "text": "ok"}],
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
}

impl ModelProvider for ModelProbe {
    fn generate(&mut self, request: &Value) -> Result<Value> {
        self.requests.push(request.clone());
        Ok(json!({
            "generation_id": request["generation_id"],
            "model_id": request["model"]["model_id"],
            "source": request["model"]["source"],
            "messages": request["messages"],
            "response": [{"kind": "text", "text": "5"}],
            "output_token_count": 1,
            "input_token_ids": [1, 2],
            "output_token_ids": [3],
            "finish_reason": "stop",
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
}

impl AgentHost for AgentProbe {
    fn prepare(&mut self, _: &Value, tools: &[Value], timeout_ms: u64) -> Result<Vec<Value>> {
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
        Ok(json!({
            "final_answer": generation["response"],
            "artifacts": [],
            "termination_reason": if self.finish_at.is_some() { "budget_exhausted" } else { "final_answer" }
        }))
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
    steps: usize,
    prepare_timeout_ms: Option<u64>,
    reset_timeout_ms: Option<u64>,
    finalize_timeout_ms: Option<u64>,
    lifecycle: Option<Lifecycle>,
}

impl EnvironmentHost for EnvironmentProbe {
    fn prepare(&mut self, _: &Value, _: &Value, timeout_ms: u64) -> Result<()> {
        mark(&self.lifecycle, "environment.prepare");
        self.prepare_timeout_ms = Some(timeout_ms);
        Ok(())
    }

    fn reset(&mut self, _: &Value, _: u64, timeout_ms: u64) -> Result<Value> {
        mark(&self.lifecycle, "environment.reset");
        self.reset_timeout_ms = Some(timeout_ms);
        Ok(json!({"content": [{"kind": "text", "text": "2+3?"}]}))
    }

    fn step(&mut self, _: &Value, _: u64) -> Result<Value> {
        self.steps += 1;
        Ok(json!({
            "observation": {"content": [{"kind": "text", "text": "updated"}]},
            "terminated": false,
            "episode_truncated": false
        }))
    }

    fn finalize(&mut self, outcome: &Value, timeout_ms: u64) -> Result<Value> {
        mark(&self.lifecycle, "environment.finalize");
        self.finalize_timeout_ms = Some(timeout_ms);
        let mut frozen = outcome.as_object().unwrap().clone();
        frozen.insert(
            "state".to_owned(),
            json!({"schema_ref": "uenv://schemas/vnext/EmptyConfig", "data": {}}),
        );
        Ok(Value::Object(frozen))
    }

    fn close(&mut self) -> Result<()> {
        mark(&self.lifecycle, "environment.close");
        self.closed = true;
        Ok(())
    }
}

#[derive(Default)]
struct ScorerProbe {
    calls: usize,
    saw_private_data: bool,
    trajectory_root_is_manifest: bool,
    closed: bool,
    lifecycle: Option<Lifecycle>,
}

impl ScorerHost for ScorerProbe {
    fn score(
        &mut self,
        _: &Value,
        request: &Value,
        context: &mut dyn ScoringContext,
    ) -> Result<Value> {
        mark(&self.lifecycle, "scorer.score");
        self.calls += 1;
        self.saw_private_data = request.get("private_data").is_some();
        let bytes = context.read_artifact(&request["trajectory_ref"])?;
        let root: Value = serde_json::from_slice(&bytes).unwrap();
        self.trajectory_root_is_manifest = root.get("event_segments").is_some();
        Ok(json!({
            "success": true,
            "metrics": [{"name": "accuracy", "value": 1.0, "unit": "ratio", "direction": "higher"}],
            "reward": 1.0,
            "evidence": []
        }))
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
            "environment_step_count": 0,
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
    let mut artifacts = MemoryArtifactStore::default();

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
    assert_eq!(result["score"]["scorer"], plan["scorer"]["implementation"]);
    assert_eq!(scorer.calls, 1);
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
            environment.finalize_timeout_ms,
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
            "environment.finalize",
            "tools.freeze",
            "backend.freeze",
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
    fn prepare(&mut self, _: &Value, tools: &[Value], _: u64) -> Result<Vec<Value>> {
        Ok(tools.to_vec())
    }

    fn run_agent(
        &mut self,
        _: &Value,
        _: &Value,
        runtime: &mut AgentRuntime<'_>,
        _: u64,
    ) -> Result<Value> {
        let transition = runtime.step(&json!({
            "schema_ref": "uenv://schemas/vnext/AnswerAction",
            "data": {"answer": "5"}
        }))?;
        Ok(json!({
            "final_answer": transition["observation"]["content"],
            "artifacts": [],
            "termination_reason": "final_answer"
        }))
    }

    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

struct ToolCallingAgent;

impl AgentHost for ToolCallingAgent {
    fn prepare(&mut self, _: &Value, tools: &[Value], _: u64) -> Result<Vec<Value>> {
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
        runtime.call_tool(&json!({
            "tool_call_id": "tool-call-1",
            "generation_id": generation["generation_id"],
            "implementation": {
                "id": "tools/test",
                "version": "1",
                "digest": format!("sha256:{}", "2".repeat(64))
            },
            "name": "test_tool",
            "arguments": {
                "schema_ref": "uenv://schemas/vnext/EmptyConfig",
                "data": {}
            },
            "timeout_ms": 1_000
        }))?;
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
        "adapter": {
            "id": "adapters/test",
            "version": "1",
            "digest": format!("sha256:{}", "3".repeat(64))
        },
        "config": {
            "schema_ref": "uenv://schemas/vnext/ToolConfig",
            "data": {"timeout_ms": 1_000, "max_preview_bytes": 1024}
        },
        "interface": "uenv_direct.v1",
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
    let mut artifacts = MemoryArtifactStore::default();
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
    assert_eq!(result["outcome"]["termination_reason"], "budget_exhausted");
    assert_eq!(environment.finalize_timeout_ms, Some(reserve));
    assert_eq!(scorer.calls, 1);
    now.set(deadline);
    assert_eq!(budget.finalize_deadline_ms(), deadline);
    assert_eq!(
        budget.remaining_finalize_ms(&clock).unwrap_err().code,
        "EPISODE_TIMEOUT"
    );
}

#[test]
fn environment_step_is_gated_and_recorded_by_agent_runtime() {
    let schema = ContractSchema::bundled();
    let mut changed = plan();
    changed["limits"]["max_environment_steps"] = json!(4);
    let plan = seal_plan(&changed).unwrap();
    let clock = FixedClock(Cell::new(1_800_000_000_100));
    let supervisor = EpisodeSupervisor::new(&schema, &clock, capabilities(&plan));
    let mut agent = SteppingAgent;
    let mut environment = EnvironmentProbe::default();
    let mut scorer = ScorerProbe::default();
    let mut backend = BackendProbe::default();
    let mut tools = ToolHostProbe::default();
    let mut model = ModelProbe::default();
    let mut artifacts = MemoryArtifactStore::default();

    let mut retry_dispatch = dispatch(&plan);
    retry_dispatch["consumed_usage"]["environment_step_count"] = json!(3);
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
    assert_eq!(result["usage"]["environment_step_count"], 4);
    assert_eq!(environment.steps, 1);

    let manifest: Value =
        serde_json::from_slice(&artifacts.read(&result["trajectory_ref"]).unwrap()).unwrap();
    let events = artifacts.read(&manifest["event_segments"][0]).unwrap();
    let events = std::str::from_utf8(&events).unwrap();
    assert!(events.contains("environment_transition"));
    assert!(events.contains("\"environment_step_index\":0"));
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
    let mut model = ModelProbe::default();
    let mut artifacts = MemoryArtifactStore::default();

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
    assert_eq!(result["error"]["operation_id"], "tool-call-1");

    let manifest: Value =
        serde_json::from_slice(&artifacts.read(&result["trajectory_ref"]).unwrap()).unwrap();
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
        "tool-call-1"
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
    let mut artifacts = MemoryArtifactStore::default();

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
    fn score(&mut self, _: &Value, _: &Value, _: &mut dyn ScoringContext) -> Result<Value> {
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
        "outcome": {"final_answer": [], "artifacts": [], "termination_reason": "final_answer"},
        "trajectory_ref": {"uri": "memory://x", "digest": format!("sha256:{}", "0".repeat(64)), "size_bytes": 0, "media_type": "application/json"},
        "private_data": plan["private_data"]
    });
    let score = run_score(
        &ContractSchema::bundled(),
        &mut ScorerProbe::default(),
        &plan["scorer"],
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
        "outcome": {"final_answer": [], "artifacts": [], "termination_reason": "final_answer"},
        "trajectory_ref": {"uri": "memory://x", "digest": format!("sha256:{}", "0".repeat(64)), "size_bytes": 0, "media_type": "application/json"},
        "private_data": plan["private_data"]
    });
    let score = run_score(
        &ContractSchema::bundled(),
        &mut ForgingScorer,
        &plan["scorer"],
        &request,
        &mut NoServices,
    );
    assert_eq!(score["status"], "error");
    assert_eq!(score["reward"], Value::Null);
    assert_eq!(score["scorer"], plan["scorer"]["implementation"]);
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
    fn score(&mut self, _: &Value, _: &Value, _: &mut dyn ScoringContext) -> Result<Value> {
        Err(ControlError::new("SCORER_PROCESS_CRASH"))
    }

    fn close(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }
}

#[test]
fn scorer_failure_keeps_the_frozen_outcome_and_error_score() {
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
    let mut artifacts = MemoryArtifactStore::default();

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
    assert!(result.get("outcome").is_some());
    assert_eq!(result["score"]["status"], "error");
    assert_eq!(result["score"]["error"]["code"], "SCORER_FAILED");
    assert!(scorer.closed);
}

struct FailingEnvironment {
    closed: bool,
}

impl EnvironmentHost for FailingEnvironment {
    fn prepare(&mut self, _: &Value, _: &Value, _: u64) -> Result<()> {
        Ok(())
    }
    fn reset(&mut self, _: &Value, _: u64, _: u64) -> Result<Value> {
        Err(ControlError::new("RESET_FAILED"))
    }
    fn step(&mut self, _: &Value, _: u64) -> Result<Value> {
        unreachable!()
    }
    fn finalize(&mut self, _: &Value, _: u64) -> Result<Value> {
        unreachable!()
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
    let mut artifacts = MemoryArtifactStore::default();
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
    let mut artifacts = MemoryArtifactStore::default();
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
    let mut artifacts = MemoryArtifactStore::default();
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
    fn prepare(&mut self, _: &Value, _: &Value, _: u64) -> Result<()> {
        Ok(())
    }
    fn reset(&mut self, _: &Value, _: u64, _: u64) -> Result<Value> {
        self.cancelled.set(true);
        Ok(json!({"content": []}))
    }
    fn step(&mut self, _: &Value, _: u64) -> Result<Value> {
        unreachable!()
    }
    fn finalize(&mut self, _: &Value, _: u64) -> Result<Value> {
        unreachable!()
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
    let mut artifacts = MemoryArtifactStore::default();
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
        let mut artifacts = MemoryArtifactStore::default();
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
        assert_eq!(scorer.closed, scored);
        assert!(result.get("outcome").is_some());
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
    let mut artifacts = MemoryArtifactStore::default();
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
    assert!(result.get("outcome").is_some());
    let manifest: Value =
        serde_json::from_slice(&artifacts.read(&result["trajectory_ref"]).unwrap()).unwrap();
    assert_eq!(manifest["trajectory_status"], "final_complete");
    assert!(scorer.closed && backend.closed);
}
