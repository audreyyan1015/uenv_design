use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::contracts::{
    ContractSchema, array, bool_field, canonical_bytes, object, string, u64_field,
};
use crate::ports::{
    ArtifactStore, Backend, Cancellation, Clock, EnvironmentHost, ModelProvider, ToolHost,
};
use crate::{ControlError, Result};

// Storage policy, not a dataset or RunSpec option. Large payloads must be
// externalized as ArtifactRef before they reach the trajectory writer.
const TRAJECTORY_SEGMENT_MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub generation_count: u64,
    pub tool_call_count: u64,
    pub environment_step_count: u64,
    pub output_token_count: u64,
}

impl Usage {
    pub fn to_value(&self) -> Value {
        json!({
            "generation_count": self.generation_count,
            "tool_call_count": self.tool_call_count,
            "environment_step_count": self.environment_step_count,
            "output_token_count": self.output_token_count,
        })
    }

    pub fn from_value(value: &Value) -> Result<Self> {
        Ok(Self {
            generation_count: u64_field(value, "generation_count")?,
            tool_call_count: u64_field(value, "tool_call_count")?,
            environment_step_count: u64_field(value, "environment_step_count")?,
            output_token_count: u64_field(value, "output_token_count")?,
        })
    }
}

pub struct BudgetEnforcer {
    deadline_at_ms: u64,
    interaction_deadline_at_ms: u64,
    max_generations: u64,
    max_tool_calls: u64,
    max_environment_steps: u64,
    max_total_output_tokens: u64,
    usage: Usage,
}

impl BudgetEnforcer {
    fn from_plan_with_deadlines(
        plan: &Value,
        usage: Usage,
        deadline_at_ms: u64,
        interaction_deadline_at_ms: u64,
    ) -> Result<Self> {
        let limits = plan
            .get("limits")
            .ok_or_else(|| ControlError::new("MISSING_LIMITS"))?;
        let result = Self {
            deadline_at_ms,
            interaction_deadline_at_ms,
            max_generations: u64_field(limits, "max_generations")?,
            max_tool_calls: u64_field(limits, "max_tool_calls")?,
            max_environment_steps: u64_field(limits, "max_environment_steps")?,
            max_total_output_tokens: u64_field(limits, "max_total_output_tokens")?,
            usage,
        };
        result.validate_usage()?;
        Ok(result)
    }

    pub fn from_dispatch(dispatch: &Value, clock: &dyn Clock) -> Result<Self> {
        let plan = dispatch
            .get("plan")
            .ok_or_else(|| ControlError::new("MISSING_EXECUTION_PLAN"))?;
        let usage = Usage::from_value(
            dispatch
                .get("consumed_usage")
                .ok_or_else(|| ControlError::new("MISSING_CONSUMED_USAGE"))?,
        )?;
        let transmitted_remaining = u64_field(dispatch, "remaining_timeout_ms")?;
        let now = clock.monotonic_ms();
        let deadline_at_ms = now
            .checked_add(transmitted_remaining)
            .ok_or_else(|| ControlError::new("INVALID_REMAINING_TIMEOUT"))?;
        let finalize_reserve_ms = u64_field(
            plan.get("limits")
                .ok_or_else(|| ControlError::new("MISSING_LIMITS"))?,
            "finalize_reserve_ms",
        )?;
        let interaction_deadline_at_ms = now
            .checked_add(transmitted_remaining.saturating_sub(finalize_reserve_ms))
            .ok_or_else(|| ControlError::new("INVALID_REMAINING_TIMEOUT"))?;
        Self::from_plan_with_deadlines(plan, usage, deadline_at_ms, interaction_deadline_at_ms)
    }

    fn validate_usage(&self) -> Result<()> {
        if self.usage.generation_count > self.max_generations
            || self.usage.tool_call_count > self.max_tool_calls
            || self.usage.environment_step_count > self.max_environment_steps
            || self.usage.output_token_count > self.max_total_output_tokens
        {
            return Err(ControlError::new("CONSUMED_USAGE_EXCEEDS_LIMITS"));
        }
        Ok(())
    }

