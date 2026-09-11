//! Attempt-local duplex RPC. The Worker owns child lifetime and deadlines.
//! No public listener, credentials, Python executors or shell commands are
//! exposed to the Agent. Nested callbacks use the same inherited pipes.
use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use crate::ports::{
    AgentHost, Cancellation, EnvironmentHost, FileStore, ModelProvider, ScorerHost, ScoringContext,
    ToolHost,
};
use crate::runtime::AgentRuntime;
use crate::{ControlError, Result};
use serde_json::{Value, json};

const MAX_FRAME: usize = 8 * 1024 * 1024;

pub struct RpcProcess {
    child: RefCell<Child>,
    outgoing: SyncSender<Vec<u8>>,
    incoming: Receiver<Result<Value>>,
    next_id: Cell<u64>,
    closed: Cell<bool>,
    cancellation: RpcCancellation,
}

#[derive(Clone, Default)]
pub struct RpcCancellation(Arc<AtomicBool>);
impl RpcCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
impl Cancellation for RpcCancellation {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

impl RpcProcess {
    pub fn spawn(command: &mut Command) -> Result<Rc<Self>> {
        // Inherit only launch prerequisites. Worker credentials, cloud tokens
        // and database settings must not leak into extension processes.
        // Role-specific variables must be supplied explicitly by the launcher.
        let explicit: Vec<_> = command
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(std::ffi::OsStr::to_os_string)))
            .collect();
        command.env_clear();
        for name in [
            "PATH",
            "SystemRoot",
            "WINDIR",
            "TEMP",
            "TMP",
            "LANG",
            "LC_ALL",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        for (name, value) in explicit {
            if let Some(value) = value {
                command.env(name, value);
            } else {
                command.env_remove(name);
            }
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command
            .spawn()
            .map_err(|_| ControlError::new("RPC_PROCESS_START_FAILED"))?;
        let mut input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (tx, incoming) = mpsc::sync_channel(64);
        let write_errors = tx.clone();
        let (outgoing, writes) = mpsc::sync_channel::<Vec<u8>>(8);
        std::thread::spawn(move || {
            while let Ok(bytes) = writes.recv() {
                if input.write_all(&bytes).and_then(|_| input.flush()).is_err() {
                    let _ = write_errors.send(Err(ControlError::new("RPC_WRITE_FAILED")));
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let mut bytes = Vec::new();
                let read = reader
                    .by_ref()
                    .take((MAX_FRAME + 1) as u64)
                    .read_until(b'\n', &mut bytes);
                let value = match read {
                    Ok(0) => Err(ControlError::new("RPC_CLOSED")),
                    Ok(_) if bytes.len() > MAX_FRAME || !bytes.ends_with(b"\n") => {
                        Err(ControlError::new("RPC_FRAME_LIMIT"))
                    }
                    Ok(_) => serde_json::from_slice::<Value>(&bytes)
                        .map_err(|_| ControlError::new("RPC_INVALID_FRAME")),
                    Err(_) => Err(ControlError::new("RPC_READ_FAILED")),
                };
                let failed = value.is_err();
                if tx.send(value).is_err() || failed {
                    break;
                }
            }
        });
        Ok(Rc::new(Self {
            child: RefCell::new(child),
            outgoing,
            incoming,
            next_id: Cell::new(0),
            closed: Cell::new(false),
            cancellation: RpcCancellation::default(),
        }))
    }

    pub fn cancellation(&self) -> RpcCancellation {
        self.cancellation.clone()
    }

    fn send(&self, value: Value) -> Result<()> {
        if self.closed.get() {
            return Err(ControlError::new("RPC_CLOSED"));
        }
        let mut bytes =
            serde_json::to_vec(&value).map_err(|_| ControlError::new("RPC_INVALID_FRAME"))?;
        bytes.push(b'\n');
        if bytes.len() > MAX_FRAME {
            return Err(ControlError::new("RPC_FRAME_LIMIT"));
        }
        self.outgoing
            .try_send(bytes)
            .map_err(|_| ControlError::new("RPC_WRITE_QUEUE_FULL"))
    }

    pub fn call(&self, method: &str, params: Value, timeout_ms: u64) -> Result<Value> {
        self.call_with(method, params, timeout_ms, &mut |method, _| {
            if method == "__poll" {
                Ok(Value::Null)
            } else {
                Err(ControlError::new("RPC_METHOD_NOT_ALLOWED"))
            }
        })
    }

    pub fn call_with(
        &self,
        method: &str,
        params: Value,
        timeout_ms: u64,
        callback: &mut dyn FnMut(&str, &Value) -> Result<Value>,
    ) -> Result<Value> {
        if timeout_ms == 0 {
            return Err(ControlError::new("RPC_TIMEOUT"));
        }
        if self.cancellation.is_cancelled() {
            self.close();
            return Err(ControlError::new("EPISODE_CANCELLED"));
        }
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(timeout_ms))
            .ok_or_else(|| ControlError::new("RPC_INVALID_TIMEOUT"))?;
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        let id = format!("rust:{id}");
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        let result = (|| {
            loop {
                if self.cancellation.is_cancelled() {
                    return Err(ControlError::new("EPISODE_CANCELLED"));
                }
                if Instant::now() >= deadline {
                    return Err(ControlError::new("RPC_TIMEOUT"));
                }
                let frame = match self.incoming.recv_timeout(Duration::from_millis(20)) {
                    Ok(frame) => frame?,
                    Err(RecvTimeoutError::Timeout) => {
                        callback("__poll", &Value::Null)?;
                        continue;
                    }
                    Err(_) => return Err(ControlError::new("RPC_CLOSED")),
                };
                if frame["jsonrpc"] != "2.0" || !frame["id"].is_string() {
                    return Err(ControlError::new("RPC_INVALID_FRAME"));
                }
                if let Some(method) = frame["method"].as_str() {
                    let response = match callback(method, &frame["params"]) {
                        Ok(value) => json!({"jsonrpc":"2.0","id":frame["id"],"result":value}),
                        Err(error) => {
                            json!({"jsonrpc":"2.0","id":frame["id"],"error":{"code":-32000,"message":"Worker rejected operation","data":{"code":error.code}}})
                        }
                    };
                    self.send(response)?;
                } else {
                    if frame["id"] != id {
                        return Err(ControlError::new("RPC_RESPONSE_ID_MISMATCH"));
                    }
                    if let Some(code) = frame.pointer("/error/data/code").and_then(Value::as_str) {
                        return Err(ControlError::new(code));
                    }
                    return frame
                        .get("result")
                        .cloned()
                        .ok_or_else(|| ControlError::new("RPC_INVALID_FRAME"));
                }
            }
        })();
        if let Err(error) = &result
            && (error.code.starts_with("RPC_") || error.code == "EPISODE_CANCELLED")
        {
            self.close();
        }
        result
    }

    pub fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        let mut child = self.child.borrow_mut();
        #[cfg(unix)]
        {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            unsafe {
                kill(-(child.id() as i32), 9);
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}
impl Drop for RpcProcess {
    fn drop(&mut self) {
        self.close();
    }
}

pub struct RpcToolHost(pub Rc<RpcProcess>);
impl ToolHost for RpcToolHost {
    fn prepare(&mut self, tools: &[Value], session: &Value, ms: u64) -> Result<Vec<Value>> {
        let value = self.0.call(
            "tools.prepare",
            json!({"tools":tools,"session":session}),
            ms,
        )?;
        value
            .as_array()
            .cloned()
            .ok_or_else(|| ControlError::new("INVALID_TOOL_TABLE"))
    }
    fn validate_call(&self, binding: &Value, call: &Value) -> Result<()> {
        self.0.call(
            "tools.validate",
            json!({"binding":binding,"call":call}),
            call["timeout_ms"].as_u64().unwrap_or(1),
        )?;
        Ok(())
    }
    fn call_tool(&mut self, binding: &Value, call: &Value) -> Result<Value> {
        self.0.call(
            "tools.execute",
            json!({"binding":binding,"call":call}),
            call["timeout_ms"].as_u64().unwrap_or(1),
        )
    }
    fn freeze(&mut self, ms: u64) -> Result<()> {
        self.0.call("tools.freeze", json!({}), ms)?;
        Ok(())
    }
    fn close(&mut self) -> Result<()> {
        Ok(())
    } // shared process closed by its owner
}

pub struct RpcModelProvider(pub Rc<RpcProcess>);
impl ModelProvider for RpcModelProvider {
    fn generate(&mut self, request: &Value) -> Result<Value> {
        self.0.call(
            "model.generate",
            request.clone(),
            request["remaining_timeout_ms"].as_u64().unwrap_or(1),
        )
    }
}

pub struct RpcAgentHost(pub Rc<RpcProcess>);
impl AgentHost for RpcAgentHost {
    fn prepare(
        &mut self,
        agent: &Value,
        tools: &[Value],
        model_id: &str,
        ms: u64,
    ) -> Result<Vec<Value>> {
        let value = self.0.call(
            "agent.prepare",
            json!({"agent":agent,"tools":tools,"model_id":model_id}),
            ms,
        )?;
        value
            .as_array()
            .cloned()
            .ok_or_else(|| ControlError::new("INVALID_TOOL_TABLE"))
    }
    fn run_agent(
        &mut self,
        task: &Value,
        observation: &Value,
        runtime: &mut AgentRuntime<'_>,
        ms: u64,
    ) -> Result<Value> {
        self.0.call_with(
            "agent.run",
            json!({"task":task,"observation":observation,"seed":runtime.seed()}),
            ms,
            &mut |method, params| match method {
                "generate" => runtime.generate(params),
                "step" => runtime.step(params),
                "__poll" => {
                    runtime.check_rpc_active()?;
                    Ok(Value::Null)
                }
                _ => Err(ControlError::new("RPC_METHOD_NOT_ALLOWED")),
            },
        )
    }
    fn close(&mut self) -> Result<()> {
        let result = self.0.call("host.close", json!({}), 5000).map(|_| ());
        self.0.close();
        result
    }
}

pub struct RpcEnvironmentHost {
    pub process: Rc<RpcProcess>,
    pub files: Rc<RefCell<dyn FileStore>>,
}
impl EnvironmentHost for RpcEnvironmentHost {
    fn prepare(
        &mut self,
        dataset_package: &Value,
        environment: &Value,
        session: &Value,
        ms: u64,
    ) -> Result<()> {
        self.process.call(
            "environment.prepare",
            json!({"dataset_package":dataset_package,
            "environment":environment,"session":session,"remaining_timeout_ms":ms}),
            ms,
        )?;
        Ok(())
    }
    fn reset(&mut self, task: &Value, seed: u64, ms: u64) -> Result<Value> {
        self.process.call_with(
            "environment.reset",
            json!({"task":task,"seed":seed,"remaining_timeout_ms":ms}),
            ms,
            &mut |method, params| match method {
                "__poll" => Ok(Value::Null),
                "file.write_artifact" => {
                    let bytes: Vec<u8> = serde_json::from_value(params["content"].clone())
                        .map_err(|_| ControlError::new("INVALID_ARTIFACT_BYTES"))?;
                    self.files
                        .borrow_mut()
                        .put_bytes(&bytes, crate::contracts::string(params, "media_type")?)
                }
                _ => Err(ControlError::new("RPC_METHOD_NOT_ALLOWED")),
            },
        )
    }
    fn state_snapshot(&mut self, ms: u64) -> Result<Option<Value>> {
        let value = self.process.call(
            "environment.state_snapshot",
            json!({"remaining_timeout_ms":ms}),
            ms,
        )?;
        Ok((!value.is_null()).then_some(value))
    }
    fn close(&mut self) -> Result<()> {
        let result = self.process.call("host.close", json!({}), 5000).map(|_| ());
        self.process.close();
        result
    }
}

pub struct RpcScorerHost(pub Rc<RpcProcess>);
impl ScorerHost for RpcScorerHost {
    fn score(
        &mut self,
        dataset_package: &Value,
        config: &Value,
        request: &Value,
        context: &mut dyn ScoringContext,
    ) -> Result<Value> {
        let ms = context.remaining_timeout_ms()?;
        self.0.call_with(
            "scorer.score",
            json!({"dataset_package":dataset_package,"config":config,
            "request":request,"remaining_timeout_ms":ms}),
            ms,
            &mut |method, params| {
                context.remaining_timeout_ms()?;
                match method {
                    "__poll" => Ok(Value::Null),
                    "scoring.run_harness" => context.run_harness(params),
                    "scoring.read_artifact" => Ok(json!(context.read_artifact(params)?)),
                    _ => Err(ControlError::new("RPC_METHOD_NOT_ALLOWED")),
                }
            },
        )
    }
    fn close(&mut self) -> Result<()> {
        let result = self.0.call("host.close", json!({}), 5000).map(|_| ());
        self.0.close();
        result
    }
}

/// Routing consumes the single resolved execution_scope. Unsupported routes
/// fail preparation; a sandbox binding can never fall back to the Agent host.
pub struct RpcToolRouter {
    hosts: std::collections::BTreeMap<String, Box<dyn ToolHost>>,
    prepared: Vec<String>,
}
impl RpcToolRouter {
    pub fn new(hosts: std::collections::BTreeMap<String, Box<dyn ToolHost>>) -> Self {
        Self {
            hosts,
            prepared: Vec::new(),
        }
    }
    fn host(&self, binding: &Value) -> Result<&dyn ToolHost> {
        self.hosts
            .get(crate::contracts::string(binding, "execution_scope")?)
            .map(|v| v.as_ref())
            .ok_or_else(|| ControlError::new("TOOL_SCOPE_UNAVAILABLE"))
    }
}
impl ToolHost for RpcToolRouter {
    fn prepare(&mut self, tools: &[Value], session: &Value, ms: u64) -> Result<Vec<Value>> {
        let start = Instant::now();
        let mut actual = Vec::new();
        for binding in tools {
            self.host(binding)?;
        }
        for (scope, host) in &mut self.hosts {
            let selected = tools
                .iter()
                .filter(|v| v["execution_scope"] == *scope)
                .cloned()
                .collect::<Vec<_>>();
            let remaining = ms
                .checked_sub(start.elapsed().as_millis() as u64)
                .filter(|v| *v > 0)
                .ok_or_else(|| ControlError::new("TOOL_PREPARE_TIMEOUT"))?;
            self.prepared.push(scope.clone());
            actual.extend(host.prepare(&selected, session, remaining)?);
        }
        Ok(actual)
    }
    fn validate_call(&self, binding: &Value, call: &Value) -> Result<()> {
        self.host(binding)?.validate_call(binding, call)
    }
    fn call_tool(&mut self, binding: &Value, call: &Value) -> Result<Value> {
        self.hosts
            .get_mut(crate::contracts::string(binding, "execution_scope")?)
            .ok_or_else(|| ControlError::new("TOOL_SCOPE_UNAVAILABLE"))?
            .call_tool(binding, call)
    }
    fn freeze(&mut self, ms: u64) -> Result<()> {
        let start = Instant::now();
        for scope in &self.prepared {
            let remaining = ms
                .checked_sub(start.elapsed().as_millis() as u64)
                .filter(|v| *v > 0)
                .ok_or_else(|| ControlError::new("TOOL_FREEZE_TIMEOUT"))?;
            self.hosts.get_mut(scope).unwrap().freeze(remaining)?;
        }
        Ok(())
    }
    fn close(&mut self) -> Result<()> {
        let mut failure = None;
        for scope in &self.prepared {
            if let Err(e) = self.hosts.get_mut(scope).unwrap().close() {
                failure.get_or_insert(e);
            }
        }
        match failure {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
