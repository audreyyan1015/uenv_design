//! Linux session drivers. All paths/executables here are administrator-owned;
//! only BackendSpec.resources and the resolved runtime come from the plan.
//! ComponentHost RPC and official harness adapters are separate integration work.
use crate::contracts::u64_field;
use crate::ports::Backend;
use crate::process::{CommandOutput, run};
use crate::{ControlError, Result};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, chown, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: usize = 1024 * 1024;
static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Platform deployment inputs, never extra user fields in RunSpec.
pub enum Engine {
    Process {
        profile_id: String,
        rootfs: PathBuf,
        bubblewrap: PathBuf,
        cgroup_parent: PathBuf,
    },
    Docker {
        executable: PathBuf,
        socket: PathBuf,
    },
    Podman {
        executable: PathBuf,
    },
}

/// One instance per attempt; shared implementation prevents three sets of
/// workspace, freeze, quota, timeout and cleanup semantics.
pub struct LinuxBackend {
    engine: Engine,
    root: PathBuf,
    session_id: String,
    cgroup: Option<PathBuf>,
    mounted: Vec<PathBuf>,
    resources: Option<Value>,
    image: Option<String>,
    frozen: bool,
    container: Option<String>,
}

fn io_error(_: std::io::Error) -> ControlError {
    ControlError::new("BACKEND_IO_FAILED")
}

fn command(executable: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(executable);
    cmd.env_clear().env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
    cmd
}

fn checked(cmd: &mut Command, timeout_ms: u64) -> Result<CommandOutput> {
    let output = run(cmd, timeout_ms, OUTPUT_LIMIT)?;
    if output.exit_code != 0 {
        return Err(ControlError::new("BACKEND_COMMAND_FAILED"));
    }
    Ok(output)
}

