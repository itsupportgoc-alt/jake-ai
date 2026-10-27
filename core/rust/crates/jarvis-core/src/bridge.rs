//! Requests the **brain may send to the runtime**.
//!
//! This is the only surface where the less-privileged process can ask for
//! something, and every answer it gets is either informational or honest about
//! being unimplemented. Tool execution goes through the same registry + gate the
//! local loop uses (see `RuntimeRpcHandler` in the CLI for the live wiring).

use crate::python::RpcHandler;
use crate::tools::ToolRegistry;
use jarvis_protocol::rpc::{self, RpcError};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Handles `log.event`, `tool.list` and the (not yet implemented) `memory.*`.
pub struct CoreRpcHandler {
    registry: Arc<ToolRegistry>,
}

impl CoreRpcHandler {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }
}

impl RpcHandler for CoreRpcHandler {
    fn handle(
        &self,
        method: String,
        params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send>> {
        let registry = self.registry.clone();
        Box::pin(async move {
            match method.as_str() {
                "log.event" => {
                    let level = params.get("level").and_then(Value::as_str).unwrap_or("info");
                    let message = params
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("(brain log)");
                    let fields = params.get("fields").cloned().unwrap_or(Value::Null);
                    match level {
                        "error" => tracing::error!(target: "brain", fields = %fields, "{}", message),
                        "warn" => tracing::warn!(target: "brain", fields = %fields, "{}", message),
                        "debug" => tracing::debug!(target: "brain", fields = %fields, "{}", message),
                        "trace" => tracing::trace!(target: "brain", fields = %fields, "{}", message),
                        _ => tracing::info!(target: "brain", fields = %fields, "{}", message),
                    }
                    Ok(json!({"ok": true}))
                }
                "tool.list" => Ok(json!({
                    "ok": true,
                    "tools": registry.descriptors(),
                })),
                // Honest: the brain must surface this as a failure, never invent data.
                "memory.put" | "memory.search" | "memory.get" | "memory.delete" => {
                    Err(RpcError::new(
                        rpc::METHOD_NOT_FOUND,
                        "long-term memory is not implemented yet (Phase 4); nothing was stored",
                    ))
                }
                _ => Err(RpcError::new(
                    rpc::METHOD_NOT_FOUND,
                    format!("the runtime does not handle `{method}`"),
                )),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn log_event_is_accepted_and_never_panics() {
        let handler = CoreRpcHandler::new(Arc::new(ToolRegistry::new()));
        let res = handler
            .handle(
                "log.event".into(),
                json!({"level": "warn", "message": "hello from brain", "fields": {"a": 1}}),
            )
            .await;
        assert_eq!(res.unwrap()["ok"], true);
    }

    #[tokio::test]
    async fn tool_list_returns_the_registry() {
        let mut registry = ToolRegistry::new();
        crate::builtin::register_all(&mut registry).unwrap();
        let handler = CoreRpcHandler::new(Arc::new(registry));
        let res = handler.handle("tool.list".into(), json!({})).await.unwrap();
        assert!(res["tools"].as_array().unwrap().len() >= 6);
    }

    #[tokio::test]
    async fn memory_says_not_implemented_instead_of_faking() {
        let handler = CoreRpcHandler::new(Arc::new(ToolRegistry::new()));
        for method in ["memory.put", "memory.search", "memory.delete"] {
            let err = handler.handle(method.into(), json!({})).await.unwrap_err();
            assert_eq!(err.code, rpc::METHOD_NOT_FOUND);
            assert!(err.message.contains("not implemented"), "{method}: {}", err.message);
        }
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let handler = CoreRpcHandler::new(Arc::new(ToolRegistry::new()));
        let err = handler.handle("evil.exec".into(), json!({})).await.unwrap_err();
        assert_eq!(err.code, rpc::METHOD_NOT_FOUND);
    }
}