    pub fn check_total(&self, clock: &dyn Clock) -> Result<()> {
        if clock.monotonic_ms() >= self.deadline_at_ms {
            return Err(ControlError::new("EPISODE_TIMEOUT"));
        }
        Ok(())
    }

    pub fn check_interaction(&self, clock: &dyn Clock) -> Result<()> {
        let now = clock.monotonic_ms();
        if now >= self.deadline_at_ms {
            return Err(ControlError::new("EPISODE_TIMEOUT"));
        }
        if now >= self.interaction_deadline_at_ms {
            return Err(ControlError::new("FINALIZE_RESERVE_REACHED"));
        }
        Ok(())
    }

    pub fn begin_model(&mut self, clock: &dyn Clock) -> Result<u64> {
        self.check_interaction(clock)?;
        if self.usage.generation_count >= self.max_generations {
            return Err(ControlError::new("GENERATION_LIMIT"));
        }
        if self.usage.output_token_count >= self.max_total_output_tokens {
            return Err(ControlError::new("OUTPUT_TOKEN_LIMIT"));
        }
        self.usage.generation_count += 1;
        Ok(self.max_total_output_tokens - self.usage.output_token_count)
    }

    pub fn finish_model(&mut self, output_tokens: u64) -> Result<()> {
        self.usage.output_token_count = self
            .usage
            .output_token_count
            .checked_add(output_tokens)
            .ok_or_else(|| ControlError::new("OUTPUT_TOKEN_LIMIT"))?;
        if self.usage.output_token_count > self.max_total_output_tokens {
            return Err(ControlError::new("OUTPUT_TOKEN_LIMIT"));
        }
        Ok(())
    }

    pub fn begin_tool(&mut self, clock: &dyn Clock) -> Result<()> {
        self.check_interaction(clock)?;
        if self.usage.tool_call_count >= self.max_tool_calls {
            return Err(ControlError::new("TOOL_CALL_LIMIT"));
        }
        self.usage.tool_call_count += 1;
        Ok(())
    }

    pub fn begin_environment_step(&mut self, clock: &dyn Clock) -> Result<()> {
        self.check_interaction(clock)?;
        if self.usage.environment_step_count >= self.max_environment_steps {
            return Err(ControlError::new("ENVIRONMENT_STEP_LIMIT"));
        }
        self.usage.environment_step_count += 1;
        Ok(())
    }

    pub fn remaining_finalize_ms(&self, clock: &dyn Clock) -> Result<u64> {
        let now = clock.monotonic_ms();
        if now >= self.deadline_at_ms {
            return Err(ControlError::new("EPISODE_TIMEOUT"));
        }
        Ok(self.deadline_at_ms - now)
    }

    /// The original local monotonic deadline; scoring must never restart it.
    pub fn finalize_deadline_ms(&self) -> u64 {
        self.deadline_at_ms
    }

    pub fn remaining_interaction_ms(&self, clock: &dyn Clock) -> Result<u64> {
        let now = clock.monotonic_ms();
        if now >= self.deadline_at_ms {
            return Err(ControlError::new("EPISODE_TIMEOUT"));
        }
        if now >= self.interaction_deadline_at_ms {
            return Err(ControlError::new("FINALIZE_RESERVE_REACHED"));
        }
        Ok(self.interaction_deadline_at_ms - now)
    }

    pub fn usage(&self) -> Usage {
        self.usage.clone()
    }
}

pub struct TrajectoryWriter {
    schema: ContractSchema,
    run_id: String,
    episode_id: String,
    attempt_id: u64,
    task_id: String,
    events: Vec<Value>,
    sealed: bool,
    complete: bool,
}