impl LinuxBackend {
    pub fn new(engine: Engine, session_parent: &Path) -> Result<Self> {
        let parent = session_parent.canonicalize().map_err(io_error)?;
        let session_id = format!(
            "uenv-{}-{}",
            std::process::id(),
            SESSION_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let root = parent.join(&session_id);
        fs::create_dir(&root).map_err(io_error)?; // Never adopt an existing directory.
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
        Ok(Self {
            engine,
            root,
            session_id,
            cgroup: None,
            mounted: vec![],
            resources: None,
            image: None,
            frozen: false,
            container: None,
        })
    }

    fn engine_command(&self) -> Result<Command> {
        match &self.engine {
            Engine::Docker { executable, socket } => {
                let mut cmd = command(executable);
                cmd.arg("--host")
                    .arg(format!("unix://{}", socket.display()));
                Ok(cmd)
            }
            Engine::Podman { executable } => Ok(command(executable)),
            _ => Err(ControlError::new("NOT_CONTAINER_BACKEND")),
        }
    }

    fn mount_workspace(&mut self, name: &str, disk_bytes: u64, timeout: u64) -> Result<PathBuf> {
        let path = self.root.join(name);
        fs::create_dir(&path).map_err(io_error)?;
        checked(
            command("/usr/bin/mount")
                .args(["-t", "tmpfs", "-o"])
                .arg(format!("size={disk_bytes},mode=1777,nosuid,nodev"))
                .arg("tmpfs")
                .arg(&path),
            timeout,
        )?;
        self.mounted.push(path.clone());
        Ok(path)
    }

    /// Command execution for the platform's session/ToolHost integration. The
    /// same argv interface works with process, Docker and Podman.
    pub fn execute(&mut self, argv: &[String], timeout_ms: u64) -> Result<CommandOutput> {
        if self.frozen {
            return Err(ControlError::new("BACKEND_FROZEN"));
        }
        self.execute_in(argv, "workspace", false, timeout_ms)
    }

    /// Read the fixed candidate after freeze. Does not expose a scoring writer
    /// to Agent and cannot be used to install hidden tests in its workspace.
    pub fn inspect_frozen(&mut self, argv: &[String], timeout_ms: u64) -> Result<CommandOutput> {
        if !self.frozen {
            return Err(ControlError::new("BACKEND_NOT_FROZEN"));
        }
        self.execute_in(argv, "candidate", true, timeout_ms)
    }

    fn execute_in(
        &mut self,
        argv: &[String],
        directory: &str,
        readonly: bool,
        timeout: u64,
    ) -> Result<CommandOutput> {
        if argv.is_empty() || argv[0].is_empty() {
            return Err(ControlError::new("EMPTY_COMMAND"));
        }
        let resources = self
            .resources
            .as_ref()
            .ok_or_else(|| ControlError::new("BACKEND_NOT_OPEN"))?;
        let workspace = self.root.join(directory);
        match &self.engine {
            Engine::Process {
                rootfs, bubblewrap, ..
            } => {
                let cgroup = self
                    .cgroup
                    .as_ref()
                    .ok_or_else(|| ControlError::new("CGROUP_UNAVAILABLE"))?;
                // The fixed launcher moves itself before launching any task
                // code. Its arguments are paths, never interpolated shell text.
                let mut cmd = command("/bin/sh");
                cmd.args([
                    "-c",
                    "printf '%s' \"$$\" > \"$1/cgroup.procs\" || exit 125; shift; exec \"$@\"",
                    "uenv",
                ])
                .arg(cgroup)
                .arg(bubblewrap)
                .args([
                    "--unshare-all",
                    "--unshare-user",
                    "--die-with-parent",
                    "--new-session",
                    "--disable-userns",
                    "--cap-drop",
                    "ALL",
                    "--clearenv",
                ])
                .args(["--ro-bind"])
                .arg(rootfs)
                .arg("/")
                .arg(if readonly { "--ro-bind" } else { "--bind" })
                .arg(&workspace)
                .arg("/workspace")
                .args([
                    "--proc",
                    "/proc",
                    "--dev",
                    "/dev",
                    "--tmpfs",
                    "/tmp",
                    "--chdir",
                    "/workspace",
                    "--setenv",
                    "PATH",
                    "/usr/bin:/bin",
                    "--uid",
                    "65534",
                    "--gid",
                    "65534",
                    "--",
                ])
                .args(argv);
                let result = run(&mut cmd, timeout, OUTPUT_LIMIT);
                // Kill any remaining namespace descendants, including setsid.
                fs::write(cgroup.join("cgroup.kill"), "1").map_err(io_error)?;
                result
            }
            Engine::Docker { .. } | Engine::Podman { .. } => {
                let mut cmd = self.engine_command()?;
                let name = format!("{}-command", self.session_id);
                cmd.args(["run", "--rm", "--name"])
                    .arg(&name)
                    .args([
                        "--pull",
                        "never",
                        "--network",
                        "none",
                        "--read-only",
                        "--cap-drop",
                        "ALL",
                        "--security-opt",
                        "no-new-privileges",
                        "--user",
                        "65534:65534",
                    ])
                    .arg("--cpus")
                    .arg(resources["cpu_cores"].to_string())
                    .arg("--memory")
                    .arg(resources["memory_bytes"].to_string())
                    .arg("--memory-swap")
                    .arg(resources["memory_bytes"].to_string())
                    .arg("--pids-limit")
                    .arg(resources["process_limit"].to_string())
                    .args([
                        "--tmpfs",
                        "/tmp:rw,nosuid,nodev,size=16777216",
                        "--workdir",
                        "/workspace",
                        "--mount",
                    ])
                    .arg(format!(
                        "type=bind,src={},dst=/workspace{}",
                        workspace.display(),
                        if readonly { ",readonly" } else { "" }
                    ))
                    .arg("--entrypoint")
                    .arg(&argv[0])
                    .arg(self.image.as_ref().unwrap())
                    .args(&argv[1..]);
                self.container = Some(name);
                let result = run(&mut cmd, timeout, OUTPUT_LIMIT);
                self.remove_container()?;
                result
            }
        }
    }

    fn remove_container(&mut self) -> Result<()> {
        if let Some(name) = &self.container {
            // Docker/Podman rm -f is idempotent only after confirming absence.
            let output = run(
                self.engine_command()?.args(["rm", "-f", name]),
                5000,
                OUTPUT_LIMIT,
            )?;
            if output.exit_code != 0 {
                let inspect = run(
                    self.engine_command()?.args([
                        "container",
                        "ls",
                        "-aq",
                        "--filter",
                        &format!("name={name}"),
                    ]),
                    5000,
                    OUTPUT_LIMIT,
                )?;
                if inspect.exit_code != 0 || !inspect.stdout.is_empty() {
                    return Err(ControlError::new("CONTAINER_CLEANUP_FAILED"));
                }
            }
            self.container = None;
        }
        Ok(())
    }
}

impl Backend for LinuxBackend {
    fn open(
        &mut self,
        backend: &Value,
        runtime: Option<&Value>,
        internet_access: bool,
        timeout_ms: u64,
    ) -> Result<Value> {
        // The isolated Podman 4.9/conmon test exposed a stuck attached client
        // and orphaned runtime helpers after a failed create. Do not advertise
        // a usable sandbox until cleanup and completion are proven together.
        if matches!(self.engine, Engine::Podman { .. }) {
            return Err(ControlError::new("PODMAN_DRIVER_NOT_VALIDATED"));
        }
        if self.resources.is_some() {
            return Err(ControlError::new("BACKEND_ALREADY_OPEN"));
        }
        if internet_access {
            return Err(ControlError::new("CONTROLLED_EGRESS_UNAVAILABLE"));
        }
        let started = Instant::now();
        let remaining = || {
            timeout_ms
                .checked_sub(started.elapsed().as_millis() as u64)
                .filter(|n| *n > 0)
                .ok_or_else(|| ControlError::new("BACKEND_TIMEOUT"))
        };
        let resources = &backend["resources"];
        let cpu = resources["cpu_cores"]
            .as_f64()
            .filter(|v| v.is_finite() && *v >= 0.01 && *v <= 1024.0)
            .ok_or_else(|| ControlError::new("INVALID_CPU_LIMIT"))?;
        for field in ["memory_bytes", "process_limit", "disk_bytes"] {
            if u64_field(resources, field)? == 0 {
                return Err(ControlError::new("INVALID_RESOURCE_LIMIT"));
            }
        }
        match &self.engine {
            Engine::Process {
                profile_id,
                rootfs,
                bubblewrap,
                cgroup_parent,
            } => {
                if backend
                    .pointer("/implementation/id")
                    .and_then(Value::as_str)
                    != Some("backends/process")
                    || backend
                        .pointer("/config/data/runtime_profile")
                        .and_then(Value::as_str)
                        != Some(profile_id.as_str())
                    || runtime.is_some()
                    || !rootfs.is_dir()
                    || !bubblewrap.is_file()
                {
                    return Err(ControlError::new("PROCESS_RUNTIME_UNAVAILABLE"));
                }
                let group = cgroup_parent
                    .canonicalize()
                    .map_err(io_error)?
                    .join(&self.session_id);
                fs::create_dir(&group).map_err(io_error)?;
                self.cgroup = Some(group.clone());
                // A missing delegated controller must fail; never fall back to
                // an ordinary unbounded child or change parent cgroup policy.
                for field in [
                    "cpu.max",
                    "memory.max",
                    "memory.swap.max",
                    "pids.max",
                    "cgroup.kill",
                ] {
                    if !group.join(field).exists() {
                        return Err(ControlError::new("CGROUP_CONTROLLER_UNAVAILABLE"));
                    }
                }
                fs::write(
                    group.join("cpu.max"),
                    format!("{} 100000", (cpu * 100000.0).ceil().max(1000.0) as u64),
                )
                .map_err(io_error)?;
                fs::write(
                    group.join("memory.max"),
                    resources["memory_bytes"].to_string(),
                )
                .map_err(io_error)?;
                fs::write(group.join("memory.swap.max"), "0").map_err(io_error)?;
                fs::write(
                    group.join("pids.max"),
                    resources["process_limit"].to_string(),
                )
                .map_err(io_error)?;
            }
            Engine::Docker { .. } | Engine::Podman { .. } => {
                let expected = if matches!(self.engine, Engine::Docker { .. }) {
                    "backends/docker"
                } else {
                    "backends/podman"
                };
                if backend
                    .pointer("/implementation/id")
                    .and_then(Value::as_str)
                    != Some(expected)
                {
                    return Err(ControlError::new("BACKEND_IMPLEMENTATION_MISMATCH"));
                }
                let image = runtime
                    .and_then(|v| v["image"].as_str())
                    .ok_or_else(|| ControlError::new("MISSING_IMAGE"))?;
                let Some((repository, digest)) = image.rsplit_once("@sha256:") else {
                    return Err(ControlError::new("UNPINNED_IMAGE"));
                };
                if repository.is_empty()
                    || repository.starts_with('-')
                    || digest.len() != 64
                    || !digest.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err(ControlError::new("INVALID_IMAGE_DIGEST"));
                }
                // Only exact, already prepared images. No tag resolution or
                // implicit pull inside the execution driver.
                checked(
                    self.engine_command()?.args(["image", "inspect", image]),
                    remaining()?,
                )?;
                self.image = Some(image.to_owned());
            }
        }
        self.mount_workspace(
            "workspace",
            u64_field(resources, "disk_bytes")?,
            remaining()?,
        )?;
        self.resources = Some(resources.clone());
        let probe = self.execute(
            &["/bin/sh".into(), "-c".into(), "true".into()],
            remaining()?,
        )?;
        if probe.exit_code != 0 {
            return Err(ControlError::new("BACKEND_ISOLATION_PROBE_FAILED"));
        }
        Ok(json!({"session_id": self.session_id, "backend": backend["implementation"]}))
    }

