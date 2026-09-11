use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{ControlError, Result};

const CORE_SCHEMA: &str = include_str!("../../contracts/uenv.schema.json");
include!(concat!(env!("OUT_DIR"), "/schemas.rs"));

/// Reads field names from the generated contract instead of declaring a second
/// Rust wire model. Full JSON Schema validation remains at RPC/package edges;
/// this executable reference adds the control-path semantic checks.
#[derive(Clone, Debug)]
pub struct ContractSchema {
    root: Value,
    extensions: BTreeMap<String, Value>,
}

impl ContractSchema {
    pub fn bundled() -> Self {
        let mut schema = Self {
            root: serde_json::from_str(CORE_SCHEMA).expect("bundled contract must be valid JSON"),
            extensions: BTreeMap::new(),
        };
        for value in BUILTIN_SCHEMAS {
            schema
                .register_extension(serde_json::from_str(value).expect("generated schema JSON"))
                .expect("unique generated schema");
        }
        schema
    }

    pub fn register_extension(&mut self, schema: Value) -> Result<()> {
        if !jsonschema::meta::try_is_valid(&schema).unwrap_or(false) {
            return Err(ControlError::new("INVALID_EXTENSION_SCHEMA"));
        }
        let id = string(&schema, "$id")?.to_owned();
        if id.is_empty() || Some(id.as_str()) == self.root["$id"].as_str() {
            return Err(ControlError::new("INVALID_EXTENSION_SCHEMA_ID"));
        }
        if let Some(existing) = self.extensions.get(&id)
            && existing != &schema
        {
            return Err(ControlError::new("SCHEMA_VERSION_CONFLICT"));
        }
        self.extensions.insert(id, schema);
        Ok(())
    }

    /// Full validation at the external boundary, using only already verified
    /// schemas. Unknown resources never trigger an implicit network download.
    pub fn validate(&self, name: &str, value: &Value) -> Result<()> {
        let mut document = self.root.clone();
        let mut locations =
            BTreeMap::from([(string(&self.root, "$id")?.to_owned(), "#".to_owned())]);
        for (i, id) in self.extensions.keys().enumerate() {
            locations.insert(id.clone(), format!("#/$defs/extension_{i}"));
        }
        fn rewrite(
            value: &mut Value,
            base: &str,
            locations: &BTreeMap<String, String>,
        ) -> Result<()> {
            match value {
                Value::Object(map) => {
                    map.remove("$id");
                    if let Some(Value::String(reference)) = map.get_mut("$ref") {
                        let (uri, fragment) = reference
                            .split_once('#')
                            .unwrap_or((reference.as_str(), ""));
                        let prefix = if uri.is_empty() {
                            base
                        } else {
                            locations
                                .get(uri)
                                .ok_or_else(|| ControlError::new("UNKNOWN_SCHEMA_REFERENCE"))?
                        };
                        *reference = format!("{prefix}{fragment}");
                    }
                    for child in map.values_mut() {
                        rewrite(child, base, locations)?;
                    }
                }
                Value::Array(values) => {
                    for child in values {
                        rewrite(child, base, locations)?;
                    }
                }
                _ => (),
            }
            Ok(())
        }
        rewrite(&mut document, "#", &locations)?;
        for (i, (id, schema)) in self.extensions.iter().enumerate() {
            let mut schema = schema.clone();
            rewrite(&mut schema, &locations[id], &locations)?;
            document["$defs"][format!("extension_{i}")] = schema;
        }
        document["$defs"]["TypedConfig"] = serde_json::json!({"oneOf":self.extensions.keys().map(|id|serde_json::json!({
            "type":"object","properties":{"schema_ref":{"const":id},"data":{"$ref":locations[id]}},
            "required":["schema_ref","data"],"additionalProperties":false
        })).collect::<Vec<_>>()});
        if self.extensions.is_empty() {
            document["$defs"]["TypedConfig"] = Value::Bool(false);
        }
        self.definition(name)?;
        document["$ref"] = Value::String(format!("#/$defs/{name}"));
        let validator = jsonschema::validator_for(&document)
            .map_err(|_| ControlError::new("INVALID_REGISTERED_SCHEMA"))?;
        validator
            .validate(value)
            .map_err(|_| ControlError::new(format!("INVALID_CONTRACT:{name}")))
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
        if name == "ContentPart" {
            let kind = string(value, "kind")?;
            let known = properties["kind"]["enum"]
                .as_array()
                .is_some_and(|values| values.iter().any(|v| v == kind));
            if !known || object.len() != 2 || !object.contains_key(kind) {
                return Err(ControlError::new("INVALID_CONTENT_PART"));
            }
            match kind {
                "text" => {
                    string(value, "text")?;
                }
                "artifact" => self.validate_shape("ArtifactRef", &value["artifact"])?,
                "structured" => {
                    self.validate_shape("TypedConfig", &value["structured"])?;
                    string(&value["structured"], "schema_ref")?;
                    if !value["structured"]["data"].is_object() {
                        return Err(ControlError::new("STRUCTURED_CONTENT_MUST_BE_MODEL"));
                    }
                }
                _ => unreachable!(),
            }
        } else if name == "ToolResult" {
            self.validate_shape("Observation", &value["observation"])?;
            let status = string(value, "status")?;
            if !["ok", "error", "timeout", "cancelled"].contains(&status) {
                return Err(ControlError::new("INVALID_TOOL_RESULT_STATUS"));
            }
            bool_field(value, "output_truncated")?;
        } else if name == "Observation" {
            for part in array(value, "content")? {
                self.validate_shape("ContentPart", part)?;
            }
            bool_field(value, "terminated")?;
            bool_field(value, "episode_truncated")?;
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
