//! Tool registry, JSON-Schema argument validation, and the gated dispatcher.
//!
//! The brain may only *propose* a call to a tool that exists here with arguments that
//! validate — everything else is rejected before it can touch the operating system.

use crate::cancel::{CancelFlag, Cancelled};
use crate::permission::{ConfirmationRequest, Confirmer, Decision, PermissionGate};
use jarvis_protocol::tool::{ToolDescriptor, ToolExecution, ToolResult};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// A tool failure that still carries a machine-readable class.
#[derive(Debug, Clone)]
pub struct ToolError {
    pub code: String,
    pub message: String,
}

impl ToolError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new("not_found", msg)
    }

    pub fn failed(msg: impl Into<String>) -> Self {
        Self::new("execution_failed", msg)
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::new("invalid_arguments", msg)
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ToolError {}

/// Lets tools use `?` on filesystem calls.
impl From<std::io::Error> for ToolError {
    fn from(e: std::io::Error) -> Self {
        let code = match e.kind() {
            std::io::ErrorKind::NotFound => "not_found",
            std::io::ErrorKind::PermissionDenied => "permission_denied",
            std::io::ErrorKind::InvalidInput => "invalid_arguments",
            _ => "io_error",
        };
        ToolError::new(code, format!("{e}"))
    }
}

/// Everything a tool may consult at runtime. Tools receive **no** ambient authority:
/// no config with secrets, no event bus, no brain handle.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Directory the tool's file access is confined to (path escapes are rejected).
    pub roots: Vec<PathBuf>,
    pub correlation_id: Option<Uuid>,
    pub cancel: CancelFlag,
    /// Project root (for tools that need the workspace, e.g. dev tools later).
    pub project_root: PathBuf,
    /// Privacy Mode: capabilities like screen capture must refuse to run.
    pub privacy_mode: bool,
}

impl ToolContext {
    pub fn new(project_root: PathBuf) -> Self {
        let mut roots = vec![project_root.clone()];
        if let Some(home) = dirs::home_dir() {
            roots.push(home);
        }
        Self {
            roots,
            correlation_id: None,
            cancel: CancelFlag::new(),
            project_root,
            privacy_mode: false,
        }
    }

    /// Resolve `path` and ensure it stays inside one of the allowed roots.
    pub fn resolve_within_roots(&self, path: &str) -> Result<PathBuf, ToolError> {
        use std::path::Component;
        let raw = std::path::Path::new(path);
        let candidate = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.project_root.join(raw)
        };
        // Normalise `.`/`..` lexically before touching the filesystem.
        let mut normalised = PathBuf::new();
        for comp in candidate.components() {
            match comp {
                Component::ParentDir => {
                    normalised.pop();
                }
                Component::CurDir => {}
                other => normalised.push(other),
            }
        }
        let canonical_file = std::fs::canonicalize(&normalised).unwrap_or(normalised.clone());
        let canonical_dir = canonical_file.is_dir().then_some(()).map(|_| canonical_file.clone());
        let target = canonical_dir.unwrap_or_else(|| {
            canonical_file
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| normalised.clone())
        });
        for root in &self.roots {
            let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
            if target.starts_with(&canonical_root) || normalised.starts_with(root) {
                return Ok(normalised);
            }
        }
        Err(ToolError::new(
            "permission_denied",
            format!(
                "path `{}` is outside the allowed roots ({})",
                normalised.display(),
                self.roots
                    .iter()
                    .map(|r| r.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ))
    }
}

/// A capability the brain may request. Implementations are synchronous and are run
/// on the blocking pool under a timeout.
pub trait Tool: Send + Sync + 'static {
    fn descriptor(&self) -> &ToolDescriptor;
    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError>;
}

pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        let name = tool.descriptor().name.clone();
        if name.is_empty() {
            return Err(ToolError::failed("tool name must not be empty"));
        }
        if self.tools.contains_key(&name) {
            return Err(ToolError::failed(format!("tool `{name}` is already registered")));
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Descriptors sorted by name (stable output for logs and diffs).
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools.values().map(|t| t.descriptor().clone()).collect()
    }

    /// OpenAI-compatible `tools` array handed to the model.
    pub fn llm_tools(&self) -> Vec<Value> {
        self.tools.values().map(|t| t.descriptor().to_llm_json()).collect()
    }

    /// Validate model-produced arguments against the tool's schema (subset:
    /// `type`, `required`, `properties`, `items`, `enum`, `additionalProperties:false`).
    pub fn validate_args(&self, name: &str, args: &Value) -> Result<(), ToolError> {
        let tool = self
            .get(name)
            .ok_or_else(|| ToolError::not_found(format!("unknown tool `{name}`")))?;
        let mut errors = Vec::new();
        validate(&tool.descriptor().input_schema, args, "$", &mut errors);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ToolError::invalid(format!(
                "arguments for `{name}` failed validation: {}",
                errors.join("; ")
            )))
        }
    }

    /// Full pipeline: lookup → validate → gate → confirm → execute (timeout) → result.
    pub async fn dispatch(
        &self,
        call: ToolExecution,
        ctx: &ToolContext,
        gate: &PermissionGate,
        confirmer: &Arc<dyn Confirmer>,
        default_timeout: Duration,
    ) -> ToolResult {
        let started = Instant::now();
        let ToolExecution { id, name, arguments } = call;

        let tool = match self.get(&name) {
            Some(t) => t.clone(),
            None => {
                return ToolResult {
                    id,
                    name: name.clone(),
                    ok: false,
                    data: None,
                    error: Some(format!("unknown tool `{name}`")),
                    error_code: Some("not_found".into()),
                    duration_ms: 0,
                    confirmation_required: false,
                }
            }
        };

        if let Err(e) = self.validate_args(&name, &arguments) {
            return ToolResult {
                id,
                name,
                ok: false,
                data: None,
                error: Some(e.message),
                error_code: Some(e.code),
                duration_ms: started.elapsed().as_millis() as u64,
                confirmation_required: false,
            };
        }

        let descriptor = tool.descriptor();
        let decision = gate.evaluate(descriptor, &arguments);
        let mut confirmation_required = false;

        match decision {
            Decision::Deny { reason } => {
                return ToolResult {
                    id,
                    name,
                    ok: false,
                    data: None,
                    error: Some(format!("permission denied: {reason}")),
                    error_code: Some("denied".into()),
                    duration_ms: started.elapsed().as_millis() as u64,
                    confirmation_required: false,
                }
            }
            Decision::Allow { reason } => {
                tracing::debug!(tool = %name, %reason, "gate allowed");
            }
            Decision::RequireConfirmation { reason } => {
                confirmation_required = true;
                let request = ConfirmationRequest {
                    tool: name.clone(),
                    arguments: truncate(&arguments.to_string(), 600),
                    level: descriptor.permission_level,
                    risk: descriptor.risk,
                    reason,
                    explanation: descriptor.confirmation_explanation.clone(),
                };
                let accepted = confirmer.confirm(request).await;
                if !accepted {
                    return ToolResult {
                        id,
                        name,
                        ok: false,
                        data: None,
                        error: Some("the user declined this action".into()),
                        error_code: Some("user_declined".into()),
                        duration_ms: started.elapsed().as_millis() as u64,
                        confirmation_required: true,
                    };
                }
            }
        }

        if let Err(Cancelled) = ctx.cancel.check() {
            return ToolResult {
                id,
                name,
                ok: false,
                data: None,
                error: Some("cancelled by emergency stop".into()),
                error_code: Some("cancelled".into()),
                duration_ms: started.elapsed().as_millis() as u64,
                confirmation_required,
            };
        }

        let timeout = Duration::from_millis(descriptor.timeout_ms.max(1).min(default_timeout.as_millis() as u64));
        let tool_handle = tool.clone();
        let exec_ctx = ctx.clone();
        let exec_args = arguments.clone();
        let blocking = tokio::task::spawn_blocking(move || tool_handle.execute(&exec_args, &exec_ctx));

        let outcome = match tokio::time::timeout(timeout, blocking).await {
            Err(_) => Err(ToolError::new(
                "timeout",
                format!("tool `{name}` exceeded {:?}", timeout),
            )),
            Ok(Err(join_err)) => Err(ToolError::failed(format!("tool panicked: {join_err}"))),
            Ok(Ok(res)) => res,
        };

        let duration_ms = started.elapsed().as_millis() as u64;
        match outcome {
            Ok(data) => {
                tracing::debug!(tool = %name, duration_ms, "tool succeeded");
                ToolResult {
                    id,
                    name,
                    ok: true,
                    data: Some(data),
                    error: None,
                    error_code: None,
                    duration_ms,
                    confirmation_required,
                }
            }
            Err(e) => {
                tracing::warn!(tool = %name, code = %e.code, duration_ms, "tool failed: {}", e.message);
                ToolResult {
                    id,
                    name,
                    ok: false,
                    data: None,
                    error: Some(e.message),
                    error_code: Some(e.code),
                    duration_ms,
                    confirmation_required,
                }
            }
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut cut = max;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}…", &s[..cut])
    }
}

