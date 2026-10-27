//! Shared protocol types for JARVIS.
//!
//! This crate contains **data only** — no behaviour, no I/O. Both the Rust core and
//! the Python brain are written against these shapes, so a change here is a
//! deliberate, visible contract change.

pub mod event;
pub mod message;
pub mod permission;
pub mod rpc;
pub mod tool;

pub use event::{Event, EventKind};
pub use message::{ChatMessage, Role, ToolCallRequest};
pub use permission::{AutonomyLevel, Decision, PermissionLevel, Risk};
pub use rpc::{RpcError, RpcRequest, RpcResponse, RpcResult};
pub use tool::{ToolDescriptor, ToolExecution, ToolResult};

/// Wire protocol version. Bumped when the JSON-RPC contract changes incompatibly.
pub const PROTOCOL_VERSION: &str = "1.0";