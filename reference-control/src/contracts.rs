use std::collections::BTreeSet;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{ControlError, Result};

const CORE_SCHEMA: &str = include_str!("../../contracts/uenv.schema.json");

/// Reads field names from the generated contract instead of declaring a second
/// Rust wire model. Full JSON Schema validation remains at RPC/package edges;
/// this executable reference adds the control-path semantic checks.
#[derive(Clone, Debug)]
pub struct ContractSchema {
    root: Value,
}

impl ContractSchema {
    pub fn bundled() -> Self {
        Self {
            root: serde_json::from_str(CORE_SCHEMA).expect("bundled contract must be valid JSON"),
        }
    }

    pub fn definition(&self, name: &str) -> Result<&Value> {
        self.root
            .get("$defs")
            .and_then(|v| v.get(name))
            .ok_or_else(|| ControlError::new(format!("UNKNOWN_CONTRACT:{name}")))
    }

    /// Enforces the contract's required and allowed top-level fields. Nested
    /// values are checked by their owning boundary or full JSON Schema validator.
    pub fn validate_shape(&self, name: &str, value: &Value) -> Result<()> {
        let definition = self.definition(name)?;
        let object = value
            .as_object()
            .ok_or_else(|| ControlError::new(format!("{name}_MUST_BE_OBJECT")))?;
        let properties = definition
            .get("properties")
            .and_then(Value::as_object)
            .ok_or_else(|| ControlError::new(format!("{name}_HAS_NO_PROPERTIES")))?;
        let required: BTreeSet<&str> = definition
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();

        for field in &required {
            if !object.contains_key(*field) {
                return Err(ControlError::new(format!("MISSING_FIELD:{name}.{field}")));
            }
        }
        for field in object.keys() {
            if !properties.contains_key(field) {
                return Err(ControlError::new(format!("UNKNOWN_FIELD:{name}.{field}")));
            }
        }
        Ok(())
    }

    pub fn fields(&self, name: &str) -> Result<BTreeSet<String>> {
        Ok(self
            .definition(name)?
            .get("properties")
            .and_then(Value::as_object)
            .ok_or_else(|| ControlError::new(format!("{name}_HAS_NO_PROPERTIES")))?
            .keys()
            .cloned()
            .collect())
    }
}

pub fn canonical_bytes(value: &Value) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| ControlError::new("CANONICAL_JSON_FAILED"))
}

pub fn digest(value: &Value) -> Result<String> {
    let bytes = canonical_bytes(value)?;
    Ok(digest_bytes(&bytes))
}

pub fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub fn object<'a>(value: &'a Value, name: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| ControlError::new(format!("{name}_MUST_BE_OBJECT")))
}

pub fn array<'a>(value: &'a Value, field: &str) -> Result<&'a Vec<Value>> {
    value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| ControlError::new(format!("INVALID_FIELD:{field}")))
}

pub fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ControlError::new(format!("INVALID_FIELD:{field}")))
}

pub fn u64_field(value: &Value, field: &str) -> Result<u64> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| ControlError::new(format!("INVALID_FIELD:{field}")))
}

pub fn bool_field(value: &Value, field: &str) -> Result<bool> {
    value
        .get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| ControlError::new(format!("INVALID_FIELD:{field}")))
}
