//! `jarvis-core` — the trusted computing base of the JARVIS assistant.
//!
//! Responsibilities: configuration, structured secret-safe logging, the event bus,
//! the permission gate, the tool registry and dispatcher, the supervised bridge to
//! the Python brain, and the agent runtime that ties them together.
//!
//! Nothing in this crate trusts the model: every side effect goes through
//! [`ToolRegistry::dispatch`], which is the only path to the operating system.

pub mod builtin;
pub mod builtin_ext;
pub mod builtin_memory;
pub mod builtin_ui;
pub mod bridge;
pub mod cancel;
pub mod config;
pub mod events;
pub mod logging;
pub mod permission;
pub mod python;
pub mod runtime;
pub mod tools;

pub use cancel::{CancelFlag, Cancelled};
pub use bridge::CoreRpcHandler;
pub use config::{Config, ConfigError, Secret};
pub use events::EventBus;
pub use permission::{AutonomyLevel, Confirmer, Decision, NeverConfirm, PermissionGate};
pub use python::{Brain, BrainSpec, NullHandler, RpcHandler};
pub use tools::{Tool, ToolContext, ToolError, ToolRegistry};
pub use jarvis_protocol::PROTOCOL_VERSION;