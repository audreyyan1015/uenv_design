use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::contracts::{ContractSchema, array, digest, object, string, u64_field};
use crate::{ControlError, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeKind {
    Process,
    Container,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentMetadata {
    pub component: Value,
    /// Roles exported by this installed component, for example `environment`
    /// and `scorer` for one dataset package.
    pub roles: BTreeSet<String>,
    pub role_config_schemas: BTreeMap<String, String>,
    pub required_capabilities: BTreeSet<String>,
    pub task_schemas: BTreeSet<String>,
    pub private_schemas: BTreeSet<String>,
    pub native_profiles: BTreeSet<String>,
    /// Tool execution metadata, independent of Agent transport (SDK or MCP).
    pub tool: Option<ToolMetadata>,
    /// Dataset-package runtime copied into the component catalog at install time.
    /// It is metadata for the selected Environment, not another component selector.
    pub default_runtime: Option<Value>,
    pub requires_internet_access: bool,
    pub runtime_kind: Option<RuntimeKind>,
    pub supports_internet_access: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolMetadata {
    /// Derived from the owning Agent package; no separately published adapter.
    pub native_agent: Option<Value>,
    pub execution_scope: String,
    pub required_capabilities: BTreeSet<String>,
}

#[derive(Clone, Debug)]
pub struct AgentToolProfile {
    pub agent: Value,
    pub required_names: BTreeSet<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ComponentCatalog {
    entries: BTreeMap<(String, String), ComponentMetadata>,
}

impl ComponentCatalog {
    pub fn insert(&mut self, metadata: ComponentMetadata) -> Result<()> {
        if !metadata
            .role_config_schemas
            .keys()
            .all(|role| metadata.roles.contains(role))
            || !metadata
                .roles
                .iter()
                .all(|role| metadata.role_config_schemas.contains_key(role))
        {
            return Err(ControlError::new("ROLE_CONFIG_SCHEMA_MISMATCH"));
        }
        if metadata.roles.contains("tool") != metadata.tool.is_some() {
            return Err(ControlError::new("TOOL_METADATA_MISMATCH"));
        }
        let key = component_key(&metadata.component)?;
        if let Some(existing) = self.entries.get(&key) {
            if existing == &metadata {
                return Ok(());
            }
            return Err(ControlError::new("CONFLICTING_COMPONENT_METADATA"));
        }
        self.entries.insert(key, metadata);
        Ok(())
    }

    pub fn get(&self, reference: &Value) -> Result<&ComponentMetadata> {
        let metadata = self
            .entries
            .get(&component_key(reference)?)
            .ok_or_else(|| ControlError::new("COMPONENT_NOT_FOUND"))?;
        if reference
            .get("digest")
            .is_some_and(|digest| Some(digest) != metadata.component.get("digest"))
        {
            return Err(ControlError::new("COMPONENT_DIGEST_MISMATCH"));
        }
        Ok(metadata)
    }
}

/// Validate the shared submission before resolving each member independently.
/// `stored_run` is a read-only comparison value for this run_id, never a fallback
/// configuration. Production must repeat this check in the persistence transaction.
pub fn validate_batch_submission(
    batch: &Value,
    stored_run: Option<&Value>,
    schema: &ContractSchema,
) -> Result<()> {
    schema.validate_shape("BatchRequest", batch)?;
    let run = &batch["run_spec"];
    schema.validate_shape("RunSpec", run)?;
    validate_run_purpose(run)?;
    let run_id = string(run, "run_id")?;
    if let Some(stored) = stored_run {
        if string(stored, "run_id")? != run_id {
            return Err(ControlError::new("RUN_ID_MISMATCH"));
        }
        if stored != run {
            return Err(ControlError::new("RUN_CONFIG_CONFLICT"));
        }
    }
    let batch_id = string(batch, "batch_id")?;
    let episodes = array(batch, "episodes")?;
    if episodes.is_empty() {
        return Err(ControlError::new("EMPTY_BATCH"));
    }
    let mut request_ids = BTreeSet::new();
    let mut episode_ids = BTreeSet::new();
    for (index, episode) in episodes.iter().enumerate() {
        schema.validate_shape("EpisodeRequest", episode)?;
        if string(episode, "batch_id")? != batch_id {
            return Err(ControlError::new("BATCH_ID_MISMATCH"));
        }
        if episode["sample_index"].as_u64() != Some(index as u64) {
            return Err(ControlError::new("SAMPLE_INDEX_MISMATCH"));
        }
        if !request_ids.insert(string(episode, "request_id")?)
            || !episode_ids.insert(string(episode, "episode_id")?)
        {
            return Err(ControlError::new("DUPLICATE_BATCH_MEMBER"));
        }
    }
    Ok(())
}

pub struct PlanResolver<'a> {
    pub schema: &'a ContractSchema,
    pub catalog: &'a ComponentCatalog,
    pub agent_profile: &'a AgentToolProfile,
}

impl PlanResolver<'_> {
    /// Resolve one validated batch member using the same BatchRequest.run_spec.
    /// This is an internal calculation, not a separate configuration RPC.
    pub fn resolve(
        &self,
        episode: &Value,
        run: &Value,
        accepted_at_ms: u64,
        image_resolver: &dyn Fn(&Value, &Value, &Value) -> Result<Value>,
    ) -> Result<Value> {
        self.schema.validate_shape("EpisodeRequest", episode)?;
        self.schema.validate_shape("RunSpec", run)?;
        validate_run_purpose(run)?;

        let task = episode
            .get("task")
            .ok_or_else(|| ControlError::new("MISSING_FIELD:EpisodeRequest.task"))?;
        let task_schema = task
            .pointer("/input/schema_ref")
            .and_then(Value::as_str)
            .ok_or_else(|| ControlError::new("INVALID_TASK_SCHEMA"))?;
        let private_schema = episode
            .get("private_data")
            .filter(|_| run["scoring"]["enabled"] == true)
            .map(|private_data| {
                private_data
                    .get("schema_ref")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ControlError::new("INVALID_PRIVATE_SCHEMA"))
            })
            .transpose()?;

        let mut selected = BTreeMap::<String, Value>::new();
        let mut required = BTreeSet::<String>::new();

        let mut plan = Map::new();
        plan.insert("run_id".to_owned(), run["run_id"].clone());
        for field in ["episode_id", "task", "seed"] {
            plan.insert(
                field.to_owned(),
                episode.get(field).cloned().ok_or_else(|| {
                    ControlError::new(format!("MISSING_FIELD:EpisodeRequest.{field}"))
                })?,
            );
        }
        if let Some(value) = episode
            .get("private_data")
            .filter(|_| run["scoring"]["enabled"] == true)
        {
            plan.insert("private_data".to_owned(), value.clone());
        }
        for field in [
            "purpose",
            "model",
            "limits",
            "training",
            "environment",
            "scoring",
        ] {
            if let Some(value) = run.get(field) {
                plan.insert(field.to_owned(), value.clone());
            }
        }

        // The harness is selected only at private_data.data.evaluation_plan.harness.
        // Resolve that existing field in place so it receives the same digest,
        // version and capability checks as every other executable component.
        if let Some(requested_harness) = plan
            .get("private_data")
            .and_then(|value| value.pointer("/data/evaluation_plan/harness"))
            .cloned()
        {
            let resolved_harness =
                self.resolve_role(&requested_harness, "harness", &mut selected, &mut required)?;
            plan.get_mut("private_data")
                .and_then(|value| value.pointer_mut("/data/evaluation_plan/harness"))
                .ok_or_else(|| ControlError::new("HARNESS_SELECTION_MISSING"))?
                .clone_from(&resolved_harness);
        }

        let package = run
            .get("dataset_package")
            .ok_or_else(|| ControlError::new("MISSING_DATASET_PACKAGE"))?;
        let resolved_package =
            self.resolve_role(package, "environment", &mut selected, &mut required)?;
        let metadata = self.catalog.get(package)?;
        if metadata
            .role_config_schemas
            .get("environment")
            .map(String::as_str)
            != run
                .pointer("/environment/schema_ref")
                .and_then(Value::as_str)
        {
            return Err(ControlError::new("COMPONENT_CONFIG_SCHEMA_MISMATCH"));
        }
        if run["scoring"]["enabled"] == true {
            self.resolve_role(package, "scorer", &mut selected, &mut required)?;
            if metadata
                .role_config_schemas
                .get("scorer")
                .map(String::as_str)
                != run
                    .pointer("/scoring/config/schema_ref")
                    .and_then(Value::as_str)
            {
                return Err(ControlError::new("COMPONENT_CONFIG_SCHEMA_MISMATCH"));
            }
        }
        plan.insert("dataset_package".to_owned(), resolved_package);
        for role in ["agent", "backend"] {
            let spec = run
                .get(role)
                .ok_or_else(|| ControlError::new(format!("MISSING_FIELD:RunSpec.{role}")))?;
            let mut resolved_spec = object(spec, role)?.clone();
            let requested = spec
                .get("implementation")
                .ok_or_else(|| ControlError::new("MISSING_COMPONENT_IMPLEMENTATION"))?;
            let config_schema = spec
                .pointer("/config/schema_ref")
                .and_then(Value::as_str)
                .ok_or_else(|| ControlError::new("INVALID_COMPONENT_CONFIG"))?;
            let metadata = self.catalog.get(requested)?;
            if metadata.role_config_schemas.get(role).map(String::as_str) != Some(config_schema) {
                return Err(ControlError::new("COMPONENT_CONFIG_SCHEMA_MISMATCH"));
            }
            let resolved = self.resolve_role(requested, role, &mut selected, &mut required)?;
            resolved_spec.insert("implementation".to_owned(), resolved);
            plan.insert(role.to_owned(), Value::Object(resolved_spec));
        }

        let environment_metadata = self.catalog.get(&plan["dataset_package"])?;
        if !environment_metadata.task_schemas.contains(task_schema) {
            return Err(ControlError::new("COMPONENT_TASK_SCHEMA_MISMATCH"));
        }
        if let Some(private_schema) = private_schema
            && !environment_metadata
                .private_schemas
                .contains(private_schema)
        {
            return Err(ControlError::new("SCORER_PRIVATE_SCHEMA_MISMATCH"));
        }
        let internet_access = environment_metadata.requires_internet_access;
        if internet_access {
            required.insert("internet_access.v1".to_owned());
        }

        let resolved_agent = plan
            .get("agent")
            .and_then(|v| v.get("implementation"))
            .ok_or_else(|| ControlError::new("MISSING_RESOLVED_AGENT"))?;
        let resolved_tools = self.resolve_tools(
            resolved_agent,
            array(run, "tools")?,
            &mut selected,
            &mut required,
        )?;
        plan.insert("tools".to_owned(), Value::Array(resolved_tools));

        let backend = plan
            .get("backend")
            .and_then(|v| v.get("implementation"))
            .ok_or_else(|| ControlError::new("MISSING_RESOLVED_BACKEND"))?;
        let backend_metadata = self.catalog.get(backend)?;
        if internet_access && !backend_metadata.supports_internet_access {
            return Err(ControlError::new("INTERNET_ACCESS_UNAVAILABLE"));
        }
        match backend_metadata
            .runtime_kind
            .as_ref()
            .ok_or_else(|| ControlError::new("BACKEND_RUNTIME_KIND_MISSING"))?
        {
            RuntimeKind::Container => {
                let candidates = [
                    ("run", run.get("runtime")),
                    ("task", task.get("runtime")),
                    ("package", environment_metadata.default_runtime.as_ref()),
                ];
                let (source, runtime) = candidates
                    .into_iter()
                    .find_map(|(source, value)| value.map(|value| (source, value)))
                    .ok_or_else(|| ControlError::new("MISSING_IMAGE"))?;
                let image = runtime
                    .get("image")
                    .ok_or_else(|| ControlError::new("MISSING_IMAGE"))?;
                let resolved_image = image_resolver(image, task, plan.get("backend").unwrap())?;
                plan.insert(
                    "runtime".to_owned(),
                    json!({"image": resolved_image, "image_source": source}),
                );
            }
            RuntimeKind::Process => {
                // Package/task images describe an available container route. They do not
                // force a Process run to consume an OCI image. Only a user run override
                // is an explicit request for this execution and therefore conflicts.
                if run.get("runtime").is_some() {
                    return Err(ControlError::new("PROCESS_IMAGE_CONFLICT"));
                }
                let profile = plan
                    .get("backend")
                    .and_then(|v| v.pointer("/config/data/runtime_profile"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| ControlError::new("RUNTIME_PROFILE_MISSING"))?;
                if !environment_metadata.native_profiles.contains(profile) {
                    return Err(ControlError::new("NATIVE_PROFILE_NOT_VERIFIED"));
                }
            }
        }

        plan.insert("attempt_id".to_owned(), json!(1));
        plan.insert(
            "required_capabilities".to_owned(),
            Value::Array(required.into_iter().map(Value::String).collect()),
        );
        plan.insert("internet_access".to_owned(), Value::Bool(internet_access));
        plan.insert(
            "deadline_at_ms".to_owned(),
            json!(
                accepted_at_ms
                    + run
                        .pointer("/limits/total_timeout_ms")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| ControlError::new("INVALID_TOTAL_TIMEOUT"))?
            ),
        );
        let plan = seal_plan(&Value::Object(plan))?;
        validate_execution_plan(&plan, self.schema)?;
        Ok(plan)
    }

    fn resolve_role(
        &self,
        requested: &Value,
        role: &str,
        selected: &mut BTreeMap<String, Value>,
        required: &mut BTreeSet<String>,
    ) -> Result<Value> {
        if !self.catalog.get(requested)?.roles.contains(role) {
            return Err(ControlError::new(format!("COMPONENT_ROLE_MISMATCH:{role}")));
        }
        self.resolve_component(requested, selected, required)
    }

    fn resolve_component(
        &self,
        requested: &Value,
        selected: &mut BTreeMap<String, Value>,
        required: &mut BTreeSet<String>,
    ) -> Result<Value> {
        let metadata = self.catalog.get(requested)?.clone();
        self.schema
            .validate_shape("ResolvedComponent", &metadata.component)?;
        if !matches_ref(requested, &metadata.component)? {
            return Err(ControlError::new("COMPONENT_RESOLUTION_MISMATCH"));
        }
        let id = string(&metadata.component, "id")?.to_owned();
        if let Some(existing) = selected.get(&id) {
            if existing != &metadata.component {
                return Err(ControlError::new("CONFLICTING_COMPONENT_LOCK"));
            }
            return Ok(existing.clone());
        }
        selected.insert(id, metadata.component.clone());
        required.extend(metadata.required_capabilities);
        Ok(metadata.component)
    }

    fn resolve_tools(
        &self,
        agent: &Value,
        bindings: &[Value],
        selected: &mut BTreeMap<String, Value>,
        required: &mut BTreeSet<String>,
    ) -> Result<Vec<Value>> {
        if !matches_ref(agent, &self.agent_profile.agent)? {
            return Err(ControlError::new("AGENT_PROFILE_MISMATCH"));
        }
        let names: Vec<&str> = bindings
            .iter()
            .map(|binding| string(binding, "name"))
            .collect::<Result<_>>()?;
        if names.iter().copied().collect::<BTreeSet<_>>().len() != names.len() {
            return Err(ControlError::new("DUPLICATE_TOOL_NAME"));
        }
        let provided: BTreeSet<String> = names.iter().map(|name| (*name).to_owned()).collect();
        if !self.agent_profile.required_names.is_subset(&provided) {
            return Err(ControlError::new("MISSING_REQUIRED_TOOLS"));
        }
        let mut resolved = Vec::with_capacity(bindings.len());
        for binding in bindings {
            let name = string(binding, "name")?;
            let requested = binding
                .get("implementation")
                .ok_or_else(|| ControlError::new("MISSING_TOOL_IMPLEMENTATION"))?;
            let config_schema = binding
                .pointer("/config/schema_ref")
                .and_then(Value::as_str)
                .ok_or_else(|| ControlError::new("INVALID_TOOL_CONFIG"))?;
            let metadata = self.catalog.get(requested)?;
            if metadata.role_config_schemas.get("tool").map(String::as_str) != Some(config_schema) {
                return Err(ControlError::new("TOOL_CONFIG_SCHEMA_MISMATCH"));
            }

            let support = metadata
                .tool
                .as_ref()
                .ok_or_else(|| ControlError::new("TOOL_METADATA_MISSING"))?;
            if let Some(owner) = &support.native_agent {
                if owner != agent {
                    return Err(ControlError::new("NATIVE_TOOL_AGENT_MISMATCH"));
                }
                if metadata.component["version"] != owner["version"]
                    || metadata.component["digest"] != owner["digest"]
                {
                    return Err(ControlError::new("NATIVE_TOOL_VERSION_MISMATCH"));
                }
            }
            if support.execution_scope == "agent_state" && !support.required_capabilities.is_empty()
            {
                return Err(ControlError::new("STATE_TOOL_REQUESTS_EXTERNAL_CAPABILITY"));
            }
            let implementation = self.resolve_role(requested, "tool", selected, required)?;
            required.extend(support.required_capabilities.iter().cloned());
            let mut tool = json!({
                "name": name,
                "implementation": implementation,
                "config": binding.get("config").cloned().ok_or_else(|| ControlError::new("MISSING_TOOL_CONFIG"))?,
                "execution_scope": support.execution_scope,
                "required_capabilities": support.required_capabilities,
            });
            if let Some(owner) = &support.native_agent {
                tool["native_agent"] = owner.clone();
            }
            resolved.push(tool);
        }
        Ok(resolved)
    }
}

pub fn seal_plan(plan: &Value) -> Result<Value> {
    let mut object = object(plan, "ExecutionPlan")?.clone();
    object.remove("plan_digest");
    let value = Value::Object(object.clone());
    object.insert("plan_digest".to_owned(), Value::String(digest(&value)?));
    Ok(Value::Object(object))
}

pub fn retry_execution_plan(
    plan: &Value,
    schema: &ContractSchema,
    max_attempts: u64,
) -> Result<Value> {
    validate_execution_plan(plan, schema)?;
    let attempt_id = u64_field(plan, "attempt_id")?;
    if attempt_id >= max_attempts {
        return Err(ControlError::new("MAX_ATTEMPTS_REACHED"));
    }
    let mut next = object(plan, "ExecutionPlan")?.clone();
    next.insert("attempt_id".to_owned(), json!(attempt_id + 1));
    seal_plan(&Value::Object(next))
}

pub fn validate_execution_plan(plan: &Value, schema: &ContractSchema) -> Result<()> {
    schema.validate_shape("ExecutionPlan", plan)?;
    schema.validate_shape("ResolvedComponent", &plan["dataset_package"])?;
    schema.validate_shape("TypedConfig", &plan["environment"])?;
    if plan["scoring"]["enabled"] == true {
        schema.validate_shape("TypedConfig", &plan["scoring"]["config"])?;
    }
    if seal_plan(plan)?.get("plan_digest") != plan.get("plan_digest") {
        return Err(ControlError::new("PLAN_DIGEST_MISMATCH"));
    }
    let task = plan
        .get("task")
        .ok_or_else(|| ControlError::new("MISSING_TASK"))?;
    if digest(
        task.get("input")
            .ok_or_else(|| ControlError::new("MISSING_TASK_INPUT"))?,
    )? != string(task, "input_digest")?
    {
        return Err(ControlError::new("INPUT_DIGEST_MISMATCH"));
    }
    let tools = array(plan, "tools")?;
    let names: Vec<&str> = tools
        .iter()
        .map(|v| string(v, "name"))
        .collect::<Result<_>>()?;
    if names.iter().copied().collect::<BTreeSet<_>>().len() != names.len() {
        return Err(ControlError::new("DUPLICATE_TOOL_NAME"));
    }
    let required: Vec<&str> = array(plan, "required_capabilities")?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| ControlError::new("INVALID_CAPABILITY"))
        })
        .collect::<Result<_>>()?;
    if required.iter().copied().collect::<BTreeSet<_>>().len() != required.len() {
        return Err(ControlError::new("DUPLICATE_CAPABILITY"));
    }
    let required_set: BTreeSet<&str> = required.into_iter().collect();
    for tool in tools {
        schema.validate_shape("ResolvedToolBinding", tool)?;
        if let Some(owner) = tool.get("native_agent") {
            if Some(owner) != plan.pointer("/agent/implementation") {
                return Err(ControlError::new("NATIVE_TOOL_AGENT_MISMATCH"));
            }
            if tool["implementation"]["version"] != owner["version"]
                || tool["implementation"]["digest"] != owner["digest"]
            {
                return Err(ControlError::new("NATIVE_TOOL_VERSION_MISMATCH"));
            }
        }
        for capability in array(tool, "required_capabilities")? {
            let capability = capability
                .as_str()
                .ok_or_else(|| ControlError::new("INVALID_CAPABILITY"))?;
            if !required_set.contains(capability) {
                return Err(ControlError::new("MISSING_TOOL_CAPABILITIES"));
            }
        }
    }
    let limits = plan
        .get("limits")
        .ok_or_else(|| ControlError::new("MISSING_LIMITS"))?;
    schema.validate_shape("Limits", limits)?;
    if u64_field(limits, "finalize_reserve_ms")? > u64_field(limits, "total_timeout_ms")? {
        return Err(ControlError::new("INVALID_FINALIZE_RESERVE"));
    }
    validate_run_purpose(plan)?;
    if plan["scoring"]["enabled"] == false && plan.get("private_data").is_some() {
        return Err(ControlError::new("PRIVATE_DATA_WITHOUT_SCORER"));
    }
    Ok(())
}

