use std::cell::RefCell;
use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::contracts::{ContractSchema, array, object, string, u64_field};
use crate::ports::{Backend, Cancellation, Clock, FileStore, ScorerHost, ScoringContext};
use crate::{ControlError, Result};

pub struct BackendScoringContext<'a> {
    backend: &'a mut dyn Backend,
    artifacts: &'a dyn FileStore,
    clock: &'a dyn Clock,
    cancellation: &'a dyn Cancellation,
    monotonic_deadline_ms: u64,
    score_input: &'a Value,
    readable: RefCell<BTreeSet<Vec<u8>>>,
}

impl<'a> BackendScoringContext<'a> {
    pub fn new(
        backend: &'a mut dyn Backend,
        artifacts: &'a dyn FileStore,
        clock: &'a dyn Clock,
        cancellation: &'a dyn Cancellation,
        monotonic_deadline_ms: u64,
        score_input: &'a Value,
    ) -> Self {
        let mut readable = BTreeSet::new();
        collect_references(score_input, &mut readable);
        Self {
            backend,
            artifacts,
            clock,
            cancellation,
            monotonic_deadline_ms,
            score_input,
            readable: RefCell::new(readable),
        }
    }
}

impl ScoringContext for BackendScoringContext<'_> {
    fn run_harness(&mut self, request: &Value) -> Result<Value> {
        let remaining = self.remaining_timeout_ms()?;
        ContractSchema::bundled().validate_shape("HarnessRequest", request)?;
        for field in ["final_answer", "private_data", "state"] {
            if request.get(field) != self.score_input.get(field) {
                return Err(ControlError::new("HARNESS_INPUT_MISMATCH"));
            }
        }
        let requested = u64_field(request, "remaining_timeout_ms")?;
        let evaluation_plan = self
            .score_input
            .pointer("/private_data/data/evaluation_plan")
            .ok_or_else(|| ControlError::new("MISSING_EVALUATION_PLAN"))?;
        let configured = u64_field(evaluation_plan, "timeout_ms")?;
        if requested == 0 || configured == 0 {
            return Err(ControlError::new("INVALID_HARNESS_TIMEOUT"));
        }
        let mut effective = request.clone();
        effective["remaining_timeout_ms"] = json!(requested.min(configured).min(remaining));
        let result = self.backend.run_harness(&effective)?;
        self.remaining_timeout_ms()?;
        Ok(result)
    }

    fn read_artifact(&self, reference: &Value) -> Result<Vec<u8>> {
        self.remaining_timeout_ms()?;
        if !self
            .readable
            .borrow()
            .contains(&crate::contracts::canonical_bytes(reference)?)
        {
            return Err(ControlError::new("SCORING_ARTIFACT_ACCESS_DENIED"));
        }
        let bytes = self.artifacts.read(reference)?;
        // A scoring checkpoint contains references to its own immutable
        // segments. Grant only references found in the verified contents.
        if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
            collect_references(&value, &mut self.readable.borrow_mut());
        }
        Ok(bytes)
    }

    fn remaining_timeout_ms(&self) -> Result<u64> {
        if self.cancellation.is_cancelled() {
            return Err(ControlError::new("EPISODE_CANCELLED"));
        }
        let now = self.clock.monotonic_ms();
        if now >= self.monotonic_deadline_ms {
            return Err(ControlError::new("SCORER_TIMEOUT"));
        }
        Ok(self.monotonic_deadline_ms - now)
    }
}

fn collect_references(value: &Value, references: &mut BTreeSet<Vec<u8>>) {
    match value {
        Value::Object(map) => {
            if ["uri", "digest", "size_bytes", "media_type"]
                .iter()
                .all(|field| map.contains_key(*field))
            {
                if let Ok(bytes) = crate::contracts::canonical_bytes(value) {
                    references.insert(bytes);
                }
            } else {
                for child in map.values() {
                    collect_references(child, references);
                }
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_references(child, references);
            }
        }
        _ => (),
    }
}

pub fn run_score(
    schema: &ContractSchema,
    scorer_host: &mut dyn ScorerHost,
    dataset_package: &Value,
    config: &Value,
    request: &Value,
    context: &mut dyn ScoringContext,
) -> Value {
    let scorer_ref = dataset_package.clone();
    let completed = (|| -> Result<Value> {
        schema.validate_shape("ScoreInput", request)?;
        schema.validate_shape("ResolvedComponent", &scorer_ref)?;
        context.remaining_timeout_ms()?;
        let candidate = scorer_host.score(dataset_package, config, request, context)?;
        context.remaining_timeout_ms()?;
        validate_candidate_score(schema, &candidate)?;
        validate_generation_rewards(schema, &candidate, request, context)?;
        context.remaining_timeout_ms()?;
        let mut result = object(&candidate, "ScorerResult")?.clone();
        result.insert("status".to_owned(), Value::String("ok".to_owned()));
        result.insert("scorer".to_owned(), scorer_ref.clone());
        let result = Value::Object(result);
        schema.validate_shape("ScoreResult", &result)?;
        Ok(result)
    })();

    match completed {
        Ok(result) => result,
        Err(error) => {
            let code = match error.code.as_str() {
                "EPISODE_CANCELLED" | "SCORER_TIMEOUT" => error.code.clone(),
                _ => "SCORER_FAILED".to_owned(),
            };
            json!({
                "status": "error",
                "success": null,
                "metrics": [],
                "reward": null,
                "evidence": [],
                "scorer": scorer_ref,
                "error": {
                    "code": code,
                    "phase": "score",
                    "message": error.code,
                    "retryable": error.retryable
                }
            })
        }
    }
}

