//! Worker admission, durable completion and restart recovery. Component
//! execution is supplied by EpisodeSupervisor; this boundary never reselects it.
use crate::contracts::ContractSchema;
use crate::ports::FileStore;
use crate::repository::WorkerLedger;
use crate::runtime::TrajectoryWriter;
use crate::storage::EventSpool;
use crate::{ControlError, Result};
use serde_json::{Value, json};

#[allow(clippy::too_many_arguments)]
pub fn execute_dispatch(
    ledger: &mut WorkerLedger,
    dispatch: &Value,
    schema: &ContractSchema,
    key: &[u8],
    worker_id: &str,
    epoch: u64,
    now: u64,
    capacity: u64,
    execute: impl FnOnce(&Value) -> Result<Value>,
) -> Result<Option<Value>> {
    if !ledger.claim(dispatch, schema, key, worker_id, epoch, now, capacity)? {
        return Ok(None);
    }
    let result = execute(dispatch)?;
    ledger.finish(&result, schema)?;
    Ok(Some(result))
}

/// Call at startup before admitting new work. Resource cleanup must refer to
/// the saved attempt, not all host processes/containers. No model/tool/scorer
/// is invoked while recovering an interrupted attempt.
pub fn recover_interrupted(
    ledger: &mut WorkerLedger,
    schema: &ContractSchema,
    files: &mut dyn FileStore,
    now: u64,
    mut cleanup: impl FnMut(&Value) -> Result<()>,
) -> Result<usize> {
    let root = files
        .spool_root()
        .ok_or_else(|| ControlError::new("DURABLE_SPOOL_REQUIRED"))?;
    let interrupted = ledger.unfinished()?;
    for dispatch in &interrupted {
        let plan = &dispatch["plan"];
        let cleanup_status = if cleanup(plan).is_ok() {
            "completed"
        } else {
            "failed"
        };
        let spool = EventSpool::open(&root, plan)?;
        let usage = spool.recovered_usage(&dispatch["consumed_usage"])?;
        let mut trace = TrajectoryWriter::recover_partial(plan, schema, &root)?;
        let started = trace
            .events()
            .first()
            .and_then(|e| e["occurred_at_ms"].as_u64())
            .unwrap_or(now);
        let error = json!({"code":"WORKER_RESTART","phase":"persist","message":"Worker stopped before completing this attempt","retryable":cleanup_status=="completed"});
        trace.record("error", error.clone(), now)?;
        trace.record(
            "terminal",
            json!({"execution_status":"failed","usage":usage,"error":error}),
            now,
        )?;
        let reference = trace.seal(now, files)?;
        let result = json!({"run_id":plan["run_id"],"episode_id":plan["episode_id"],"attempt_id":plan["attempt_id"],
            "task_id":plan["task"]["task_id"],"execution_status":"failed","cleanup_status":cleanup_status,
            "started_at_ms":started,"finished_at_ms":now,"usage":usage,"trajectory_ref":reference,"error":error});
        ledger.finish(&result, schema)?;
    }
    Ok(interrupted.len())
}

/// Pending reports survive process exit. Only successful Server acceptance
/// removes them from the outbox; failures remain eligible for retransmission.
pub fn flush_results(
    ledger: &mut WorkerLedger,
    mut send: impl FnMut(&Value) -> Result<()>,
) -> Result<usize> {
    let reports = ledger.pending_reports()?;
    for report in &reports {
        send(report)?;
        ledger.acknowledge(report)?;
    }
    Ok(reports.len())
}