impl TrajectoryWriter {
    pub fn from_plan(plan: &Value, schema: &ContractSchema) -> Result<Self> {
        Ok(Self {
            schema: schema.clone(),
            run_id: string(plan, "run_id")?.to_owned(),
            episode_id: string(plan, "episode_id")?.to_owned(),
            attempt_id: u64_field(plan, "attempt_id")?,
            task_id: plan
                .pointer("/task/task_id")
                .and_then(Value::as_str)
                .ok_or_else(|| ControlError::new("INVALID_TASK_ID"))?
                .to_owned(),
            events: Vec::new(),
            sealed: false,
            complete: true,
        })
    }

    pub fn record(&mut self, kind: &str, payload: Value, now_ms: u64) -> Result<String> {
        let result = self.try_record(kind, payload, now_ms);
        if result.is_err() {
            self.complete = false;
        }
        result
    }

    fn try_record(&mut self, kind: &str, payload: Value, now_ms: u64) -> Result<String> {
        if self.sealed {
            return Err(ControlError::new("TRAJECTORY_ALREADY_SEALED"));
        }
        let payload_type = match kind {
            "state" => "StateEvent",
            "observation" => "Observation",
            "generation" => "GenerationEvent",
            "tool_call" => "ToolCall",
            "tool_result" => "ToolResult",
            "environment_transition" => "EnvironmentTransition",
            "score" => "ScoreResult",
            "error" => "ErrorRecord",
            "terminal" => "TerminalEvent",
            _ => return Err(ControlError::new("UNKNOWN_TRAJECTORY_EVENT_KIND")),
        };
        self.schema.validate_shape(payload_type, &payload)?;
        let sequence = self.events.len() as u64;
        let event_id = format!("{}:{}:{}", self.episode_id, self.attempt_id, sequence);
        let event = json!({
            "schema_version": "vnext.3",
            "event_id": event_id,
            "run_id": self.run_id,
            "episode_id": self.episode_id,
            "attempt_id": self.attempt_id,
            "task_id": self.task_id,
            "sequence": sequence,
            "occurred_at_ms": now_ms,
            "kind": kind,
            "payload": payload,
        });
        self.schema.validate_shape("TrajectoryEvent", &event)?;
        if contains_field(&event["payload"], "private_data") {
            return Err(ControlError::new("PRIVATE_DATA_IN_TRAJECTORY"));
        }
        if canonical_bytes(&event)?.len() + 1 > TRAJECTORY_SEGMENT_MAX_BYTES {
            return Err(ControlError::new("TRAJECTORY_EVENT_TOO_LARGE"));
        }
        self.events.push(event);
        Ok(event_id)
    }

    fn write_segments(&self, artifacts: &mut dyn ArtifactStore) -> Result<Vec<Value>> {
        encode_jsonl_segments(&self.events, TRAJECTORY_SEGMENT_MAX_BYTES)?
            .iter()
            .map(|segment| artifacts.put_bytes(segment, "application/x-ndjson"))
            .collect()
    }

    fn write_manifest(
        &self,
        trajectory_status: &str,
        now_ms: u64,
        artifacts: &mut dyn ArtifactStore,
    ) -> Result<Value> {
        let segments = self.write_segments(artifacts)?;
        let manifest = json!({
            "schema_version": "vnext.3",
            "run_id": self.run_id,
            "episode_id": self.episode_id,
            "attempt_id": self.attempt_id,
            "task_id": self.task_id,
            "event_segments": segments,
            "event_count": self.events.len(),
            "trajectory_status": trajectory_status,
            "created_at_ms": now_ms,
        });
        self.schema
            .validate_shape("TrajectoryManifest", &manifest)?;
        artifacts.put_json(&manifest)
    }

    pub fn checkpoint(&mut self, now_ms: u64, artifacts: &mut dyn ArtifactStore) -> Result<Value> {
        let result = self.write_manifest("scoring_checkpoint", now_ms, artifacts);
        if result.is_err() {
            self.complete = false;
        }
        result
    }