/// Shared semantic check for the submission and resolved plan boundaries.
pub fn validate_run_purpose(config: &Value) -> Result<()> {
    let purpose = string(config, "purpose")?;
    if !["evaluation", "training", "trajectory_collection"].contains(&purpose) {
        return Err(ControlError::new("INVALID_PURPOSE"));
    }
    match (purpose, config.get("training")) {
        ("training", None) => return Err(ControlError::new("MISSING_TRAINING_CONFIGURATION")),
        ("evaluation" | "trajectory_collection", Some(_)) => {
            return Err(ControlError::new("UNEXPECTED_TRAINING_CONFIGURATION"));
        }
        _ => {}
    }
    let scoring = config
        .get("scoring")
        .ok_or_else(|| ControlError::new("MISSING_SCORING_CONFIGURATION"))?;
    let scoring = object(scoring, "scoring")?;
    if scoring
        .keys()
        .any(|key| key != "enabled" && key != "config")
    {
        return Err(ControlError::new("UNKNOWN_SCORING_FIELD"));
    }
    let enabled = scoring
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| ControlError::new("INVALID_SCORING_ENABLED"))?;
    if enabled != scoring.contains_key("config") {
        return Err(ControlError::new("SCORING_CONFIG_MISMATCH"));
    }
    if enabled {
        object(&scoring["config"], "scoring.config")?;
    } else if purpose != "trajectory_collection" {
        return Err(ControlError::new("SCORING_REQUIRED"));
    }
    Ok(())
}

