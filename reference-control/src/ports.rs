use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::contracts::{canonical_bytes, digest_bytes};
use crate::runtime::AgentRuntime;
use crate::{ControlError, Result};

pub trait Clock {
    /// Process-local monotonic milliseconds for enforcing timeouts.
    fn monotonic_ms(&self) -> u64;

    /// UTC Unix milliseconds for persisted timestamps.
    fn unix_time_ms(&self) -> u64;
}

pub trait Cancellation {
    fn is_cancelled(&self) -> bool;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NeverCancelled;

impl Cancellation for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

pub static NEVER_CANCELLED: NeverCancelled = NeverCancelled;

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn monotonic_ms(&self) -> u64 {
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        ORIGIN.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    fn unix_time_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after Unix epoch")
            .as_millis() as u64
    }
}

/// Internal byte-storage port, not a user extension or a standalone service.
/// Callers own authorization, package metadata, trajectory structure and
/// retention policy. Storage operations keep the existing ArtifactRef wire type.
pub trait FileStore {
    fn spool_root(&self) -> Option<std::path::PathBuf> {
        None
    }
    fn put_bytes(&mut self, content: &[u8], media_type: &str) -> Result<Value>;

    fn put_json(&mut self, value: &Value) -> Result<Value> {
        let content = canonical_bytes(value)?;
        self.put_bytes(&content, "application/json")
    }

    fn read(&self, reference: &Value) -> Result<Vec<u8>>;
}

/// In-memory implementation for reference validation, not persistent storage.
#[derive(Default)]
pub struct MemoryFileStore {
    objects: BTreeMap<String, Vec<u8>>,
}

impl FileStore for MemoryFileStore {
    fn put_bytes(&mut self, content: &[u8], media_type: &str) -> Result<Value> {
        let content_digest = digest_bytes(content);
        let uri = format!("memory://{content_digest}");
        self.objects.insert(uri.clone(), content.to_vec());
        Ok(serde_json::json!({
            "uri": uri,
            "digest": content_digest,
            "size_bytes": content.len(),
            "media_type": media_type
        }))
    }

    fn read(&self, reference: &Value) -> Result<Vec<u8>> {
        let uri = reference
            .get("uri")
            .and_then(Value::as_str)
            .ok_or_else(|| ControlError::new("INVALID_ARTIFACT_REFERENCE"))?;
        let content = self
            .objects
            .get(uri)
            .cloned()
            .ok_or_else(|| ControlError::new("ARTIFACT_NOT_FOUND"))?;
        if reference.get("digest").and_then(Value::as_str) != Some(digest_bytes(&content).as_str())
            || reference.get("size_bytes").and_then(Value::as_u64) != Some(content.len() as u64)
        {
            return Err(ControlError::new("ARTIFACT_INTEGRITY_MISMATCH"));
        }
        Ok(content)
    }
}

/// A backend instance owns one attempt. Dataset names are never passed here.
pub trait Backend {
    fn open(
        &mut self,
        backend: &Value,
        runtime: Option<&Value>,
        internet_access: bool,
        remaining_timeout_ms: u64,
    ) -> Result<Value>;
    /// Makes the attempt filesystem/session immutable for final scoring.
    fn freeze(&mut self, remaining_timeout_ms: u64) -> Result<()>;
    fn run_harness(&mut self, request: &Value) -> Result<Value>;
    fn close(&mut self) -> Result<()>;
}

/// Attempt-scoped tool router. Its implementation dispatches agent_state,
/// sandbox, and external_service bindings without changing the selected set.
pub trait ToolHost {
    /// Binds every selected tool to the attempt and returns the actual routable
    /// table. The Supervisor compares this table with ExecutionPlan.tools.
    fn prepare(
        &mut self,
        tools: &[Value],
        session: &Value,
        remaining_timeout_ms: u64,
    ) -> Result<Vec<Value>>;
    /// Validate selected tool argument schema before Worker counts or executes it.
    fn validate_call(&self, binding: &Value, call: &Value) -> Result<()>;
    fn call_tool(&mut self, binding: &Value, call: &Value) -> Result<Value>;
    /// Revokes admission and settles or cancels in-flight calls before backend freeze and scoring.
    /// Stops tool-owned background writers; success means no later writes.
    fn freeze(&mut self, remaining_timeout_ms: u64) -> Result<()>;
    fn close(&mut self) -> Result<()>;
}

/// Converts the selected model API using command.tools as the only tool table.
/// Descriptions/input schemas come from these registered component versions.
/// Native tool requests become ToolCall values; control fields come from the
/// command and bindings, never from model-authored configuration.
pub trait ModelProvider {
    fn generate(&mut self, request: &Value) -> Result<Value>;
}

/// Role-limited view of the managed Python host that runs the selected Environment.
pub trait EnvironmentHost {
    fn prepare(
        &mut self,
        dataset_package: &Value,
        environment: &Value,
        session: &Value,
        remaining_timeout_ms: u64,
    ) -> Result<()>;
    fn reset(&mut self, task: &Value, seed: u64, remaining_timeout_ms: u64) -> Result<Value>;
    /// Private scoring state; the host validates the package's registered state model.
    fn state_snapshot(&mut self, _remaining_timeout_ms: u64) -> Result<Option<Value>> {
        Ok(None)
    }
    fn close(&mut self) -> Result<()>;
}

/// Role-limited view of the same host protocol for the selected AgentRunner.
pub trait AgentHost {
    /// Starts the selected Agent and returns the tools actually visible to its
    /// model. This catches hidden native tools as well as missing adapters.
    fn prepare(
        &mut self,
        agent: &Value,
        tools: &[Value],
        model_id: &str,
        remaining_timeout_ms: u64,
    ) -> Result<Vec<Value>>;
    /// Returns ContentPart[]: explicit final_answer, including an intentional empty list.
    fn run_agent(
        &mut self,
        task: &Value,
        observation: &Value,
        runtime: &mut AgentRuntime<'_>,
        remaining_timeout_ms: u64,
    ) -> Result<Value>;
    fn close(&mut self) -> Result<()>;
}

pub trait ScoringContext {
    fn run_harness(&mut self, request: &Value) -> Result<Value>;
    fn read_artifact(&self, reference: &Value) -> Result<Vec<u8>>;
    fn remaining_timeout_ms(&self) -> Result<u64>;
}

/// Managed Python scoring host. One attempt invokes the selected Scorer at most once.
pub trait ScorerHost {
    fn score(
        &mut self,
        dataset_package: &Value,
        config: &Value,
        request: &Value,
        context: &mut dyn ScoringContext,
    ) -> Result<Value>;
    fn close(&mut self) -> Result<()>;
}