/// Validate `value` against `schema`, appending human-readable problems to `errors`.
pub fn validate(schema: &Value, value: &Value, path: &str, errors: &mut Vec<String>) {
    let Some(obj) = schema.as_object() else { return };

    if let Some(expected) = obj.get("type") {
        let types: Vec<String> = match expected {
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
            _ => Vec::new(),
        };
        if !types.is_empty() && !types.iter().any(|t| type_matches(t, value)) {
            errors.push(format!(
                "{path}: expected {}, got {}",
                types.join("|"),
                json_type(value)
            ));
            return;
        }
    }

    if let Some(Value::Array(required)) = obj.get("required") {
        if let Some(props) = value.as_object() {
            for key in required.iter().filter_map(|v| v.as_str()) {
                if !props.contains_key(key) {
                    errors.push(format!("{path}: missing required property `{key}`"));
                }
            }
        }
    }

    if let Some(Value::Array(items)) = obj.get("enum") {
        if !items.iter().any(|e| e == value) {
            errors.push(format!(
                "{path}: value {} is not one of {}",
                value,
                truncate(&serde_json::to_string(items).unwrap_or_default(), 120)
            ));
        }
    }

    if let Some(Value::Bool(false)) = obj.get("additionalProperties") {
        if let Some(props) = value.as_object() {
            if let Some(Value::Object(known)) = obj.get("properties") {
                for key in props.keys() {
                    if !known.contains_key(key) {
                        errors.push(format!("{path}: unexpected property `{key}`"));
                    }
                }
            }
        }
    }

    if let Some(Value::Object(properties)) = obj.get("properties") {
        if let Some(props) = value.as_object() {
            for (key, sub_schema) in properties {
                if let Some(sub_value) = props.get(key) {
                    validate(sub_schema, sub_value, &format!("{path}.{key}"), errors);
                }
            }
        }
    }

    if let Some(items) = obj.get("items") {
        if let Some(arr) = value.as_array() {
            for (i, item) in arr.iter().enumerate() {
                validate(items, item, &format!("{path}[{i}]"), errors);
            }
        }
    }
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_matches(expected: &str, value: &Value) -> bool {
    match expected {
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        _ => true, // unknown type keyword: don't reject blindly
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{AlwaysConfirm, AutonomyLevel, NeverConfirm};
    use jarvis_protocol::permission::{PermissionLevel, Risk};
    use serde_json::json;

    struct EchoTool {
        descriptor: ToolDescriptor,
    }

    impl Tool for EchoTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.descriptor
        }
        fn execute(&self, args: &Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
            Ok(json!({"echo": args}))
        }
    }

    struct SleepTool {
        descriptor: ToolDescriptor,
    }

    impl Tool for SleepTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.descriptor
        }
        fn execute(&self, _args: &Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
            std::thread::sleep(std::time::Duration::from_millis(500));
            Ok(json!({"slept": true}))
        }
    }

    struct BoomTool {
        descriptor: ToolDescriptor,
    }

    impl Tool for BoomTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.descriptor
        }
        fn execute(&self, _args: &Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
            Err(ToolError::failed("intentional failure"))
        }
    }

    fn descriptor(name: &str, level: PermissionLevel, timeout_ms: u64) -> ToolDescriptor {
        ToolDescriptor {
            name: name.into(),
            description: "test tool".into(),
            input_schema: json!({
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"]
            }),
            output_schema: json!({"type": "object"}),
            permission_level: level,
            risk: Risk::Low,
            timeout_ms,
            destructive: false,
            confirmation_explanation: String::new(),
        }
    }

    fn registry_with(tool: Arc<dyn Tool>) -> ToolRegistry {
        let mut r = ToolRegistry::new();
        r.register(tool).unwrap();
        r
    }

    fn ctx() -> ToolContext {
        ToolContext::new(std::env::temp_dir())
    }

    fn call(name: &str, args: Value) -> ToolExecution {
        ToolExecution {
            id: "call-1".into(),
            name: name.into(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn dispatch_runs_an_allowed_tool() {
        let registry = registry_with(Arc::new(EchoTool {
            descriptor: descriptor("echo", PermissionLevel::Level0, 5000),
        }));
        let gate = PermissionGate::new(AutonomyLevel::new(3).unwrap());
        let confirmer: Arc<dyn Confirmer> = Arc::new(NeverConfirm);
        let res = registry
            .dispatch(
                call("echo", json!({"value": "hi"})),
                &ctx(),
                &gate,
                &confirmer,
                Duration::from_secs(5),
            )
            .await;
        assert!(res.ok, "{res:?}");
        assert_eq!(res.data.unwrap()["echo"]["value"], "hi");
    }

    #[tokio::test]
    async fn unknown_tool_is_reported_not_faked() {
        let registry = ToolRegistry::new();
        let gate = PermissionGate::new(AutonomyLevel::new(3).unwrap());
        let confirmer: Arc<dyn Confirmer> = Arc::new(NeverConfirm);
        let res = registry
            .dispatch(
                call("nope", json!({"value": "x"})),
                &ctx(),
                &gate,
                &confirmer,
                Duration::from_secs(5),
            )
            .await;
        assert!(!res.ok);
        assert_eq!(res.error_code.as_deref(), Some("not_found"));
    }

    #[tokio::test]
    async fn invalid_arguments_never_reach_the_tool() {
        let registry = registry_with(Arc::new(EchoTool {
            descriptor: descriptor("echo", PermissionLevel::Level0, 5000),
        }));
        let gate = PermissionGate::new(AutonomyLevel::new(3).unwrap());
        let confirmer: Arc<dyn Confirmer> = Arc::new(AlwaysConfirm);
        let res = registry
            .dispatch(call("echo", json!({"wrong": 1})), &ctx(), &gate, &confirmer, Duration::from_secs(5))
            .await;
        assert!(!res.ok);
        assert_eq!(res.error_code.as_deref(), Some("invalid_arguments"));
    }

    #[tokio::test]
    async fn declining_confirmation_blocks_execution() {
        let registry = registry_with(Arc::new(EchoTool {
            descriptor: descriptor("echo", PermissionLevel::Level3, 5000),
        }));
        let gate = PermissionGate::new(AutonomyLevel::new(3).unwrap());
        let confirmer: Arc<dyn Confirmer> = Arc::new(NeverConfirm);
        let res = registry
            .dispatch(call("echo", json!({"value": "x"})), &ctx(), &gate, &confirmer, Duration::from_secs(5))
            .await;
        assert!(!res.ok);
        assert_eq!(res.error_code.as_deref(), Some("user_declined"));
        assert!(res.confirmation_required);
    }

    #[tokio::test]
    async fn confirming_allows_a_high_risk_tool() {
        let registry = registry_with(Arc::new(EchoTool {
            descriptor: descriptor("echo", PermissionLevel::Level3, 5000),
        }));
        let gate = PermissionGate::new(AutonomyLevel::new(3).unwrap());
        let confirmer: Arc<dyn Confirmer> = Arc::new(AlwaysConfirm);
        let res = registry
            .dispatch(call("echo", json!({"value": "x"})), &ctx(), &gate, &confirmer, Duration::from_secs(5))
            .await;
        assert!(res.ok, "{res:?}");
        assert!(res.confirmation_required);
    }

    #[tokio::test]
    async fn slow_tools_time_out_with_an_honest_error() {
        let registry = registry_with(Arc::new(SleepTool {
            descriptor: descriptor("slow", PermissionLevel::Level0, 50),
        }));
        let gate = PermissionGate::new(AutonomyLevel::new(3).unwrap());
        let confirmer: Arc<dyn Confirmer> = Arc::new(NeverConfirm);
        let res = registry
            .dispatch(call("slow", json!({"value": "x"})), &ctx(), &gate, &confirmer, Duration::from_secs(5))
            .await;
        assert!(!res.ok);
        assert_eq!(res.error_code.as_deref(), Some("timeout"));
    }

    #[tokio::test]
    async fn tool_errors_surface_with_their_message() {
        let registry = registry_with(Arc::new(BoomTool {
            descriptor: descriptor("boom", PermissionLevel::Level0, 5000),
        }));
        let gate = PermissionGate::new(AutonomyLevel::new(3).unwrap());
        let confirmer: Arc<dyn Confirmer> = Arc::new(NeverConfirm);
        let res = registry
            .dispatch(call("boom", json!({"value": "x"})), &ctx(), &gate, &confirmer, Duration::from_secs(5))
            .await;
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("intentional"));
        assert_eq!(res.error_code.as_deref(), Some("execution_failed"));
    }

    #[tokio::test]
    async fn cancelled_flag_blocks_execution() {
        let registry = registry_with(Arc::new(EchoTool {
            descriptor: descriptor("echo", PermissionLevel::Level0, 5000),
        }));
        let gate = PermissionGate::new(AutonomyLevel::new(3).unwrap());
        let confirmer: Arc<dyn Confirmer> = Arc::new(NeverConfirm);
        let c = ctx();
        c.cancel.cancel("emergency stop"); // simulate the stop
        let res = registry
            .dispatch(call("echo", json!({"value": "x"})), &c, &gate, &confirmer, Duration::from_secs(5))
            .await;
        assert!(!res.ok);
        assert_eq!(res.error_code.as_deref(), Some("cancelled"));
    }

    #[test]
    fn duplicate_registration_is_rejected() {
        let mut r = ToolRegistry::new();
        r.register(Arc::new(EchoTool { descriptor: descriptor("echo", PermissionLevel::Level0, 100) }))
            .unwrap();
        let err = r
            .register(Arc::new(EchoTool { descriptor: descriptor("echo", PermissionLevel::Level0, 100) }))
            .unwrap_err();
        assert!(err.message.contains("already registered"));
    }

    #[test]
    fn schema_validation_catches_missing_and_wrong_types() {
        let mut r = ToolRegistry::new();
        r.register(Arc::new(EchoTool { descriptor: descriptor("echo", PermissionLevel::Level0, 100) }))
            .unwrap();
        assert!(r.validate_args("echo", &json!({"value": "ok"})).is_ok());
        assert!(r.validate_args("echo", &json!({})).is_err());
        assert!(r.validate_args("echo", &json!({"value": 42})).is_err());
        assert!(r.validate_args("ghost", &json!({})).is_err());
    }

    #[test]
    fn nested_schema_paths_are_reported() {
        let mut errors = Vec::new();
        let schema = json!({
            "type": "object",
            "properties": {
                "items": { "type": "array", "items": { "type": "object", "required": ["id"] } }
            }
        });
        validate(&schema, &json!({"items": [{"name": "x"}]}), "$", &mut errors);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("$.items[0]"), "{}", errors[0]);
        assert!(errors[0].contains("`id`"), "{}", errors[0]);
    }

    #[test]
    fn absolute_paths_outside_roots_are_rejected() {
        let ctx = ToolContext::new(std::env::temp_dir());
        let err = ctx.resolve_within_roots("C:\\Windows\\System32\\config").unwrap_err();
        assert_eq!(err.code, "permission_denied");
    }

    #[test]
    fn traversal_that_escapes_every_allowed_root_is_rejected() {
        let ctx = ToolContext::new(std::env::temp_dir());
        // Enough `..` to climb above both the project root and the user's home.
        let deep = "../../../../../../../../etc/passwd";
        let err = ctx.resolve_within_roots(deep).unwrap_err();
        assert_eq!(err.code, "permission_denied", "escaped the roots");
    }

    #[test]
    fn traversal_that_stays_inside_a_root_is_allowed() {
        let ctx = ToolContext::new(std::env::temp_dir());
        // temp dir is <home>/AppData/Local/Temp — one `..` stays inside home,
        // which is an allowed root by policy.
        let result = ctx.resolve_within_roots("../anything.txt");
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn llm_tools_are_openai_shaped() {
        let mut r = ToolRegistry::new();
        r.register(Arc::new(EchoTool { descriptor: descriptor("echo", PermissionLevel::Level0, 100) }))
            .unwrap();
        let tools = r.llm_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["function"]["name"], "echo");
    }
}