//! Tool descriptors and execution results.

use crate::permission::{PermissionLevel, Risk};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Everything the AI is allowed to know about a tool.
///
/// The model may only ever request tools present in this registry — there is no
/// "run arbitrary code" escape hatch anywhere in the protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    /// Stable dotted id, e.g. `fs.delete`.
    pub name: String,
    /// One paragraph the model reads when deciding whether to use the tool.
    pub description: String,
    /// JSON Schema (subset: type, properties, required, items, enum, description).
    pub input_schema: Value,
    /// Shape of `result.data` on success — used for verification.
    pub output_schema: Value,
    pub permission_level: PermissionLevel,
    pub risk: Risk,
    /// Hard execution timeout enforced by the runtime.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// True when the tool can destroy or irreversibly alter user data.
    #[serde(default)]
    pub destructive: bool,
    /// Human-facing explanation shown in confirmation prompts ("why is this asked?").
    #[serde(default)]
    pub confirmation_explanation: String,
}

fn default_timeout() -> u64 {
    30_000
}

impl ToolDescriptor {
    /// The subset of the descriptor sent to the LLM (no internal metadata).
    pub fn to_llm_json(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.input_schema,
            }
        })
    }
}

/// A tool call proposed by the brain, before the gate has seen it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecution {
    /// Id correlating the call with the model's tool message.
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// Result of one tool execution (or refusal). Never faked: `ok` reflects reality.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub name: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Machine-readable failure class: `user_declined`, `denied`, `timeout`,
    /// `invalid_arguments`, `not_found`, `execution_failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default)]
    pub duration_ms: u64,
    /// Set when the gate asked for confirmation before executing.
    #[serde(default)]
    pub confirmation_required: bool,
}

impl ToolResult {
    pub fn failure(id: impl Into<String>, name: impl Into<String>, code: &str, error: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            ok: false,
            data: None,
            error: Some(error.into()),
            error_code: Some(code.to_string()),
            duration_ms: 0,
            confirmation_required: false,
        }
    }

    /// Shape fed back to the model as a `tool` message.
    pub fn to_model_value(&self) -> Value {
        serde_json::json!({
            "ok": self.ok,
            "data": self.data,
            "error": self.error,
            "error_code": self.error_code,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> ToolDescriptor {
        ToolDescriptor {
            name: "fs.read".into(),
            description: "Read a UTF-8 text file".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            output_schema: serde_json::json!({"type": "object"}),
            permission_level: PermissionLevel::Level1,
            risk: Risk::Low,
            timeout_ms: 5_000,
            destructive: false,
            confirmation_explanation: String::new(),
        }
    }

    #[test]
    fn llm_shape_is_openai_function_call_format() {
        let v = descriptor().to_llm_json();
        assert_eq!(v["type"], "function");
        assert_eq!(v["function"]["name"], "fs.read");
        assert!(v["function"]["parameters"]["required"].is_array());
    }

    #[test]
    fn failure_results_are_never_ok() {
        let r = ToolResult::failure("t1", "fs.delete", "user_declined", "cancelled");
        assert!(!r.ok);
        assert_eq!(r.error_code.as_deref(), Some("user_declined"));
        assert_eq!(r.to_model_value()["ok"], false);
    }
}