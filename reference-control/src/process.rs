//! Linux process supervision. Namespace/cgroup isolation belongs to Backend;
//! a process group alone is not a sandbox.
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{ControlError, Result};

unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
    fn fcntl(fd: i32, command: i32, ...) -> i32;
}

#[derive(Debug)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Caller supplies a trusted executable and argv, never a string-built shell
/// command. Pipes are nonblocking, so inherited descriptors cannot hang reap.
pub fn run(command: &mut Command, timeout_ms: u64, output_limit: usize) -> Result<CommandOutput> {
    if timeout_ms == 0 || output_limit == 0 {
        return Err(ControlError::new("INVALID_PROCESS_LIMIT"));
    }
    let started = Instant::now();
    command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|_| ControlError::new("PROCESS_START_FAILED"))?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut output = CommandOutput {
        exit_code: -1,
        stdout: vec![],
        stderr: vec![],
    };
    let result = (|| {
        for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
            // Linux F_SETFL / O_NONBLOCK; only these newly created pipes.
            if unsafe { fcntl(fd, 4, 2048) } < 0 {
                return Err(ControlError::new("PROCESS_PIPE_FAILED"));
            }
        }
        loop {
            for (reader, bytes) in [
                (&mut stdout as &mut dyn Read, &mut output.stdout),
                (&mut stderr as &mut dyn Read, &mut output.stderr),
            ] {
                for _ in 0..8 {
                    let mut chunk = [0; 8192];
                    match reader.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            bytes.extend_from_slice(&chunk[..n]);
                            if bytes.len() > output_limit {
                                return Err(ControlError::new("PROCESS_OUTPUT_LIMIT"));
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => return Err(ControlError::new("PROCESS_PIPE_FAILED")),
                    }
                }
            }
            if started.elapsed() >= Duration::from_millis(timeout_ms) {
                return Err(ControlError::new("PROCESS_TIMEOUT"));
            }
            if let Some(status) = child
                .try_wait()
                .map_err(|_| ControlError::new("PROCESS_WAIT_FAILED"))?
            {
                output.exit_code = status.code().unwrap_or(-1);
                // Drain already buffered output after exit, without waiting for
                // descendants that illegally retained stdout/stderr.
                for (reader, bytes) in [
                    (&mut stdout as &mut dyn Read, &mut output.stdout),
                    (&mut stderr as &mut dyn Read, &mut output.stderr),
                ] {
                    loop {
                        let mut chunk = [0; 8192];
                        match reader.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(n) => {
                                bytes.extend_from_slice(&chunk[..n]);
                                if bytes.len() > output_limit {
                                    return Err(ControlError::new("PROCESS_OUTPUT_LIMIT"));
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => return Err(ControlError::new("PROCESS_PIPE_FAILED")),
                        }
                    }
                }
                return Ok(output);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    })();
    // Also remove ordinary background descendants after a successful command.
    unsafe {
        kill(-(child.id() as i32), 9);
    }
    let _ = child.kill();
    let _ = child.wait();
    result
}
