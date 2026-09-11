//! SQLite transaction boundaries for the Server and Worker result outbox.
use crate::contracts::{ContractSchema, canonical_bytes, digest, string, u64_field};
use crate::plan::{validate_batch_submission, validate_execution_plan, validate_result_for_plan};
use crate::{ControlError, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use std::path::Path;

fn db_error(_: rusqlite::Error) -> ControlError {
    ControlError::new("DATABASE_FAILED")
}
fn decode(text: String) -> Result<Value> {
    serde_json::from_str(&text).map_err(|_| ControlError::new("CORRUPT_DATABASE_RECORD"))
}
fn encode(v: &Value) -> Result<String> {
    String::from_utf8(canonical_bytes(v)?).map_err(|_| ControlError::new("INVALID_JSON"))
}
fn open(path: &Path) -> Result<Connection> {
    let db = Connection::open(path).map_err(db_error)?;
    db.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(db_error)?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")
        .map_err(db_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| ControlError::new("DATABASE_PERMISSION_FAILED"))?;
    }
    Ok(db)
}

pub struct EpisodeRepository {
    db: Connection,
    capacity: usize,
}
impl EpisodeRepository {
    pub fn open(path: &Path, capacity: usize) -> Result<Self> {
        let db = open(path)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS runs(run_id TEXT PRIMARY KEY,owner TEXT NOT NULL,spec TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS batches(batch_id TEXT PRIMARY KEY,owner TEXT NOT NULL,digest TEXT NOT NULL,receipt TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS episodes(episode_id TEXT PRIMARY KEY,request_id TEXT UNIQUE NOT NULL,run_id TEXT NOT NULL REFERENCES runs(run_id),request TEXT NOT NULL,plan TEXT NOT NULL,state TEXT NOT NULL,lease TEXT,worker_id TEXT,result TEXT);
          CREATE TABLE IF NOT EXISTS attempts(episode_id TEXT NOT NULL,attempt_id INTEGER NOT NULL,result TEXT NOT NULL,PRIMARY KEY(episode_id,attempt_id));
          CREATE TABLE IF NOT EXISTS notifications(id INTEGER PRIMARY KEY AUTOINCREMENT,episode_id TEXT NOT NULL,payload TEXT NOT NULL,acked INTEGER NOT NULL DEFAULT 0);
          CREATE TABLE IF NOT EXISTS retry_delays(episode_id TEXT PRIMARY KEY,not_before_ms INTEGER NOT NULL);
          CREATE TABLE IF NOT EXISTS attempt_leases(episode_id TEXT NOT NULL,attempt_id INTEGER NOT NULL,lease TEXT NOT NULL,PRIMARY KEY(episode_id,attempt_id));").map_err(db_error)?;
        Ok(Self { db, capacity })
    }

    pub fn submit(
        &mut self,
        owner: &str,
        batch: &Value,
        schema: &ContractSchema,
        resolve: &dyn Fn(&Value, &Value) -> Result<Value>,
    ) -> Result<Value> {
        schema.validate("BatchRequest", batch)?;
        let run = &batch["run_spec"];
        let run_id = string(run, "run_id")?;
        let batch_id = string(batch, "batch_id")?;
        let hash = digest(batch)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let previous: Option<(String, String, String)> = tx
            .query_row(
                "SELECT owner,digest,receipt FROM batches WHERE batch_id=?",
                [batch_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(db_error)?;
        if let Some((who, stored, receipt)) = previous {
            if who != owner || stored != hash {
                return Err(ControlError::new("BATCH_IDEMPOTENCY_CONFLICT"));
            }
            return decode(receipt);
        }
        let old: Option<(String, String)> = tx
            .query_row(
                "SELECT owner,spec FROM runs WHERE run_id=?",
                [run_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(db_error)?;
        let old_spec = old.as_ref().map(|(_, s)| decode(s.clone())).transpose()?;
        if old.as_ref().is_some_and(|(who, _)| who != owner) {
            return Err(ControlError::new("RUN_ACCESS_DENIED"));
        }
        validate_batch_submission(batch, old_spec.as_ref(), schema)?;
        let queued:usize=tx.query_row("SELECT count(*) FROM episodes WHERE state IN ('queued','dispatched','cancel_requested')",[],|r|r.get(0)).map_err(db_error)?;
        let episodes = crate::contracts::array(batch, "episodes")?;
        let mut added = 0;
        if old.is_none() {
            tx.execute(
                "INSERT INTO runs VALUES(?,?,?)",
                params![run_id, owner, encode(run)?],
            )
            .map_err(db_error)?;
        }
        let mut ids = Vec::new();
        for episode in episodes {
            let id = string(episode, "episode_id")?;
            let request_id = string(episode, "request_id")?;
            let existing: Option<(String, String)> = tx
                .query_row(
                    "SELECT run_id,request FROM episodes WHERE request_id=? OR episode_id=?",
                    params![request_id, id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(db_error)?;
            if let Some((old_run, request)) = existing {
                if old_run != run_id || decode(request)? != *episode {
                    return Err(ControlError::new("EPISODE_IDEMPOTENCY_CONFLICT"));
                }
            } else {
                added += 1;
                if queued.saturating_add(added) > self.capacity {
                    return Err(ControlError::retryable("QUEUE_CAPACITY_EXCEEDED"));
                }
                let plan = resolve(episode, run)?;
                schema.validate("ExecutionPlan", &plan)?;
                validate_execution_plan(&plan, schema)?;
                tx.execute("INSERT INTO episodes(episode_id,request_id,run_id,request,plan,state) VALUES(?,?,?,?,?,'queued')",
                    params![id,request_id,run_id,encode(episode)?,encode(&plan)?]).map_err(db_error)?;
            }
            ids.push(id);
        }
        let receipt = json!({"batch_id":batch_id,"episode_ids":ids});
        tx.execute(
            "INSERT INTO batches VALUES(?,?,?,?)",
            params![batch_id, owner, hash, encode(&receipt)?],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(receipt)
    }

    /// Reservation and lease become durable before any Worker side effect.
    pub fn dispatch(
        &mut self,
        episode_id: &str,
        worker: &Value,
        key: &[u8],
        epoch: u64,
        now: u64,
        lease_ms: u64,
    ) -> Result<Value> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let (plan, state): (String, String) = tx
            .query_row(
                "SELECT plan,state FROM episodes WHERE episode_id=?",
                [episode_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(db_error)?;
        if state != "queued" {
            return Err(ControlError::new("EPISODE_NOT_QUEUED"));
        }
        let not_before: Option<u64> = tx
            .query_row(
                "SELECT not_before_ms FROM retry_delays WHERE episode_id=?",
                [episode_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if not_before.is_some_and(|value| now < value) {
            return Err(ControlError::new("RETRY_BACKOFF_ACTIVE"));
        }
        let plan = decode(plan)?;
        let required = crate::contracts::array(&plan, "required_capabilities")?;
        let available = crate::contracts::array(worker, "capabilities")?;
        if required.iter().any(|c| !available.contains(c)) {
            return Err(ControlError::new("WORKER_CAPABILITY_MISMATCH"));
        }
        let components = crate::contracts::array(worker, "components")?;
        for selected in [
            &plan["dataset_package"],
            &plan["agent"]["implementation"],
            &plan["backend"]["implementation"],
        ]
        .into_iter()
        .chain(
            crate::contracts::array(&plan, "tools")?
                .iter()
                .map(|v| &v["implementation"]),
        ) {
            if !components.contains(selected) {
                return Err(ControlError::new("WORKER_COMPONENT_UNAVAILABLE"));
            }
        }
        let worker_id = string(worker, "worker_id")?;
        let mut statement=tx.prepare("SELECT plan FROM episodes WHERE worker_id=? AND state IN ('dispatched','cancel_requested')").map_err(db_error)?;
        let active = statement
            .query_map([worker_id], |r| r.get::<_, String>(0))
            .map_err(db_error)?
            .map(|v| decode(v.map_err(db_error)?))
            .collect::<Result<Vec<_>>>()?;
        if active.len() as u64 >= u64_field(worker, "capacity")? {
            return Err(ControlError::new("WORKER_CAPACITY_EXCEEDED"));
        }
        for field in ["cpu_cores", "memory_bytes", "disk_bytes", "process_limit"] {
            let need = plan["backend"]["resources"][field]
                .as_f64()
                .ok_or_else(|| ControlError::new("INVALID_RESOURCE_REQUIREMENT"))?;
            let used: f64 = active
                .iter()
                .map(|p| {
                    p["backend"]["resources"][field]
                        .as_f64()
                        .unwrap_or(f64::INFINITY)
                })
                .sum();
            if need + used > worker["resource_capacity"][field].as_f64().unwrap_or(0.0) {
                return Err(ControlError::new("WORKER_RESOURCES_EXCEEDED"));
            }
        }
        drop(statement);
        let remaining = u64_field(&plan, "deadline_at_ms")?.saturating_sub(now);
        if remaining == 0 {
            return Err(ControlError::new("EPISODE_TIMEOUT"));
        }
        let previous: Option<String> = tx
            .query_row(
                "SELECT result FROM attempts WHERE episode_id=? ORDER BY attempt_id DESC LIMIT 1",
                [episode_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        let usage = match previous {
            Some(value) => decode(value)?["usage"].clone(),
            None => json!({"generation_count":0,"tool_call_count":0,"output_token_count":0}),
        };
        let lease = crate::lease::issue(
            key,
            worker_id,
            &plan,
            epoch,
            now.saturating_add(lease_ms)
                .min(u64_field(&plan, "deadline_at_ms")?),
            remaining,
            &usage,
        )?;
        tx.execute(
            "UPDATE episodes SET state='dispatched',lease=?,worker_id=? WHERE episode_id=?",
            params![encode(&lease)?, worker_id, episode_id],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(
            json!({"plan":plan,"lease":lease,"remaining_timeout_ms":remaining,"consumed_usage":usage}),
        )
    }

    pub fn accept_result(
        &mut self,
        report: &Value,
        schema: &ContractSchema,
        now: u64,
    ) -> Result<()> {
        schema.validate("ResultReport", report)?;
        let result = &report["result"];
        let episode_id = string(result, "episode_id")?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let accepted:Option<(String,String)>=tx.query_row("SELECT a.result,l.lease FROM attempts a JOIN attempt_leases l ON a.episode_id=l.episode_id AND a.attempt_id=l.attempt_id WHERE a.episode_id=? AND a.attempt_id=?",
            params![episode_id,u64_field(result,"attempt_id")?],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(db_error)?;
        if let Some((saved, lease)) = accepted {
            return if decode(saved)? == *result && decode(lease)? == report["lease"] {
                Ok(())
            } else {
                Err(ControlError::new("RESULT_ALREADY_ACCEPTED"))
            };
        }
        let (plan, state, lease, previous): (String, String, Option<String>, Option<String>) = tx
            .query_row(
                "SELECT plan,state,lease,result FROM episodes WHERE episode_id=?",
                [episode_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(db_error)?;
        let plan = decode(plan)?;
        if lease.map(decode).transpose()?.as_ref() != Some(&report["lease"]) {
            return Err(ControlError::new("STALE_RESULT_LEASE"));
        }
        if let Some(previous) = previous {
            return if decode(previous)? == *result {
                Ok(())
            } else {
                Err(ControlError::new("RESULT_ALREADY_ACCEPTED"))
            };
        }
        if u64_field(&report["lease"], "expires_at_ms")? <= now {
            return Err(ControlError::new("STALE_RESULT_LEASE"));
        }
        if state == "cancel_requested" && result["execution_status"] != "cancelled" {
            return Err(ControlError::new("CANCELLATION_WINS"));
        }
        if state != "dispatched" && state != "cancel_requested" {
            return Err(ControlError::new("INVALID_EPISODE_STATE"));
        }
        validate_result_for_plan(result, &plan, schema)?;
        let encoded = encode(result)?;
        tx.execute(
            "INSERT INTO attempts VALUES(?,?,?)",
            params![episode_id, u64_field(result, "attempt_id")?, encoded],
        )
        .map_err(db_error)?;
        tx.execute(
            "INSERT INTO attempt_leases VALUES(?,?,?)",
            params![
                episode_id,
                u64_field(result, "attempt_id")?,
                encode(&report["lease"])?
            ],
        )
        .map_err(db_error)?;
        let run: String = tx
            .query_row(
                "SELECT spec FROM runs WHERE run_id=?",
                [string(&plan, "run_id")?],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        let run = decode(run)?;
        let retry = &run["retry"];
        let attempt = u64_field(&plan, "attempt_id")?;
        let delay = u64_field(retry, "initial_backoff_ms")?
            .saturating_mul(2_u64.saturating_pow(attempt.saturating_sub(1).min(63) as u32))
            .min(u64_field(retry, "max_backoff_ms")?);
        if state == "dispatched"
            && result["execution_status"] == "failed"
            && result["error"]["retryable"] == true
            && result["cleanup_status"] == "completed"
            && attempt < u64_field(retry, "max_attempts")?
            && now.saturating_add(delay) < u64_field(&plan, "deadline_at_ms")?
        {
            let next = crate::plan::retry_execution_plan(
                &plan,
                schema,
                u64_field(retry, "max_attempts")?,
            )?;
            tx.execute("UPDATE episodes SET plan=?,state='queued',lease=NULL,worker_id=NULL,result=NULL WHERE episode_id=?",params![encode(&next)?,episode_id]).map_err(db_error)?;
            tx.execute(
                "INSERT OR REPLACE INTO retry_delays VALUES(?,?)",
                params![episode_id, now.saturating_add(delay)],
            )
            .map_err(db_error)?;
            return tx.commit().map_err(db_error);
        }
        tx.execute(
            "UPDATE episodes SET state=?,result=? WHERE episode_id=?",
            params![string(result, "execution_status")?, encoded, episode_id],
        )
        .map_err(db_error)?;
        tx.execute(
            "INSERT INTO notifications(episode_id,payload) VALUES(?,?)",
            params![episode_id, encoded],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)
    }

    pub fn cancel_run(&mut self, owner: &str, run_id: &str) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let who: String = tx
            .query_row("SELECT owner FROM runs WHERE run_id=?", [run_id], |r| {
                r.get(0)
            })
            .map_err(db_error)?;
        if who != owner {
            return Err(ControlError::new("RUN_ACCESS_DENIED"));
        }
        tx.execute("UPDATE episodes SET state=CASE WHEN state='queued' THEN 'cancelled' ELSE 'cancel_requested' END WHERE run_id=? AND state IN ('queued','dispatched')",[run_id]).map_err(db_error)?;
        tx.commit().map_err(db_error)
    }

    pub fn statuses(&self, owner: &str, run_id: &str) -> Result<Vec<(String, String)>> {
        let who: String = self
            .db
            .query_row("SELECT owner FROM runs WHERE run_id=?", [run_id], |r| {
                r.get(0)
            })
            .map_err(db_error)?;
        if who != owner {
            return Err(ControlError::new("RUN_ACCESS_DENIED"));
        }
        let mut q = self
            .db
            .prepare("SELECT episode_id,state FROM episodes WHERE run_id=? ORDER BY episode_id")
            .map_err(db_error)?;
        q.query_map([run_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(db_error)?
            .map(|r| r.map_err(db_error))
            .collect()
    }

    pub fn results(&self, owner: &str, run_id: &str) -> Result<Vec<Value>> {
        let who: String = self
            .db
            .query_row("SELECT owner FROM runs WHERE run_id=?", [run_id], |r| {
                r.get(0)
            })
            .map_err(db_error)?;
        if who != owner {
            return Err(ControlError::new("RUN_ACCESS_DENIED"));
        }
        let mut query=self.db.prepare("SELECT result FROM episodes WHERE run_id=? AND result IS NOT NULL ORDER BY episode_id").map_err(db_error)?;
        query
            .query_map([run_id], |r| r.get::<_, String>(0))
            .map_err(db_error)?
            .map(|v| decode(v.map_err(db_error)?))
            .collect()
    }
}

/// A restart never re-executes an accepted dispatch. Unfinished rows need
/// Server reconciliation; finished rows replay their original result until ACK.
pub struct WorkerLedger {
    db: Connection,
}
impl WorkerLedger {
    pub fn open(path: &Path) -> Result<Self> {
        let db = open(path)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS executions(episode_id TEXT NOT NULL,attempt_id INTEGER NOT NULL,dispatch TEXT NOT NULL,result TEXT,acked INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(episode_id,attempt_id));").map_err(db_error)?;
        Ok(Self { db })
    }
    #[allow(clippy::too_many_arguments)]
    pub fn claim(
        &mut self,
        dispatch: &Value,
        schema: &ContractSchema,
        key: &[u8],
        worker_id: &str,
        epoch: u64,
        now: u64,
        capacity: u64,
    ) -> Result<bool> {
        schema.validate("DispatchRequest", dispatch)?;
        validate_execution_plan(&dispatch["plan"], schema)?;
        crate::lease::verify(key, worker_id, dispatch, epoch, now)?;
        let plan = &dispatch["plan"];
        let id = string(plan, "episode_id")?;
        let attempt = u64_field(plan, "attempt_id")?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let old: Option<String> = tx
            .query_row(
                "SELECT dispatch FROM executions WHERE episode_id=? AND attempt_id=?",
                params![id, attempt],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if let Some(old) = old {
            let old = decode(old)?;
            if old["plan"] != *plan || old["lease"] != dispatch["lease"] {
                return Err(ControlError::new("DISPATCH_IDENTITY_CONFLICT"));
            }
            return Ok(false);
        }
        let active: u64 = tx
            .query_row(
                "SELECT count(*) FROM executions WHERE result IS NULL",
                [],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        if active >= capacity {
            return Err(ControlError::new("WORKER_CAPACITY_EXCEEDED"));
        }
        tx.execute(
            "INSERT INTO executions(episode_id,attempt_id,dispatch) VALUES(?,?,?)",
            params![id, attempt, encode(dispatch)?],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(true)
    }
    pub fn finish(&mut self, result: &Value, schema: &ContractSchema) -> Result<()> {
        schema.validate("EpisodeResult", result)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let id = string(result, "episode_id")?;
        let attempt = u64_field(result, "attempt_id")?;
        let (dispatch, old): (String, Option<String>) = tx
            .query_row(
                "SELECT dispatch,result FROM executions WHERE episode_id=? AND attempt_id=?",
                params![id, attempt],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(db_error)?;
        validate_result_for_plan(result, &decode(dispatch)?["plan"], schema)?;
        if let Some(old) = old {
            return if decode(old)? == *result {
                Ok(())
            } else {
                Err(ControlError::new("IMMUTABLE_RESULT_CONFLICT"))
            };
        }
        tx.execute(
            "UPDATE executions SET result=? WHERE episode_id=? AND attempt_id=?",
            params![encode(result)?, id, attempt],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)
    }
    pub fn pending_reports(&self) -> Result<Vec<Value>> {
        let mut q = self
            .db
            .prepare("SELECT dispatch,result FROM executions WHERE result IS NOT NULL AND acked=0")
            .map_err(db_error)?;
        q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(db_error)?
            .map(|row| {
                let (dispatch, result) = row.map_err(db_error)?;
                Ok(json!({"lease":decode(dispatch)?["lease"],"result":decode(result)?}))
            })
            .collect()
    }
    pub fn acknowledge(&mut self, report: &Value) -> Result<()> {
        let result = &report["result"];
        let dispatch: String = self
            .db
            .query_row(
                "SELECT dispatch FROM executions WHERE episode_id=? AND attempt_id=?",
                params![
                    string(result, "episode_id")?,
                    u64_field(result, "attempt_id")?
                ],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        if decode(dispatch)?["lease"] != report["lease"] {
            return Err(ControlError::new("ACK_LEASE_MISMATCH"));
        }
        let changed = self
            .db
            .execute(
                "UPDATE executions SET acked=1 WHERE episode_id=? AND attempt_id=? AND result=?",
                params![
                    string(result, "episode_id")?,
                    u64_field(result, "attempt_id")?,
                    encode(result)?
                ],
            )
            .map_err(db_error)?;
        if changed != 1 {
            return Err(ControlError::new("ACK_RESULT_MISMATCH"));
        }
        Ok(())
    }
    pub fn unfinished(&self) -> Result<Vec<Value>> {
        let mut q = self
            .db
            .prepare("SELECT dispatch FROM executions WHERE result IS NULL")
            .map_err(db_error)?;
        q.query_map([], |r| r.get::<_, String>(0))
            .map_err(db_error)?
            .map(|r| decode(r.map_err(db_error)?))
            .collect()
    }
}