    pub fn seal(&mut self, now_ms: u64, artifacts: &mut dyn ArtifactStore) -> Result<Value> {
        let trajectory_status = if self.complete {
            "final_complete"
        } else {
            "final_partial"
        };
        let manifest = self.write_manifest(trajectory_status, now_ms, artifacts)?;
        self.sealed = true;
        Ok(manifest)
    }

    pub fn events(&self) -> &[Value] {
        &self.events
    }
}

fn encode_jsonl_segments(events: &[Value], max_bytes: usize) -> Result<Vec<Vec<u8>>> {
    if max_bytes == 0 {
        return Err(ControlError::new("INVALID_TRAJECTORY_SEGMENT_SIZE"));
    }

    let mut segments = Vec::new();
    let mut current = Vec::new();
    for event in events {
        let mut line = canonical_bytes(event)?;
        line.push(b'\n');
        if line.len() > max_bytes {
            return Err(ControlError::new("TRAJECTORY_EVENT_TOO_LARGE"));
        }
        if !current.is_empty() && current.len() + line.len() > max_bytes {
            segments.push(std::mem::take(&mut current));
        }
        current.extend(line);
    }
    if !current.is_empty() || segments.is_empty() {
        segments.push(current);
    }
    Ok(segments)
}

fn contains_field(value: &Value, field: &str) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key(field) || object.values().any(|value| contains_field(value, field))
        }
        Value::Array(values) => values.iter().any(|value| contains_field(value, field)),
        _ => false,
    }
}

pub struct AgentRuntime<'a> {
    pub schema: &'a ContractSchema,
    pub clock: &'a dyn Clock,
    pub cancellation: &'a dyn Cancellation,
    pub tool_host: &'a mut dyn ToolHost,
    pub environment_host: &'a mut dyn EnvironmentHost,
    pub model_provider: &'a mut dyn ModelProvider,
    pub budget: &'a mut BudgetEnforcer,
    pub trajectory: &'a mut TrajectoryWriter,
    model: Value,
    training: Option<Value>,
    tools: Vec<Value>,
    seed: u64,
    current_observation: Value,
    episode_id: String,
    attempt_id: u64,
    generation_ids: BTreeSet<String>,
    tool_call_ids: BTreeSet<String>,
    next_generation_index: u64,
    next_environment_step_index: u64,
}

