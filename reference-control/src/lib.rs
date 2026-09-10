//! Executable reference for the Rust-owned UEnv control path.
//!
//! Public wire fields still come from `contracts/uenv.schema.json`. Python owns
//! user extensions; this crate owns plan resolution, budgets, routing, scoring
//! completion, cleanup, trajectory sealing, and the attempt result.

pub mod contracts;
pub mod plan;
pub mod ports;
pub mod runtime;
pub mod scoring;
pub mod supervisor;

use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{code}")]
pub struct ControlError {
    pub code: String,
    pub retryable: bool,
    pub phase: Option<&'static str>,
    pub operation_id: Option<String>,
}

impl ControlError {
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            retryable: false,
            phase: None,
            operation_id: None,
        }
    }

    pub fn retryable(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            retryable: true,
            phase: None,
            operation_id: None,
        }
    }

    pub fn in_phase(mut self, phase: &'static str) -> Self {
        if self.phase.is_none() {
            self.phase = Some(phase);
        }
        self
    }

    pub fn with_operation(mut self, operation_id: impl Into<String>) -> Self {
        self.operation_id = Some(operation_id.into());
        self
    }
}

pub type Result<T> = std::result::Result<T, ControlError>;
