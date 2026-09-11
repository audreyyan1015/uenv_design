use serde_json::{Value, json};
use std::path::Path;
use uenv_reference_control::contracts::ContractSchema;
use uenv_reference_control::ports::FileStore;
use uenv_reference_control::repository::{EpisodeRepository, WorkerLedger};
use uenv_reference_control::storage::{EventSpool, LocalFileStore};

fn schema() -> ContractSchema {
    let mut schema = ContractSchema::bundled();
    let dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../reference/generated/packages/gsm8k/schemas");
    for file in std::fs::read_dir(dir).unwrap() {
        schema
            .register_extension(
                serde_json::from_slice(&std::fs::read(file.unwrap().path()).unwrap()).unwrap(),
            )
            .unwrap();
    }
    schema
}
fn fixture(name: &str) -> Value {
    serde_json::from_slice(
        &std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../reference/generated/episodes/gsm8k/{name}.json")),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn full_validation_rejects_nested_wrong_types_and_unknown_business_fields() {
    let schema = schema();
    let batch = fixture("batch_request");
    schema.validate("BatchRequest", &batch).unwrap();
    let mut invalid = batch.clone();
    invalid["run_spec"]["limits"]["max_generations"] = json!("1");
    assert!(schema.validate("BatchRequest", &invalid).is_err());
    invalid = batch;
    invalid["episodes"][0]["task"]["input"]["data"]["hidden_override"] = json!(true);
    assert!(schema.validate("BatchRequest", &invalid).is_err());
}

#[test]
fn files_and_spool_survive_reopen_and_preserve_sequence_gaps() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = LocalFileStore::open(temp.path()).unwrap();
    let reference = store.put_bytes(b"actual bytes", "text/plain").unwrap();
    drop(store);
    let store = LocalFileStore::open(temp.path()).unwrap();
    assert_eq!(store.read(&reference).unwrap(), b"actual bytes");
    let mut wrong = reference.clone();
    wrong["digest"] = json!(format!("sha256:{}", "0".repeat(64)));
    assert!(store.read(&wrong).is_err());
    let plan = fixture("execution_plan");
    let root = temp.path().join("spool");
    let mut spool = EventSpool::open(&root, &plan).unwrap();
    let event = |sequence| json!({"sequence":sequence,"run_id":plan["run_id"],"episode_id":plan["episode_id"],"attempt_id":plan["attempt_id"],"task_id":plan["task"]["task_id"]});
    spool.reserve(0).unwrap();
    spool.append(&event(0)).unwrap();
    spool.reserve(1).unwrap(); // Process died after reservation, before event.
    spool.reserve(2).unwrap();
    spool.append(&event(2)).unwrap();
    assert!(spool.append(&event(3)).is_err());
    let mut wrong = event(2);
    wrong["episode_id"] = json!("other");
    assert!(spool.append(&wrong).is_err());
    drop(spool);
    let (events, next) = EventSpool::open(&root, &plan).unwrap().recover().unwrap();
    assert_eq!(next, 3);
    assert_eq!(events, vec![event(0), event(2)]);
}

#[test]
fn submit_is_atomic_and_worker_claim_is_durable_and_fenced() {
    let temp = tempfile::tempdir().unwrap();
    let schema = schema();
    let path = temp.path().join("server.db");
    let batch = fixture("batch_request");
    let plan = fixture("execution_plan");
    let mut server = EpisodeRepository::open(&path, 10).unwrap();
    let receipt = server
        .submit("alice", &batch, &schema, &|_, _| Ok(plan.clone()))
        .unwrap();
    drop(server);
    let mut server = EpisodeRepository::open(&path, 10).unwrap();
    assert_eq!(
        server
            .submit("alice", &batch, &schema, &|_, _| panic!(
                "must not re-resolve a replay"
            ))
            .unwrap(),
        receipt
    );
    assert!(
        server
            .submit("bob", &batch, &schema, &|_, _| Ok(plan.clone()))
            .is_err()
    );
    let worker = json!({"worker_id":"w1","capacity":1,"capabilities":plan["required_capabilities"],
        "components":[plan["dataset_package"],plan["agent"]["implementation"],plan["backend"]["implementation"]],
        "resource_capacity":plan["backend"]["resources"]});
    let now = plan["deadline_at_ms"].as_u64().unwrap() - 1000;
    let key = [7_u8; 32];
    let dispatch = server
        .dispatch(
            plan["episode_id"].as_str().unwrap(),
            &worker,
            &key,
            1,
            now,
            1000,
        )
        .unwrap();
    let ledger_path = temp.path().join("worker.db");
    let mut ledger = WorkerLedger::open(&ledger_path).unwrap();
    assert!(
        ledger
            .claim(&dispatch, &schema, &key, "w1", 1, now + 1, 1)
            .unwrap()
    );
    drop(ledger);
    let mut ledger = WorkerLedger::open(&ledger_path).unwrap();
    assert!(
        !ledger
            .claim(&dispatch, &schema, &key, "w1", 1, now + 2, 1)
            .unwrap()
    );
    let mut forged = dispatch.clone();
    forged["remaining_timeout_ms"] = json!(999);
    assert!(
        ledger
            .claim(&forged, &schema, &key, "w1", 1, now + 2, 1)
            .is_err()
    );
    assert!(
        ledger
            .claim(&dispatch, &schema, &key, "another-worker", 1, now + 2, 1)
            .is_err()
    );
    assert!(
        ledger
            .claim(&dispatch, &schema, &key, "w1", 2, now + 2, 1)
            .is_err()
    );
}

