use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::contracts::{ContractSchema, object};
use crate::ports::{ArtifactStore, Backend, Cancellation, Clock, ScorerHost, ScoringContext};
use crate::{ControlError, Result};

pub struct BackendScoringContext<'a> {
    backend: &'a mut dyn Backend,
    artifacts: &'a dyn ArtifactStore,
    clock: &'a dyn Clock,
    cancellation: &'a dyn Cancellation,
    monotonic_deadline_ms: u64,
}

impl<'a> BackendScoringContext<'a> {
    pub fn new(
        backend: &'a mut dyn Backend,
        artifacts: &'a dyn ArtifactStore,
        clock: &'a dyn Clock,
        cancellation: &'a dyn Cancellation,
        monotonic_deadline_ms: u64,
    ) -> Self {
        Self {
            backend,
            artifacts,
            clock,
            cancellation,
            monotonic_deadline_ms,
        }
    }
}

impl ScoringContext for BackendScoringContext<'_> {
    fn run_harness(&mut self, request: &Value) -> Result<Value> {
        self.remaining_timeout_ms()?;
        self.backend.run_harness(request)
    }

    fn read_artifact(&self, reference: &Value) -> Result<Vec<u8>> {
        self.artifacts.read(reference)
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

pub fn run_score(
    schema: &ContractSchema,
    scorer_host: &mut dyn ScorerHost,
    scorer: &Value,
    request: &Value,
    context: &mut dyn ScoringContext,
) -> Value {
    let scorer_ref = scorer.get("implementation").cloned().unwrap_or(Value::Null);
    let completed = (|| -> Result<Value> {
        schema.validate_shape("ScoreInput", request)?;
        schema.validate_shape("ResolvedComponent", &scorer_ref)?;
        context.remaining_timeout_ms()?;
        let candidate = scorer_host.score(scorer, request, context)?;
        context.remaining_timeout_ms()?;
        validate_candidate_score(&candidate)?;
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

fn validate_candidate_score(candidate: &Value) -> Result<()> {
    let object = object(candidate, "ScorerResult")?;
    let expected: BTreeSet<&str> = ["success", "metrics", "reward", "evidence"]
        .into_iter()
        .collect();
    if object.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected {
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
