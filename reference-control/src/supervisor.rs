use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::contracts::{ContractSchema, array, u64_field};
use crate::plan::{validate_execution_plan, validate_result_for_plan, verify_actual_tools};
use crate::ports::{
    AgentHost, ArtifactStore, Backend, Cancellation, Clock, EnvironmentHost, ModelProvider,
    NEVER_CANCELLED, ScorerHost, ToolHost,
};
use crate::runtime::{AgentRuntime, BudgetEnforcer, TrajectoryWriter, ensure_plan_backend_started};
use crate::scoring::{BackendScoringContext, run_score};
use crate::{ControlError, Result};

pub struct EpisodeSupervisor<'a> {
    schema: &'a ContractSchema,
    clock: &'a dyn Clock,
    cancellation: &'a dyn Cancellation,
    host_capabilities: BTreeSet<String>,
}

struct CompletedAttempt {
    outcome: Value,
    score: Option<Value>,
}

impl<'a> EpisodeSupervisor<'a> {
    pub fn new(
        schema: &'a ContractSchema,
        clock: &'a dyn Clock,
        host_capabilities: BTreeSet<String>,
    ) -> Self {
        Self {
            schema,
            clock,
            cancellation: &NEVER_CANCELLED,
            host_capabilities,
        }
    }

    pub fn with_cancellation(
        schema: &'a ContractSchema,
        clock: &'a dyn Clock,
        cancellation: &'a dyn Cancellation,
        host_capabilities: BTreeSet<String>,
    ) -> Self {
        Self {
            schema,
            clock,
            cancellation,
            host_capabilities,
        }
    }