    fn freeze(&mut self, timeout_ms: u64) -> Result<()> {
        if self.frozen {
            return Ok(());
        }
        let started = Instant::now();
        let disk = u64_field(
            self.resources
                .as_ref()
                .ok_or_else(|| ControlError::new("BACKEND_NOT_OPEN"))?,
            "disk_bytes",
        )?;
        let candidate = self.mount_workspace("candidate", disk, timeout_ms)?;
        copy_tree(
            &self.root.join("workspace"),
            &candidate,
            started,
            timeout_ms,
        )?;
        let remaining = timeout_ms
            .checked_sub(started.elapsed().as_millis() as u64)
            .filter(|n| *n > 0)
            .ok_or_else(|| ControlError::new("BACKEND_TIMEOUT"))?;
        checked(
            command("/usr/bin/mount")
                .args(["-o", "remount,ro,nosuid,nodev"])
                .arg(&candidate),
            remaining,
        )?;
        self.frozen = true;
        Ok(())
    }

    fn run_harness(&mut self, _: &Value) -> Result<Value> {
        Err(ControlError::new("HARNESS_ADAPTER_UNAVAILABLE"))
    }

    fn close(&mut self) -> Result<()> {
        self.remove_container()?;
        if let Some(group) = &self.cgroup {
            if group.join("cgroup.kill").exists() {
                fs::write(group.join("cgroup.kill"), "1").map_err(io_error)?;
            }
            let started = Instant::now();
            loop {
                match fs::remove_dir(group) {
                    Ok(()) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                    Err(_) if started.elapsed() < Duration::from_secs(2) => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => return Err(ControlError::new("CGROUP_CLEANUP_FAILED")),
                }
            }
            self.cgroup = None;
        }
        while let Some(path) = self.mounted.last() {
            checked(command("/usr/bin/umount").arg(path), 5000)?;
            self.mounted.pop();
        }
        if self.root.exists() {
            fs::remove_dir_all(&self.root).map_err(io_error)?;
        }
        self.resources = None;
        Ok(())
    }
}

impl Drop for LinuxBackend {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn copy_tree(source: &Path, destination: &Path, started: Instant, timeout: u64) -> Result<()> {
    for entry in fs::read_dir(source).map_err(io_error)? {
        if started.elapsed() >= Duration::from_millis(timeout) {
            return Err(ControlError::new("BACKEND_TIMEOUT"));
        }
        let entry = entry.map_err(io_error)?;
        let target = destination.join(entry.file_name());
        let kind = entry.file_type().map_err(io_error)?;
        if kind.is_symlink() {
            symlink(fs::read_link(entry.path()).map_err(io_error)?, target).map_err(io_error)?;
        } else if kind.is_dir() {
            fs::create_dir(&target).map_err(io_error)?;
            copy_tree(&entry.path(), &target, started, timeout)?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
            chown(&target, Some(metadata.uid()), Some(metadata.gid())).map_err(io_error)?;
            fs::set_permissions(&target, metadata.permissions()).map_err(io_error)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target).map_err(io_error)?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
            chown(&target, Some(metadata.uid()), Some(metadata.gid())).map_err(io_error)?;
        } else {
            return Err(ControlError::new("UNSUPPORTED_WORKSPACE_FILE"));
        }
    }
    Ok(())
}