fn validate_candidate_score(schema: &ContractSchema, candidate: &Value) -> Result<()> {
    let object = object(candidate, "ScorerResult")?;
    let expected: BTreeSet<&str> = ["success", "metrics", "reward", "evidence"]
        .into_iter()
        .collect();
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if !expected.is_subset(&actual)
        || actual
            .difference(&expected)
            .any(|name| *name != "generation_rewards")
    {
        return Err(ControlError::new("SCORER_RESULT_FIELDS_INVALID"));
    }
    let reward = object
        .get("reward")
        .and_then(Value::as_f64)
        .ok_or_else(|| ControlError::new("SCORER_REWARD_REQUIRED"))?;
    if !reward.is_finite() {
        return Err(ControlError::new("SCORER_REWARD_NOT_FINITE"));
    }
    let metrics = object
        .get("metrics")
        .and_then(Value::as_array)
        .ok_or_else(|| ControlError::new("SCORER_METRICS_INVALID"))?;
    for metric in metrics {
        schema.validate_shape("Metric", metric)?;
        if !metric["value"].as_f64().is_some_and(f64::is_finite)
            || metric["name"].as_str().is_none_or(str::is_empty)
            || metric["unit"].as_str().is_none_or(str::is_empty)
            || !matches!(
                metric["direction"].as_str(),
                Some("higher" | "lower" | "none")
            )
        {
            return Err(ControlError::new("SCORER_METRIC_VALUE_INVALID"));
        }
    }
    let names: Vec<&str> = metrics
        .iter()
        .map(|metric| {
            metric
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| ControlError::new("SCORER_METRIC_NAME_INVALID"))
        })
        .collect::<Result<_>>()?;
    if names.iter().copied().collect::<BTreeSet<_>>().len() != names.len() {
        return Err(ControlError::new("DUPLICATE_METRIC_NAME"));
    }
    if !object.get("evidence").is_some_and(Value::is_array)
        || !object
            .get("success")
            .is_some_and(|value| value.is_boolean() || value.is_null())
    {
        return Err(ControlError::new("SCORER_RESULT_VALUE_INVALID"));
    }
    Ok(())
}

/// Verify process scores against the immutable pre-score trajectory, not IDs
/// invented by a scorer or a second generation counter.
fn validate_generation_rewards(
    schema: &ContractSchema,
    candidate: &Value,
    request: &Value,
    context: &mut dyn ScoringContext,
) -> Result<()> {
    let Some(rewards) = candidate.get("generation_rewards") else {
        return Ok(());
    };
    let rewards = rewards
        .as_array()
        .ok_or_else(|| ControlError::new("INVALID_GENERATION_REWARDS"))?;
    if rewards.is_empty() {
        return Ok(());
    }
    let bytes = context.read_artifact(&request["trajectory_ref"])?;
    let manifest: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ControlError::new("INVALID_SCORING_TRAJECTORY"))?;
    schema.validate_shape("TrajectoryManifest", &manifest)?;
    if manifest["trajectory_status"] != "scoring_checkpoint"
        || manifest.get("task_id") != request.pointer("/task/task_id")
    {
        return Err(ControlError::new("SCORING_TRAJECTORY_IDENTITY_MISMATCH"));
    }
    let mut generation_ids = BTreeSet::new();
    let mut seen_ids = BTreeSet::new();
    let mut count = 0_u64;
    for segment in array(&manifest, "event_segments")? {
        context.remaining_timeout_ms()?;
        let bytes = context.read_artifact(segment)?;
        let content = std::str::from_utf8(&bytes)
            .map_err(|_| ControlError::new("INVALID_SCORING_TRAJECTORY"))?;
        for line in content.lines() {
            context.remaining_timeout_ms()?;
            let event: Value = serde_json::from_str(line)
                .map_err(|_| ControlError::new("INVALID_SCORING_TRAJECTORY"))?;
            schema.validate_shape("TrajectoryEvent", &event)?;
            for field in ["run_id", "episode_id", "attempt_id", "task_id"] {
                if event.get(field) != manifest.get(field) {
                    return Err(ControlError::new("SCORING_TRAJECTORY_IDENTITY_MISMATCH"));
                }
            }
            if event["sequence"].as_u64() != Some(count) {
                return Err(ControlError::new("INCOMPLETE_SCORING_TRAJECTORY"));
            }
            count += 1;
            if event["kind"] == "generation" {
                let generation = &event["payload"];
                schema.validate_shape("GenerationEvent", generation)?;
                let id = string(generation, "generation_id")?;
                let response = array(generation, "response")?;
                if id.is_empty() || !seen_ids.insert(id.to_owned()) {
                    return Err(ControlError::new("INVALID_TRAJECTORY_GENERATION_ID"));
                }
                if !response.is_empty()
                    || generation
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .is_some_and(|calls| !calls.is_empty())
                {
                    generation_ids.insert(id.to_owned());
                }
            }
        }
    }
    if manifest["event_count"].as_u64() != Some(count) {
        return Err(ControlError::new("INCOMPLETE_SCORING_TRAJECTORY"));
    }
    let mut scored_ids = BTreeSet::new();
    for item in rewards {
        let fields = object(item, "GenerationReward")?;
        if fields.len() != 2
            || !fields.contains_key("generation_id")
            || !fields.contains_key("reward")
            || !item["reward"].as_f64().is_some_and(f64::is_finite)
        {
            return Err(ControlError::new("INVALID_GENERATION_REWARD"));
        }
        let id = string(item, "generation_id")?;
        if !generation_ids.contains(id) || !scored_ids.insert(id) {
            return Err(ControlError::new("INVALID_SCORED_GENERATION_ID"));
        }
    }
    Ok(())
}