    /// The only per-attempt execution entry. All effective choices come from
    /// `dispatch.plan`; sibling fields are lease and cumulative transport state.
    /// Injected ports are process dependencies, not configuration.
    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &self,
        dispatch: &Value,
        agent_host: &mut dyn AgentHost,
        environment_host: &mut dyn EnvironmentHost,
        mut scorer_host: Option<&mut dyn ScorerHost>,
        backend: &mut dyn Backend,
        tool_host: &mut dyn ToolHost,
        model_provider: &mut dyn ModelProvider,
        artifacts: &mut dyn ArtifactStore,
    ) -> Result<Value> {
        self.schema.validate_shape("DispatchRequest", dispatch)?;
        let plan = dispatch
            .get("plan")
            .ok_or_else(|| ControlError::new("MISSING_EXECUTION_PLAN"))?;
        validate_execution_plan(plan, self.schema)?;
        if plan.get("scorer").is_some() != scorer_host.is_some() {
            return Err(ControlError::new("SCORER_HOST_MISMATCH"));
        }
        self.check_cancelled()?;
        let required: BTreeSet<String> = array(plan, "required_capabilities")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| ControlError::new("INVALID_CAPABILITY"))
            })
            .collect::<Result<_>>()?;
        if !required.is_subset(&self.host_capabilities) {
            return Err(ControlError::new("UNAVAILABLE_HOST_CAPABILITIES"));
        }

        let started_at_ms = self.clock.unix_time_ms();
        let mut budget = BudgetEnforcer::from_dispatch(dispatch, self.clock)?;
        budget.check_total(self.clock)?;
        let mut trajectory = TrajectoryWriter::from_plan(plan, self.schema)?;
        let attempt = (|| {
            // Backend creation is inside the same cleanup domain as every later phase.
            // close() is idempotent and is attempted even when open() fails part-way.
            let open_timeout_ms = budget
                .remaining_interaction_ms(self.clock)
                .map_err(|error| error.in_phase("prepare"))?;
            let session = ensure_plan_backend_started(plan, backend, open_timeout_ms)
                .map_err(|error| error.in_phase("prepare"))?;
            trajectory
                .record(
                    "state",
                    json!({"phase": "preparing", "session": session}),
                    self.clock.unix_time_ms(),
                )
                .map_err(|error| error.in_phase("persist"))?;
            let environment_prepare_timeout_ms = budget
                .remaining_interaction_ms(self.clock)
                .map_err(|error| error.in_phase("prepare"))?;
            environment_host
                .prepare(
                    &plan["environment"],
                    &session,
                    environment_prepare_timeout_ms,
                )
                .map_err(|error| error.in_phase("prepare"))?;
            self.check_cancelled()?;
            let planned_tools = array(plan, "tools")?;
            let tool_prepare_timeout_ms = budget
                .remaining_interaction_ms(self.clock)
                .map_err(|error| error.in_phase("prepare"))?;
            let routable_tools = tool_host
                .prepare(planned_tools, &session, tool_prepare_timeout_ms)
                .map_err(|error| error.in_phase("prepare"))?;
            verify_actual_tools(planned_tools, &routable_tools)
                .map_err(|error| error.in_phase("prepare"))?;
            self.check_cancelled()?;
            let agent_prepare_timeout_ms = budget
                .remaining_interaction_ms(self.clock)
                .map_err(|error| error.in_phase("prepare"))?;
            let visible_tools = agent_host
                .prepare(&plan["agent"], planned_tools, agent_prepare_timeout_ms)
                .map_err(|error| error.in_phase("prepare"))?;
            verify_actual_tools(planned_tools, &visible_tools)
                .map_err(|error| error.in_phase("prepare"))?;
            self.check_cancelled()?;
            self.run_components(
                plan,
                agent_host,
                environment_host,
                &mut scorer_host,
                backend,
                tool_host,
                model_provider,
                artifacts,
                &mut budget,
                &mut trajectory,
            )
        })();

        let (execution_status, error, completed) = match attempt {
            Ok(completed)
                if completed.score.as_ref().is_none_or(|score| {
                    score.get("status").and_then(Value::as_str) == Some("ok")
                }) =>
            {
                ("completed", None, Some(completed))
            }
            Ok(completed) => {
                let error = completed
                    .score
                    .as_ref()
                    .and_then(|score| score.get("error"))
                    .cloned()
                    .unwrap_or_else(|| error_record(&ControlError::new("SCORER_FAILED"), "score"));
                let code = error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("SCORER_FAILED")
                    .to_owned();
                let status = execution_status_for(&code);
                let _ = trajectory.record("error", error.clone(), self.clock.unix_time_ms());
                (status, Some(error), Some(completed))
            }
            Err(failure) => {
                let status = execution_status_for(&failure.code);
                let error = error_record(&failure, failure.phase.unwrap_or("agent"));
                let _ = trajectory.record("error", error.clone(), self.clock.unix_time_ms());
                (status, Some(error), None)
            }
        };

        let _ = trajectory.record(
            "state",
            json!({"phase": "cleaning"}),
            self.clock.unix_time_ms(),
        );
        let scorer_cleanup = match scorer_host.as_mut() {
            Some(host) => host.close(),
            None => Ok(()),
        };
        let agent_cleanup = agent_host.close();
        let tool_cleanup = tool_host.close();
        let environment_cleanup = environment_host.close();
        let backend_cleanup = backend.close();
        let cleanup_status = if scorer_cleanup.is_ok()
            && agent_cleanup.is_ok()
            && environment_cleanup.is_ok()
            && tool_cleanup.is_ok()
            && backend_cleanup.is_ok()
        {
            "completed"
        } else {
            let _ = trajectory.record(
                "error",
                error_record(&ControlError::new("CLEANUP_FAILED"), "cleanup"),
                self.clock.unix_time_ms(),
            );
            "failed"
        };

        let terminal = json!({
            "execution_status": execution_status,
            "usage": budget.usage().to_value(),
            "error": error,
        });
        let mut terminal = terminal.as_object().unwrap().clone();
        if terminal.get("error") == Some(&Value::Null) {
            terminal.remove("error");
        }
        let _ = trajectory.record(
            "terminal",
            Value::Object(terminal),
            self.clock.unix_time_ms(),
        );
        let trajectory_ref = trajectory.seal(self.clock.unix_time_ms(), artifacts)?;

        let mut result = Map::from_iter([
            ("run_id".to_owned(), plan["run_id"].clone()),
            ("episode_id".to_owned(), plan["episode_id"].clone()),
            ("attempt_id".to_owned(), plan["attempt_id"].clone()),
            ("task_id".to_owned(), plan["task"]["task_id"].clone()),
            (
                "execution_status".to_owned(),
                Value::String(execution_status.to_owned()),
            ),
            ("trajectory_ref".to_owned(), trajectory_ref),
            ("usage".to_owned(), budget.usage().to_value()),
            ("started_at_ms".to_owned(), json!(started_at_ms)),
            (
                "finished_at_ms".to_owned(),
                json!(self.clock.unix_time_ms()),
            ),
            (
                "cleanup_status".to_owned(),
                Value::String(cleanup_status.to_owned()),
            ),
        ]);
        if let Some(completed) = completed {
            result.insert("outcome".to_owned(), completed.outcome);
            if let Some(score) = completed.score {
                result.insert("score".to_owned(), score);
            }
        }
        if let Some(error) = error {
            result.insert("error".to_owned(), error);
        }
        let result = Value::Object(result);
        validate_result_for_plan(&result, plan, self.schema)?;
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_components(
        &self,
        plan: &Value,
        agent_host: &mut dyn AgentHost,
        environment_host: &mut dyn EnvironmentHost,
        scorer_host: &mut Option<&mut dyn ScorerHost>,
        backend: &mut dyn Backend,
        tool_host: &mut dyn ToolHost,
        model_provider: &mut dyn ModelProvider,
        artifacts: &mut dyn ArtifactStore,
        budget: &mut BudgetEnforcer,
        trajectory: &mut TrajectoryWriter,
    ) -> Result<CompletedAttempt> {
        let reset_timeout_ms = budget
            .remaining_interaction_ms(self.clock)
            .map_err(|error| error.in_phase("environment"))?;
        let observation = environment_host
            .reset(&plan["task"], u64_field(plan, "seed")?, reset_timeout_ms)
            .map_err(|error| error.in_phase("environment"))?;
        self.schema
            .validate_shape("Observation", &observation)
            .map_err(|error| error.in_phase("environment"))?;
        self.check_cancelled()?;
        trajectory
            .record(
                "observation",
                observation.clone(),
                self.clock.unix_time_ms(),
            )
            .map_err(|error| error.in_phase("persist"))?;
        trajectory
            .record(
                "state",
                json!({"phase": "running"}),
                self.clock.unix_time_ms(),
            )
            .map_err(|error| error.in_phase("persist"))?;

        let agent_timeout_ms = budget
            .remaining_interaction_ms(self.clock)
            .map_err(|error| error.in_phase("agent"))?;
        let submitted = {
            let mut runtime = AgentRuntime::new(
                plan,
                self.schema,
                self.clock,
                self.cancellation,
                tool_host,
                environment_host,
                model_provider,
                budget,
                trajectory,
                &observation,
            )?;
            agent_host
                .run_agent(&plan["task"], &observation, &mut runtime, agent_timeout_ms)
                .map_err(|error| error.in_phase("agent"))?
        };
        self.check_cancelled()?;
        self.schema
            .validate_shape("Outcome", &submitted)
            .map_err(|error| error.in_phase("agent"))?;
        if submitted.get("state").is_some() {
            return Err(ControlError::new("AGENT_OUTCOME_STATE_FORBIDDEN").in_phase("agent"));
        }
        if submitted.get("termination_reason").and_then(Value::as_str) == Some("in_progress") {
            return Err(ControlError::new("UNFINISHED_AGENT_OUTCOME").in_phase("agent"));
        }
        trajectory
            .record(
                "state",
                json!({"phase": "finalizing"}),
                self.clock.unix_time_ms(),
            )
            .map_err(|error| error.in_phase("persist"))?;
        let finalize_timeout_ms = budget
            .remaining_finalize_ms(self.clock)
            .map_err(|error| error.in_phase("environment"))?;
        let outcome = environment_host
            .finalize(&submitted, finalize_timeout_ms)
            .map_err(|error| error.in_phase("environment"))?;
        self.check_cancelled()?;
        self.schema
            .validate_shape("Outcome", &outcome)
            .map_err(|error| error.in_phase("environment"))?;
        if outcome.get("termination_reason").and_then(Value::as_str) == Some("in_progress") {
            return Err(ControlError::new("UNFINISHED_OUTCOME").in_phase("environment"));
        }
        for artifact in outcome
            .get("artifacts")
            .and_then(Value::as_array)
            .ok_or_else(|| ControlError::new("INVALID_OUTCOME_ARTIFACTS"))?
        {
            artifacts
                .read(artifact)
                .map_err(|error| error.in_phase("environment"))?;
        }

        let tool_freeze_timeout_ms = budget
            .remaining_finalize_ms(self.clock)
            .map_err(|error| error.in_phase("tool"))?;
        tool_host
            .freeze(tool_freeze_timeout_ms)
            .map_err(|error| error.in_phase("tool"))?;
        let backend_freeze_timeout_ms = budget
            .remaining_finalize_ms(self.clock)
            .map_err(|error| error.in_phase("environment"))?;
        backend
            .freeze(backend_freeze_timeout_ms)
            .map_err(|error| error.in_phase("environment"))?;
        self.check_cancelled()?;
        budget.remaining_finalize_ms(self.clock)?;
        let Some(scorer_host) = scorer_host.as_deref_mut() else {
            return Ok(CompletedAttempt {
                outcome,
                score: None,
            });
        };
        trajectory
            .record(
                "state",
                json!({"phase": "scoring"}),
                self.clock.unix_time_ms(),
            )
            .map_err(|error| error.in_phase("persist"))?;
        let trajectory_ref = trajectory
            .checkpoint(self.clock.unix_time_ms(), artifacts)
            .map_err(|error| error.in_phase("persist"))?;
        // The episode deadline and finalization reserve are the only scoring-time
        // controls. A scorer package may define algorithm parameters in config,
        // but it cannot introduce a second execution timeout.
        budget.remaining_finalize_ms(self.clock)?;
        let mut score_input = json!({
            "task": plan["task"],
            "outcome": outcome,
            "trajectory_ref": trajectory_ref,
        });
        if let Some(private_data) = plan.get("private_data") {
            score_input
                .as_object_mut()
                .unwrap()
                .insert("private_data".to_owned(), private_data.clone());
        }
        let score_deadline = budget.finalize_deadline_ms();
        let mut scoring_context = BackendScoringContext::new(
            backend,
            artifacts,
            self.clock,
            self.cancellation,
            score_deadline,
        );
        let score = run_score(
            self.schema,
            scorer_host,
            &plan["scorer"],
            &score_input,
            &mut scoring_context,
        );
        // A valid ScoreResult is retained even if its trajectory append fails.
        // record() marks the final manifest partial in that case.
        let _ = trajectory.record("score", score.clone(), self.clock.unix_time_ms());
        Ok(CompletedAttempt {
            outcome,
            score: Some(score),
        })
    }

    fn check_cancelled(&self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(ControlError::new("EPISODE_CANCELLED"));
        }
        Ok(())
    }
}

fn error_record(error: &ControlError, phase: &str) -> Value {
    let mut record = json!({
        "code": error.code,
        "phase": phase,
        "message": error.code,
        "retryable": error.retryable,
    });
    if let Some(operation_id) = &error.operation_id {
        record.as_object_mut().unwrap().insert(
            "operation_id".to_owned(),
            Value::String(operation_id.clone()),
        );
    }
    record
}

fn execution_status_for(code: &str) -> &'static str {
    if code.ends_with("TIMEOUT") || code == "FINALIZE_RESERVE_REACHED" {
        "timeout"
    } else if code.ends_with("CANCELLED") {
        "cancelled"
    } else {
        "failed"
    }
}
