//! Typed events published on the runtime event bus.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Severity, mirrored into the log so one severity scale is used everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    /// A user turn arrived (text or voice).
    UserMessage,
    /// The assistant produced a reply.
    AssistantMessage,
    /// The brain proposed a tool call.
    ToolRequested,
    /// A tool finished (success or failure) — includes duration and outcome.
    ToolCompleted,
    /// The gate paused for confirmation.
    PermissionRequested,
    /// The user answered (confirmed / declined).
    PermissionResolved,
    /// One iteration of the agent loop.
    AgentStep,
    /// A multi-step task started / finished.
    TaskStarted,
    TaskFinished,
    /// Emergency stop triggered.
    EmergencyStop,
    /// Brain/provider health changes (online/offline/local fallback).
    ProviderStatus,
    /// Something failed. Never silent.
    Error,
    /// Something degraded but recovered.
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: Uuid,
    pub ts: DateTime<Utc>,
    /// Groups every event belonging to one user turn / task.
    pub correlation_id: Option<Uuid>,
    pub kind: EventKind,
    /// Human-readable summary. Never contains secrets (the logger redacts anyway).
    pub message: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

impl Event {
    pub fn new(kind: EventKind, message: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            ts: Utc::now(),
            correlation_id: None,
            kind,
            message: message.into(),
            data: Value::Null,
        }
    }

    pub fn with_correlation(mut self, id: Uuid) -> Self {
        self.correlation_id = Some(id);
        self
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = data;
        self
    }

    /// `tracing` level for this event.
    pub fn level(&self) -> tracing::Level {
        match self.kind {
            EventKind::Error => tracing::Level::ERROR,
            EventKind::Warning => tracing::Level::WARN,
            EventKind::AgentStep
            | EventKind::ToolRequested
            | EventKind::ToolCompleted
            | EventKind::PermissionRequested
            | EventKind::PermissionResolved => tracing::Level::DEBUG,
            _ => tracing::Level::INFO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_carry_correlation_and_data() {
        let id = Uuid::new_v4();
        let e = Event::new(EventKind::TaskStarted, "start")
            .with_correlation(id)
            .with_data(serde_json::json!({"task": "demo"}));
        assert_eq!(e.correlation_id, Some(id));
        assert_eq!(e.data["task"], "demo");
        assert_eq!(e.level(), tracing::Level::INFO);
    }

    #[test]
    fn error_events_map_to_error_level() {
        assert_eq!(
            Event::new(EventKind::Error, "boom").level(),
            tracing::Level::ERROR
        );
        assert_eq!(
            Event::new(EventKind::ToolCompleted, "done").level(),
            tracing::Level::DEBUG
        );
    }

    #[test]
    fn events_serialise_without_null_data() {
        let v = serde_json::to_value(Event::new(EventKind::Warning, "hmm")).unwrap();
        assert!(v.get("data").is_none());
        assert_eq!(v["kind"], "warning");
    }
}