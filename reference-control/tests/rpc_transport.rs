use serde_json::json;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use uenv_reference_control::ControlError;
use uenv_reference_control::rpc::RpcProcess;

fn process() -> std::rc::Rc<RpcProcess> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let python = std::env::var("UENV_INTEGRATION_PYTHON").unwrap_or_else(|_| "python".into());
    RpcProcess::spawn(Command::new(python).arg(root.join("reference/sdk/tests/rpc_fixture.py")))
        .unwrap()
}

#[test]
fn duplex_rpc_nested_callback_and_errors() {
    let peer = process();
    let value = peer
        .call_with(
            "nested",
            json!({"value":42}),
            5000,
            &mut |method, params| {
                if method == "__poll" {
                    return Ok(serde_json::Value::Null);
                }
                assert_eq!(method, "step");
                // Worker calls Python back while Python is awaiting Worker step.
                peer.call("echo", params.clone(), 1000)
            },
        )
        .unwrap();
    assert_eq!(value, json!({"value":42}));
    let denied = peer
        .call_with("nested", json!({}), 1000, &mut |method, _| {
            if method == "__poll" {
                return Ok(serde_json::Value::Null);
            }
            Err(ControlError::new("TOOL_CALL_LIMIT"))
        })
        .unwrap_err();
    assert_eq!(denied.code, "TOOL_CALL_LIMIT");
    assert_eq!(peer.call("echo", json!(7), 1000).unwrap(), 7);
}

#[test]
fn rpc_timeout_kills_host_and_disallows_reuse() {
    let peer = process();
    peer.call("echo", json!({}), 5000).unwrap();
    let start = Instant::now();
    assert_eq!(
        peer.call("sleep", json!({"seconds":30}), 80)
            .unwrap_err()
            .code,
        "RPC_TIMEOUT"
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(
        peer.call("echo", json!({}), 1000).unwrap_err().code,
        "RPC_CLOSED"
    );
}

#[test]
fn rpc_cancellation_interrupts_inflight_component() {
    let peer = process();
    peer.call("echo", json!({}), 5000).unwrap();
    let cancellation = peer.cancellation();
    let trigger = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        cancellation.cancel();
    });
    let start = Instant::now();
    assert_eq!(
        peer.call("sleep", json!({"seconds":30}), 20000)
            .unwrap_err()
            .code,
        "EPISODE_CANCELLED"
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    trigger.join().unwrap();
}

#[test]
fn unexpected_component_exit_is_reported_without_hanging() {
    let peer = process();
    peer.call("echo", json!({}), 5000).unwrap();
    assert_eq!(
        peer.call("crash", json!({}), 2000).unwrap_err().code,
        "RPC_CLOSED"
    );
}

#[test]
fn child_that_never_reads_cannot_block_worker_write_deadline() {
    let python = std::env::var("UENV_INTEGRATION_PYTHON").unwrap_or_else(|_| "python".into());
    let peer = RpcProcess::spawn(Command::new(python).args(["-c", "import time; time.sleep(30)"]))
        .unwrap();
    let start = Instant::now();
    assert_eq!(
        peer.call("echo", json!("x".repeat(1024 * 1024)), 100)
            .unwrap_err()
            .code,
        "RPC_TIMEOUT"
    );
    assert!(start.elapsed() < Duration::from_secs(3));
}
