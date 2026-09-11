#![cfg(target_os = "linux")]
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use uenv_reference_control::backends::{Engine, LinuxBackend};
use uenv_reference_control::ports::Backend;
use uenv_reference_control::process::run;

fn shell(script: &str) -> Vec<String> {
    vec!["/bin/sh".into(), "-c".into(), script.into()]
}

#[test]
fn process_supervisor_enforces_timeout_and_output_bound() {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "sleep 30 & wait"]);
    let start = std::time::Instant::now();
    assert_eq!(
        run(&mut cmd, 100, 4096).unwrap_err().code,
        "PROCESS_TIMEOUT"
    );
    assert!(start.elapsed().as_secs() < 3);
    let mut cmd = Command::new("/bin/sh");
    cmd.args([
        "-c",
        "while true; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'; done",
    ]);
    assert_eq!(
        run(&mut cmd, 3000, 4096).unwrap_err().code,
        "PROCESS_OUTPUT_LIMIT"
    );
}

/// Opt-in: uses only the administrator supplied independent session directory,
/// a curated test rootfs and delegated cgroup parent. No production services.
#[test]
#[ignore = "requires Linux sandbox tools and a dedicated test directory"]
fn real_backends_isolate_files_network_processes_and_freeze() {
    let base =
        PathBuf::from(std::env::var("UENV_BACKEND_TEST_ROOT").expect("explicit test directory"));
    let mode = std::env::var("UENV_BACKEND_TEST_ENGINE").unwrap_or("process".into());
    let (engine, id, config, runtime) = match mode.as_str() {
        "process" => (
            Engine::Process {
                profile_id: "test".into(),
                rootfs: base.join("rootfs"),
                bubblewrap: base.join("tools/usr/bin/bwrap"),
                cgroup_parent: PathBuf::from("/sys/fs/cgroup"),
            },
            "backends/process",
            json!({"runtime_profile":"test"}),
            None,
        ),
        "docker" => (
            Engine::Docker {
                executable: base.join("tools/docker/docker"),
                socket: base.join("docker.sock"),
            },
            "backends/docker",
            json!({}),
            Some(json!({"image":std::env::var("UENV_BACKEND_TEST_IMAGE").unwrap()})),
        ),
        "podman" => (
            Engine::Podman {
                executable: base.join("tools/podman-test"),
            },
            "backends/podman",
            json!({}),
            Some(json!({"image":std::env::var("UENV_BACKEND_TEST_IMAGE").unwrap()})),
        ),
        _ => panic!("unknown engine"),
    };
    let spec = json!({"implementation":{"id":id}, "config":{"data":config},
        "resources":{"cpu_cores":0.5,"memory_bytes":134217728,"process_limit":24,"disk_bytes":4194304}});
    let mut backend = LinuxBackend::new(engine, &base.join("sessions")).unwrap();
    let sentinel = base.join("host-private-sentinel");
    std::fs::write(&sentinel, "test-only, not a real secret").unwrap();
    assert_eq!(
        backend
            .open(&spec, runtime.as_ref(), true, 10000)
            .unwrap_err()
            .code,
        "CONTROLLED_EGRESS_UNAVAILABLE"
    );
    let session = backend.open(&spec, runtime.as_ref(), false, 10000).unwrap();
    if mode == "process" {
        let group = PathBuf::from("/sys/fs/cgroup").join(session["session_id"].as_str().unwrap());
        for (field, expected) in [
            ("cpu.max", "50000 100000"),
            ("memory.max", "134217728"),
            ("memory.swap.max", "0"),
            ("pids.max", "24"),
        ] {
            assert_eq!(
                std::fs::read_to_string(group.join(field)).unwrap().trim(),
                expected
            );
        }
    }
    let hidden = backend
        .execute(
            &[
                "/bin/test".into(),
                "!".into(),
                "-e".into(),
                sentinel.to_string_lossy().into_owned(),
            ],
            3000,
        )
        .unwrap();
    assert_eq!(hidden.exit_code, 0, "host files must not be visible");
    let output = backend.execute(&shell("set -e; id -u; cat /proc/self/status | grep '^CapEff:'; test ! -e /var/run/docker.sock; test ! -w /sys/fs/cgroup/cgroup.procs"), 3000).unwrap();
    assert_eq!(output.exit_code, 0);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("65534\n"), "{text}");
    assert!(text.contains("CapEff:\t0000000000000000"), "{text}");
    let output = backend
        .execute(&shell("cat /proc/net/route"), 3000)
        .unwrap();
    assert_eq!(output.exit_code, 0);
    assert_eq!(String::from_utf8(output.stdout).unwrap().lines().count(), 1);
    assert_eq!(
        backend
            .execute(
                &shell("printf candidate > answer; chmod 600 answer; ln -s answer alias"),
                3000
            )
            .unwrap()
            .exit_code,
        0
    );
    assert_ne!(
        backend
            .execute(
                &shell("dd if=/dev/zero of=too-large bs=1048576 count=8 2>/dev/null"),
                3000
            )
            .unwrap()
            .exit_code,
        0
    );
    backend.execute(&shell("rm -f too-large; setsid sh -c 'sleep 1; echo leaked > late' >/dev/null 2>&1 & sleep 30"), 100).unwrap_err();
    std::thread::sleep(std::time::Duration::from_millis(1200));
    assert_eq!(
        backend
            .execute(&shell("test ! -e late"), 3000)
            .unwrap()
            .exit_code,
        0
    );
    backend.freeze(5000).unwrap();
    assert_eq!(
        backend.execute(&shell("true"), 1000).unwrap_err().code,
        "BACKEND_FROZEN"
    );
    let output = backend
        .inspect_frozen(&shell("cat alias; echo changed > answer"), 3000)
        .unwrap();
    assert_eq!(output.stdout, b"candidate");
    assert_ne!(output.exit_code, 0);
    backend.close().unwrap();
    backend.close().unwrap();
}

#[test]
fn unverified_podman_is_rejected_before_starting_resources() {
    let parent = std::env::temp_dir().join(format!("uenv-podman-admission-{}", std::process::id()));
    std::fs::create_dir(&parent).unwrap();
    {
        let mut backend = LinuxBackend::new(
            Engine::Podman {
                executable: PathBuf::from("/nonexistent/podman"),
            },
            &parent,
        )
        .unwrap();
        assert_eq!(
            backend
                .open(&json!({}), None, false, 1000)
                .unwrap_err()
                .code,
            "PODMAN_DRIVER_NOT_VALIDATED"
        );
        backend.close().unwrap();
    }
    assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
    std::fs::remove_dir(parent).unwrap();
}