impl<'a> AgentRuntime<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plan: &Value,
        schema: &'a ContractSchema,
        clock: &'a dyn Clock,
        cancellation: &'a dyn Cancellation,
        tool_host: &'a mut dyn ToolHost,
        environment_host: &'a mut dyn EnvironmentHost,
        model_provider: &'a mut dyn ModelProvider,
        budget: &'a mut BudgetEnforcer,
        trajectory: &'a mut TrajectoryWriter,
        initial_observation: &Value,
    ) -> Result<Self> {
        schema.validate_shape("Observation", initial_observation)?;
        Ok(Self {
            schema,
            clock,
            cancellation,
            tool_host,
            environment_host,
            model_provider,
            budget,
            trajectory,
            model: plan
                .get("model")
                .cloned()
                .ok_or_else(|| ControlError::new("MISSING_MODEL"))?,
            training: plan.get("training").cloned(),
            tools: array(plan, "tools")?.clone(),
            seed: u64_field(plan, "seed")?,
            current_observation: initial_observation.clone(),
            episode_id: string(plan, "episode_id")?.to_owned(),
            attempt_id: u64_field(plan, "attempt_id")?,
            generation_ids: BTreeSet::new(),
            tool_call_ids: BTreeSet::new(),
            next_generation_index: 0,
            next_environment_step_index: 0,
        })
    }

    pub fn generate(&mut self, messages: &Value) -> Result<Value> {
        self.check_active()?;
        let messages = messages
            .as_array()
            .ok_or_else(|| ControlError::new("MESSAGES_MUST_BE_ARRAY"))?;
        let mut model = object(&self.model, "ModelSpec")?.clone();
        let generation = model
            .get_mut("generation")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| ControlError::new("INVALID_MODEL_GENERATION"))?;
        let configured = generation
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .ok_or_else(|| ControlError::new("INVALID_MAX_OUTPUT_TOKENS"))?;
        let remaining_timeout_ms = self
            .budget
            .remaining_interaction_ms(self.clock)
            .map_err(|error| error.in_phase("model"))?;
        let remaining_tokens = self
            .budget
            .begin_model(self.clock)
            .map_err(|error| error.in_phase("model"))?;
        let generation_id = format!(
            "{}:{}:generation:{}",
            self.episode_id, self.attempt_id, self.next_generation_index
        );
        self.next_generation_index = self
            .next_generation_index
            .checked_add(1)
            .ok_or_else(|| ControlError::new("GENERATION_INDEX_OVERFLOW").in_phase("model"))?;
        generation.insert(
            "max_output_tokens".to_owned(),
            json!(configured.min(remaining_tokens)),
        );
        let mut command = json!({
            "generation_id": generation_id,
            "model": model,
            "messages": messages,
            "tools": self.tools,
            "seed": self.seed,
            "remaining_timeout_ms": remaining_timeout_ms,
        });
        if let Some(training) = &self.training {
            command
                .as_object_mut()
                .unwrap()
                .insert("training".to_owned(), training.clone());
        }
        let event = self.model_provider.generate(&command).map_err(|error| {
            error
                .in_phase("model")
                .with_operation(generation_id.clone())
        })?;
        self.schema
            .validate_shape("GenerationEvent", &event)
            .map_err(|error| {
                error
                    .in_phase("model")
                    .with_operation(generation_id.clone())
            })?;
        if event.get("generation_id").and_then(Value::as_str) != Some(generation_id.as_str()) {
            return Err(ControlError::new("MODEL_GENERATION_ID_MISMATCH")
                .in_phase("model")
                .with_operation(generation_id));
        }
        if event.get("messages") != Some(&Value::Array(messages.clone())) {
            return Err(ControlError::new("MODEL_MESSAGES_MISMATCH")
                .in_phase("model")
                .with_operation(generation_id));
        }
        let output_tokens = self.validate_generation_trace(&event).map_err(|error| {
            error
                .in_phase("model")
                .with_operation(generation_id.clone())
        })?;
        if !self.generation_ids.insert(generation_id.clone()) {
            return Err(ControlError::new("DUPLICATE_GENERATION_ID")
                .in_phase("model")
                .with_operation(generation_id));
        }
        self.budget.finish_model(output_tokens).map_err(|error| {
            error
                .in_phase("model")
                .with_operation(generation_id.clone())
        })?;
        self.trajectory
            .record("generation", event.clone(), self.clock.unix_time_ms())
            .map_err(|error| error.in_phase("persist").with_operation(generation_id))?;
        self.check_active()?;
        Ok(event)
    }

    fn validate_generation_trace(&self, event: &Value) -> Result<u64> {
        // Proposed calls are not executed here. The same bindings are checked
        // again when Agent requests the side effect through call_tool.
        let proposed = optional_array(event, "tool_calls")?;
        let requests_tools =
            event.get("finish_reason").and_then(Value::as_str) == Some("tool_calls");
        if requests_tools != proposed.is_some() || proposed.is_some_and(Vec::is_empty) {
            return Err(ControlError::new("MODEL_TOOL_CALLS_MISMATCH"));
        }
        let mut proposed_ids = BTreeSet::new();
        for call in proposed.into_iter().flatten() {
            self.schema.validate_shape("ToolCall", call)?;
            let id = string(call, "tool_call_id")?;
            if !proposed_ids.insert(id) || self.tool_call_ids.contains(id) {
                return Err(ControlError::new("DUPLICATE_TOOL_CALL_ID"));
            }
            if call.get("generation_id") != event.get("generation_id") {
                return Err(ControlError::new("MODEL_TOOL_GENERATION_MISMATCH"));
            }
            let binding = self
                .tools
                .iter()
                .find(|binding| binding.get("name") == call.get("name"))
                .ok_or_else(|| ControlError::new("TOOL_NOT_SELECTED"))?;
            if call.get("implementation") != binding.get("implementation") {
                return Err(ControlError::new("TOOL_CALL_BINDING_MISMATCH"));
            }
            if u64_field(call, "timeout_ms")? == 0 {
                return Err(ControlError::new("INVALID_TOOL_TIMEOUT"));
            }
        }
        let output_token_count = u64_field(event, "output_token_count")?;
        let output_token_ids = optional_array(event, "output_token_ids")?;
        if output_token_ids.is_some_and(|values| values.len() as u64 != output_token_count) {
            return Err(ControlError::new("OUTPUT_TOKEN_COUNT_MISMATCH"));
        }
        let output_logprobs = optional_array(event, "output_logprobs")?;
        if output_logprobs
            .is_some_and(|values| Some(values.len()) != output_token_ids.map(Vec::len))
        {
            return Err(ControlError::new("OUTPUT_LOGPROBS_LENGTH_MISMATCH"));
        }
        let loss_mask = optional_array(event, "loss_mask")?;
        if loss_mask.is_some_and(|values| Some(values.len()) != output_token_ids.map(Vec::len)) {
            return Err(ControlError::new("LOSS_MASK_LENGTH_MISMATCH"));
        }

        let token_trace_required = self
            .training
            .as_ref()
            .and_then(|training| training.get("require_token_trace"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if token_trace_required {
            for field in [
                "policy_version",
                "parameter_version",
                "tokenizer",
                "input_token_ids",
                "output_token_ids",
                "output_logprobs",
                "loss_mask",
            ] {
                if event.get(field).is_none() {
                    return Err(ControlError::new(format!(
                        "TRAINING_TOKEN_TRACE_REQUIRED:{field}"
                    )));
                }
            }
        }
        Ok(output_token_count)
    }

    pub fn call_tool(&mut self, call: &Value) -> Result<Value> {
        self.check_active()?;
        self.schema
            .validate_shape("ToolCall", call)
            .map_err(|error| error.in_phase("tool"))?;
        let tool_call_id = string(call, "tool_call_id")?.to_owned();
        let generation_id = string(call, "generation_id")?;
        if !self.generation_ids.contains(generation_id) {
            return Err(ControlError::new("UNKNOWN_TOOL_CALL_GENERATION_ID")
                .in_phase("tool")
                .with_operation(tool_call_id));
        }
        if self.tool_call_ids.contains(&tool_call_id) {
            return Err(ControlError::new("DUPLICATE_TOOL_CALL_ID")
                .in_phase("tool")
                .with_operation(tool_call_id));
        }
        let name = string(call, "name")?;
        let binding = self
            .tools
            .iter()
            .find(|binding| binding.get("name").and_then(Value::as_str) == Some(name))
            .ok_or_else(|| {
                ControlError::new("TOOL_NOT_SELECTED")
                    .in_phase("tool")
                    .with_operation(tool_call_id.clone())
            })?;
        if call.get("implementation") != binding.get("implementation") {
            return Err(ControlError::new("TOOL_CALL_BINDING_MISMATCH")
                .in_phase("tool")
                .with_operation(tool_call_id));
        }
        let configured_timeout = binding
            .pointer("/config/data/timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX);
        let requested_timeout = u64_field(call, "timeout_ms")?;
        let remaining_timeout_ms = self
            .budget
            .remaining_interaction_ms(self.clock)
            .map_err(|error| error.in_phase("tool").with_operation(tool_call_id.clone()))?;
        let mut effective_call = object(call, "ToolCall")?.clone();
        effective_call.insert(
            "timeout_ms".to_owned(),
            json!(
                requested_timeout
                    .min(configured_timeout)
                    .min(remaining_timeout_ms)
                    .max(1)
            ),
        );
        let effective_call = Value::Object(effective_call);
        self.budget
            .begin_tool(self.clock)
            .map_err(|error| error.in_phase("tool").with_operation(tool_call_id.clone()))?;
        self.tool_call_ids.insert(tool_call_id.clone());
        self.trajectory
            .record(
                "tool_call",
                effective_call.clone(),
                self.clock.unix_time_ms(),
            )
            .map_err(|error| {
                error
                    .in_phase("persist")
                    .with_operation(tool_call_id.clone())
            })?;
        let result = match self.tool_host.call_tool(binding, &effective_call) {
            Ok(result) => result,
            Err(failure) => {
                let failure = failure
                    .in_phase("tool")
                    .with_operation(tool_call_id.clone());
                let result = failed_tool_result(&effective_call, &failure);
                self.trajectory
                    .record("tool_result", result, self.clock.unix_time_ms())
                    .map_err(|error| {
                        error
                            .in_phase("persist")
                            .with_operation(tool_call_id.clone())
                    })?;
                return Err(failure);
            }
        };
        if let Err(failure) = self.schema.validate_shape("ToolResult", &result) {
            let failure = failure
                .in_phase("tool")
                .with_operation(tool_call_id.clone());
            let recorded = failed_tool_result(&effective_call, &failure);
            self.trajectory
                .record("tool_result", recorded, self.clock.unix_time_ms())
                .map_err(|error| {
                    error
                        .in_phase("persist")
                        .with_operation(tool_call_id.clone())
                })?;
            return Err(failure);
        }
        if result.get("tool_call_id") != effective_call.get("tool_call_id") {
            let failure = ControlError::new("TOOL_RESULT_ID_MISMATCH")
                .in_phase("tool")
                .with_operation(tool_call_id.clone());
            let recorded = failed_tool_result(&effective_call, &failure);
            self.trajectory
                .record("tool_result", recorded, self.clock.unix_time_ms())
                .map_err(|error| {
                    error
                        .in_phase("persist")
                        .with_operation(tool_call_id.clone())
                })?;
            return Err(failure);
        }
        self.trajectory
            .record("tool_result", result.clone(), self.clock.unix_time_ms())
            .map_err(|error| error.in_phase("persist").with_operation(tool_call_id))?;
        self.check_active()?;
        Ok(result)
    }

    pub fn step(&mut self, action: &Value) -> Result<Value> {
        self.check_active()?;
        self.schema
            .validate_shape("TypedConfig", action)
            .map_err(|error| error.in_phase("environment"))?;
        let remaining_timeout_ms = self
            .budget
            .remaining_interaction_ms(self.clock)
            .map_err(|error| error.in_phase("environment"))?;
        self.budget
            .begin_environment_step(self.clock)
            .map_err(|error| error.in_phase("environment"))?;
        let index = self.next_environment_step_index;
        let operation_id = index.to_string();
        self.next_environment_step_index = self
            .next_environment_step_index
            .checked_add(1)
            .ok_or_else(|| {
                ControlError::new("ENVIRONMENT_STEP_INDEX_OVERFLOW")
                    .in_phase("environment")
                    .with_operation(operation_id.clone())
            })?;
        let transition = self
            .environment_host
            .step(action, remaining_timeout_ms)
            .map_err(|error| {
                error
                    .in_phase("environment")
                    .with_operation(operation_id.clone())
            })?;
        self.schema
            .validate_shape("Transition", &transition)
            .map_err(|error| {
                error
                    .in_phase("environment")
                    .with_operation(operation_id.clone())
            })?;
        let observation_before = self.current_observation.clone();
        let event = json!({
            "environment_step_index": index,
            "observation_before": observation_before,
            "action": action,
            "transition": transition,
        });
        self.schema
            .validate_shape("EnvironmentTransition", &event)
            .map_err(|error| {
                error
                    .in_phase("environment")
                    .with_operation(operation_id.clone())
            })?;
        self.trajectory
            .record(
                "environment_transition",
                event.clone(),
                self.clock.unix_time_ms(),
            )
            .map_err(|error| {
                error
                    .in_phase("persist")
                    .with_operation(operation_id.clone())
            })?;
        self.current_observation = transition.get("observation").cloned().ok_or_else(|| {
            ControlError::new("TRANSITION_OBSERVATION_REQUIRED")
                .in_phase("environment")
                .with_operation(operation_id)
        })?;
        self.check_active()?;
        Ok(transition.clone())
    }

    pub fn usage(&self) -> Usage {
        self.budget.usage()
    }

    fn check_active(&self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(ControlError::new("EPISODE_CANCELLED"));
        }
        Ok(())
    }
}

fn failed_tool_result(call: &Value, failure: &ControlError) -> Value {
    let status = if failure.code.contains("TIMEOUT") {
        "timeout"
    } else if failure.code.contains("CANCELLED") {
        "cancelled"
    } else {
        "error"
    };
    json!({
        "tool_call_id": call["tool_call_id"],
        "status": status,
        "content": [],
        "output_truncated": false,
        "error": {
            "code": failure.code,
            "phase": "tool",
            "message": failure.code,
            "retryable": failure.retryable,
            "operation_id": call["tool_call_id"],
        }
    })
}

fn optional_array<'a>(value: &'a Value, field: &str) -> Result<Option<&'a Vec<Value>>> {
    match value.get(field) {
        None => Ok(None),
        Some(value) => value
            .as_array()
            .map(Some)
            .ok_or_else(|| ControlError::new(format!("INVALID_FIELD:{field}"))),
    }
}