/// Server must apply this together with full schema and lease validation.
/// The plan is the source of scoring intent; absent results never disable scoring.
pub fn validate_result_for_plan(
    result: &Value,
    plan: &Value,
    schema: &ContractSchema,
) -> Result<()> {
    schema.validate_shape("EpisodeResult", result)?;
    for field in ["run_id", "episode_id", "attempt_id"] {
        if result.get(field) != plan.get(field) {
            return Err(ControlError::new("RESULT_IDENTITY_MISMATCH"));
        }
    }
    if result.get("task_id") != plan.pointer("/task/task_id") {
        return Err(ControlError::new("RESULT_IDENTITY_MISMATCH"));
    }
    if plan["scoring"]["enabled"] == false && result.get("score").is_some() {
        return Err(ControlError::new("UNEXPECTED_SCORE"));
    }
    if let Some(score) = result.get("score") {
        schema.validate_shape("ScoreResult", score)?;
        if score.get("scorer") != plan.get("dataset_package") {
            return Err(ControlError::new("RESULT_SCORER_MISMATCH"));
        }
    }
    if string(result, "execution_status")? == "completed" {
        for field in [
            "final_answer",
            "termination_reason",
            "trajectory_ref",
            "started_at_ms",
        ] {
            if result.get(field).is_none_or(Value::is_null) {
                return Err(ControlError::new("INCOMPLETE_RESULT"));
            }
        }
        if plan["scoring"]["enabled"] == true
            && result.pointer("/score/status").and_then(Value::as_str) != Some("ok")
        {
            return Err(ControlError::new("MISSING_SUCCESSFUL_SCORE"));
        }
    }
    Ok(())
}

pub fn verify_actual_tools(expected: &[Value], actual: &[Value]) -> Result<()> {
    fn routes(values: &[Value]) -> Result<BTreeMap<String, Value>> {
        let mut routes = BTreeMap::new();
        for value in values {
            let name = string(value, "name")?.to_owned();
            if routes.insert(name, value.clone()).is_some() {
                return Err(ControlError::new("DUPLICATE_ACTUAL_TOOL_NAME"));
            }
        }
        Ok(routes)
    }
    if routes(expected)? != routes(actual)? {
        return Err(ControlError::new("ACTUAL_TOOL_ROUTING_MISMATCH"));
    }
    Ok(())
}

fn matches_ref(requested: &Value, resolved: &Value) -> Result<bool> {
    Ok(string(requested, "id")? == string(resolved, "id")?
        && string(requested, "version")? == string(resolved, "version")?
        && requested
            .get("digest")
            .is_none_or(|value| Some(value) == resolved.get("digest")))
}

fn component_key(reference: &Value) -> Result<(String, String)> {
    Ok((
        string(reference, "id")?.to_owned(),
        string(reference, "version")?.to_owned(),
    ))
}
