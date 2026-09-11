//! Durable internal storage. Authorization remains at Server/Hub/Worker ports.
use crate::contracts::{canonical_bytes, digest_bytes, string, u64_field};
use crate::ports::FileStore;
use crate::{ControlError, Result};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
fn io_error(_: std::io::Error) -> ControlError {
    ControlError::new("STORAGE_IO_FAILED")
}

pub fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(io_error)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Install a complete immutable object; a conflicting existing object is an
/// error. Temporary files never become a reader-visible partial result.
pub fn write_immutable(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        return if fs::read(path).map_err(io_error)? == bytes {
            Ok(())
        } else {
            Err(ControlError::new("IMMUTABLE_CONTENT_CONFLICT"))
        };
    }
    let parent = path
        .parent()
        .ok_or_else(|| ControlError::new("INVALID_STORAGE_PATH"))?;
    fs::create_dir_all(parent).map_err(io_error)?;
    let temporary = parent.join(format!(
        ".pending-{}-{}",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(io_error)?;
    let result = (|| {
        file.write_all(bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        // hard_link is an atomic no-replace publish on both supported platforms.
        match fs::hard_link(&temporary, path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if fs::read(path).map_err(io_error)? != bytes {
                    return Err(ControlError::new("IMMUTABLE_CONTENT_CONFLICT"));
                }
            }
            Err(e) => return Err(io_error(e)),
        }
        sync_directory(parent)
    })();
    drop(file);
    let cleanup = fs::remove_file(&temporary).map_err(io_error);
    result.and(cleanup)
}

pub struct LocalFileStore {
    root: PathBuf,
}
impl LocalFileStore {
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root).map_err(io_error)?;
        Ok(Self {
            root: root.canonicalize().map_err(io_error)?,
        })
    }

    fn object_path(&self, digest: &str) -> Result<PathBuf> {
        let hex = digest
            .strip_prefix("sha256:")
            .filter(|v| {
                v.len() == 64
                    && v.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            .ok_or_else(|| ControlError::new("INVALID_ARTIFACT_DIGEST"))?;
        Ok(self.root.join("objects").join(&hex[..2]).join(hex))
    }
}

impl FileStore for LocalFileStore {
    fn put_bytes(&mut self, content: &[u8], media_type: &str) -> Result<Value> {
        let digest = digest_bytes(content);
        write_immutable(&self.object_path(&digest)?, content)?;
        Ok(
            json!({"uri":format!("uenv-file://{digest}"),"digest":digest,
                  "size_bytes":content.len(),"media_type":media_type}),
        )
    }
    fn read(&self, reference: &Value) -> Result<Vec<u8>> {
        let digest = string(reference, "digest")?;
        if string(reference, "uri")? != format!("uenv-file://{digest}") {
            return Err(ControlError::new("ARTIFACT_LOCATION_MISMATCH"));
        }
        let content = fs::read(self.object_path(digest)?).map_err(io_error)?;
        if digest_bytes(&content) != digest
            || content.len() as u64 != u64_field(reference, "size_bytes")?
        {
            return Err(ControlError::new("ARTIFACT_INTEGRITY_MISMATCH"));
        }
        Ok(content)
    }
    fn spool_root(&self) -> Option<PathBuf> {
        Some(self.root.join("spool"))
    }
}

/// One durable journal per attempt. Failed reservations are not re-used;
/// payloads are immutable and recovery preserves the original sequence gaps.
pub struct EventSpool {
    root: PathBuf,
    identity: Value,
}
impl EventSpool {
    pub fn open(root: &Path, plan: &Value) -> Result<Self> {
        let identity = canonical_bytes(&json!([
            plan["run_id"],
            plan["episode_id"],
            plan["attempt_id"]
        ]))?;
        let directory = root.join(digest_bytes(&identity).replace(':', "-"));
        fs::create_dir_all(&directory).map_err(io_error)?;
        write_immutable(&directory.join("identity.json"), &identity)?;
        write_immutable(
            &directory.join("plan.json"),
            &canonical_bytes(
                &json!({"plan_digest":plan["plan_digest"],"task_id":plan["task"]["task_id"]}),
            )?,
        )?;
        Ok(Self {
            root: directory,
            identity: json!({"run_id":plan["run_id"],"episode_id":plan["episode_id"],"attempt_id":plan["attempt_id"],"task_id":plan["task"]["task_id"]}),
        })
    }
    pub fn reserve(&mut self, sequence: u64) -> Result<()> {
        let path = self.root.join(format!("{sequence:020}.reserved"));
        let f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(io_error)?;
        f.sync_all().map_err(io_error)?;
        sync_directory(&self.root)
    }
    pub fn append(&mut self, event: &Value) -> Result<()> {
        self.validate_event(event, u64_field(event, "sequence")?)?;
        write_immutable(
            &self
                .root
                .join(format!("{:020}.json", u64_field(event, "sequence")?)),
            &canonical_bytes(event)?,
        )
    }
    fn validate_event(&self, event: &Value, sequence: u64) -> Result<()> {
        if u64_field(event, "sequence")? != sequence
            || ["run_id", "episode_id", "attempt_id", "task_id"]
                .iter()
                .any(|k| event[*k] != self.identity[*k])
            || !self.root.join(format!("{sequence:020}.reserved")).is_file()
        {
            return Err(ControlError::new("INVALID_SPOOL_EVENT"));
        }
        Ok(())
    }
    pub fn recover(&self) -> Result<(Vec<Value>, u64)> {
        let mut events = Vec::new();
        let mut next = 0;
        for entry in fs::read_dir(&self.root).map_err(io_error)? {
            let path = entry.map_err(io_error)?.path();
            if let Some(stem) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<u64>().ok())
            {
                next = next.max(
                    stem.checked_add(1)
                        .ok_or_else(|| ControlError::new("SEQUENCE_OVERFLOW"))?,
                );
                if path.extension().is_some_and(|e| e == "json") {
                    let event = serde_json::from_slice::<Value>(&fs::read(path).map_err(io_error)?)
                        .map_err(|_| ControlError::new("INVALID_SPOOL_EVENT"))?;
                    self.validate_event(&event, stem)?;
                    events.push(event);
                }
            }
        }
        events.sort_by_key(|v| v["sequence"].as_u64());
        Ok((events, next))
    }

    /// Internal reservation record, not a second public trajectory format.
    /// If an operation has no completion record after restart, retain its
    /// conservative reservation instead of resetting the episode's budget.
    pub fn reserve_usage(&self, usage: &Value) -> Result<()> {
        let index = fs::read_dir(&self.root)
            .map_err(io_error)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|s| s == "usage"))
            .count();
        write_immutable(
            &self.root.join(format!("{index:020}.usage")),
            &canonical_bytes(usage)?,
        )
    }
    pub fn recovered_usage(&self, initial: &Value) -> Result<Value> {
        let mut usage = initial.clone();
        let mut records = fs::read_dir(&self.root)
            .map_err(io_error)?
            .map(|e| e.map(|e| e.path()).map_err(io_error))
            .collect::<Result<Vec<_>>>()?;
        records.sort();
        if let Some(path) = records
            .iter()
            .rev()
            .find(|p| p.extension().is_some_and(|s| s == "usage"))
        {
            let value: Value = serde_json::from_slice(&fs::read(path).map_err(io_error)?)
                .map_err(|_| ControlError::new("INVALID_SPOOL_USAGE"))?;
            for key in ["generation_count", "tool_call_count", "output_token_count"] {
                usage[key] = json!(u64_field(&usage, key)?.max(u64_field(&value, key)?));
            }
        }
        Ok(usage)
    }
}