pub fn ensure_plan_backend_started(
    plan: &Value,
    backend: &mut dyn Backend,
    remaining_timeout_ms: u64,
) -> Result<Value> {
    backend.open(
        plan.get("backend")
            .ok_or_else(|| ControlError::new("MISSING_BACKEND"))?,
        plan.get("runtime"),
        bool_field(plan, "internet_access")?,
        remaining_timeout_ms,
    )
}

#[cfg(test)]
mod trajectory_segment_tests {
    use super::*;

    #[test]
    fn splits_jsonl_only_between_complete_events() {
        let events = vec![json!({"i": 0}), json!({"i": 1}), json!({"i": 2})];
        let one_line_bytes = canonical_bytes(&events[0]).unwrap().len() + 1;
        let segments = encode_jsonl_segments(&events, one_line_bytes * 2).unwrap();

        assert_eq!(segments.len(), 2);
        let decoded: Vec<Value> = segments
            .iter()
            .flat_map(|segment| segment.split(|byte| *byte == b'\n'))
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert_eq!(decoded, events);
    }

    #[test]
    fn rejects_an_event_larger_than_the_segment_limit() {
        let error = encode_jsonl_segments(&[json!({"payload": "too-large"})], 4).unwrap_err();
        assert_eq!(error.code, "TRAJECTORY_EVENT_TOO_LARGE");
    }

    #[test]
    fn any_record_failure_makes_the_final_manifest_partial() {
        let schema = ContractSchema::bundled();
        let plan = json!({
            "run_id": "run-1",
            "episode_id": "episode-1",
            "attempt_id": 1,
            "task": {"task_id": "task-1"}
        });
        let mut writer = TrajectoryWriter::from_plan(&plan, &schema).unwrap();
        assert_eq!(
            writer.record("unknown", json!({}), 1).unwrap_err().code,
            "UNKNOWN_TRAJECTORY_EVENT_KIND"
        );
        let mut artifacts = crate::ports::MemoryArtifactStore::default();
        let reference = writer.seal(2, &mut artifacts).unwrap();
        let manifest: Value = serde_json::from_slice(&artifacts.read(&reference).unwrap()).unwrap();
        assert_eq!(manifest["trajectory_status"], "final_partial");
    }
}