#[test]
fn restart_recovers_partial_evidence_then_replays_outbox_without_model_calls() {
    use uenv_reference_control::worker::{flush_results, recover_interrupted};
    let temp = tempfile::tempdir().unwrap();
    let schema = schema();
    let plan = fixture("execution_plan");
    let key = [9u8; 32];
    let now = plan["deadline_at_ms"].as_u64().unwrap() - 1000;
    let usage = json!({"generation_count":0,"tool_call_count":0,"output_token_count":0});
    let lease =
        uenv_reference_control::lease::issue(&key, "w1", &plan, 1, now + 1000, 1000, &usage)
            .unwrap();
    let dispatch =
        json!({"plan":plan,"lease":lease,"remaining_timeout_ms":1000,"consumed_usage":usage});
    let path = temp.path().join("worker.db");
    let mut ledger = WorkerLedger::open(&path).unwrap();
    ledger
        .claim(&dispatch, &schema, &key, "w1", 1, now, 1)
        .unwrap();
    let mut files = LocalFileStore::open(&temp.path().join("files")).unwrap();
    let root = files.spool_root().unwrap();
    let mut spool = EventSpool::open(&root, &plan).unwrap();
    spool.reserve(0).unwrap(); // No fabricated event for the interrupted write.
    spool
        .reserve_usage(&json!({"generation_count":1,"tool_call_count":0,"output_token_count":100}))
        .unwrap();
    drop(ledger);
    drop(spool);
    let mut ledger = WorkerLedger::open(&path).unwrap();
    let mut cleanups = 0;
    assert_eq!(
        recover_interrupted(&mut ledger, &schema, &mut files, now + 10, |saved| {
            assert_eq!(saved, &plan);
            cleanups += 1;
            Ok(())
        })
        .unwrap(),
        1
    );
    assert_eq!(cleanups, 1);
    assert!(ledger.unfinished().unwrap().is_empty());
    let reports = ledger.pending_reports().unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["result"]["usage"]["generation_count"], 1);
    let manifest: Value =
        serde_json::from_slice(&files.read(&reports[0]["result"]["trajectory_ref"]).unwrap())
            .unwrap();
    assert_eq!(manifest["trajectory_status"], "final_partial");
    let mut forged = reports[0].clone();
    forged["lease"]["token"] = json!("wrong");
    assert!(ledger.acknowledge(&forged).is_err());
    assert!(
        flush_results(&mut ledger, |_| Err(
            uenv_reference_control::ControlError::new("OFFLINE")
        ))
        .is_err()
    );
    drop(ledger);
    let mut ledger = WorkerLedger::open(&path).unwrap();
    assert_eq!(ledger.pending_reports().unwrap(), reports);
    assert_eq!(
        flush_results(&mut ledger, |report| {
            assert_eq!(report, &reports[0]);
            Ok(())
        })
        .unwrap(),
        1
    );
    assert!(ledger.pending_reports().unwrap().is_empty());
}

#[test]
fn result_transaction_uses_stored_retry_and_fences_delayed_first_attempt() {
    let temp = tempfile::tempdir().unwrap();
    let schema = schema();
    let mut batch = fixture("batch_request");
    batch["run_spec"]["retry"]["max_attempts"] = json!(2);
    let plan = fixture("execution_plan");
    let id = plan["episode_id"].as_str().unwrap();
    let run = plan["run_id"].as_str().unwrap();
    let mut server = EpisodeRepository::open(&temp.path().join("server.db"), 10).unwrap();
    server
        .submit("alice", &batch, &schema, &|_, _| Ok(plan.clone()))
        .unwrap();
    let worker = json!({"worker_id":"w1","capacity":1,"capabilities":plan["required_capabilities"],"components":[plan["dataset_package"],plan["agent"]["implementation"],plan["backend"]["implementation"]],"resource_capacity":plan["backend"]["resources"]});
    let now = plan["deadline_at_ms"].as_u64().unwrap() - 10000;
    let key = [1u8; 32];
    let first = server.dispatch(id, &worker, &key, 1, now, 5000).unwrap();
    let mut files = LocalFileStore::open(&temp.path().join("files")).unwrap();
    let trace = files.put_json(&json!({"fixture":"partial"})).unwrap();
    let result = json!({"run_id":run,"episode_id":id,"task_id":plan["task"]["task_id"],"attempt_id":1,"execution_status":"failed", "cleanup_status":"completed","trajectory_ref":trace,"started_at_ms":now,"finished_at_ms":now+1,"usage":{"generation_count":1,"tool_call_count":0,"output_token_count":1},"error":{"code":"MODEL_DISCONNECTED","phase":"model","message":"disconnected","retryable":true}});
    let report = json!({"lease":first["lease"],"result":result});
    server.accept_result(&report, &schema, now + 2).unwrap();
    assert!(server.results("alice", run).unwrap().is_empty());
    assert_eq!(
        server.statuses("alice", run).unwrap(),
        vec![(id.to_string(), "queued".to_string())]
    );
    assert_eq!(
        server
            .dispatch(id, &worker, &key, 1, now + 100, 5000)
            .unwrap_err()
            .code,
        "RETRY_BACKOFF_ACTIVE"
    );
    let second = server
        .dispatch(id, &worker, &key, 1, now + 502, 5000)
        .unwrap();
    assert_eq!(second["plan"]["attempt_id"], 2);
    assert_eq!(second["plan"]["deadline_at_ms"], plan["deadline_at_ms"]);
    assert_eq!(second["consumed_usage"], report["result"]["usage"]);
    // A lost first ACK can be recovered even after its lease expired.
    server.accept_result(&report, &schema, now + 9000).unwrap();
    let mut changed = report;
    changed["result"]["error"]["code"] = json!("different");
    assert!(server.accept_result(&changed, &schema, now + 9000).is_err());
    assert!(server.results("alice", run).unwrap().is_empty());
}
