//! JSON-RPC 2.0 message frames (newline-delimited over stdio).
//!
//! Specification: <https://www.jsonrpc.org/specification>. Error codes follow the
//! spec; JARVIS-specific codes live in the `-32000..-32099` range.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

/// Brain refused / could not produce a usable reply (provider error, bad output).
pub const BRAIN_ERROR: i64 = -32001;
/// Execution exceeded its timeout.
pub const TIMEOUT: i64 = -32002;
/// Permission gate denied the operation.
pub const PERMISSION_DENIED: i64 = -32003;
/// Cancelled by the user or the emergency stop.
pub const CANCELLED: i64 = -32004;
/// The peer process died.
pub const PEER_DISCONNECTED: i64 = -32005;

/// Standard result alias for RPC calls.
pub type RpcResult<T> = Result<T, RpcError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    /// Absent for notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl RpcRequest {
    pub fn new(id: Value, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: Some(id),
            method: method.into(),
            params: Some(params),
        }
    }

    pub fn notification(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: None,
            method: method.into(),
            params: Some(params),
        }
    }

    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(id: Value, error: RpcError) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(error),
        }
    }

    /// Extract the result, turning a peer error into a `serde_json::Error`-free enum.
    pub fn into_result(self) -> Result<Value, RpcError> {
        match (self.result, self.error) {
            (Some(r), None) => Ok(r),
            (None, Some(e)) => Err(e),
            _ => Err(RpcError::new(INVALID_REQUEST, "response has neither result nor error")),
        }
    }
}

/// Tolerant parse of one incoming line: distinguishes requests, responses and garbage.
#[derive(Debug, Clone)]
pub enum Frame {
    Request(RpcRequest),
    Response(RpcResponse),
    Invalid(String),
}

pub fn parse_frame(line: &str) -> Frame {
    let line = line.trim();
    if line.is_empty() {
        return Frame::Invalid("empty frame".into());
    }
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Frame::Invalid(format!("not JSON: {e}")),
    };
    if value.get("method").is_some() {
        match serde_json::from_value::<RpcRequest>(value) {
            Ok(r) => Frame::Request(r),
            Err(e) => Frame::Invalid(format!("bad request frame: {e}")),
        }
    } else if value.get("id").is_some() && (value.get("result").is_some() || value.get("error").is_some()) {
        match serde_json::from_value::<RpcResponse>(value) {
            Ok(r) => Frame::Response(r),
            Err(e) => Frame::Invalid(format!("bad response frame: {e}")),
        }
    } else {
        Frame::Invalid("frame is neither a request nor a response".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_request_and_response() {
        let req = parse_frame(r#"{"jsonrpc":"2.0","id":1,"method":"brain.chat","params":{}}"#);
        assert!(matches!(req, Frame::Request(r) if r.method == "brain.chat"));

        let res = parse_frame(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#);
        match res {
            Frame::Response(r) => assert_eq!(r.into_result().unwrap()["ok"], true),
            _ => panic!("expected response"),
        }
    }

    #[test]
    fn parses_notification() {
        let n = parse_frame(r#"{"jsonrpc":"2.0","method":"log.event","params":{}}"#);
        match n {
            Frame::Request(r) => assert!(r.is_notification()),
            _ => panic!("expected request"),
        }
    }

    #[test]
    fn garbage_is_invalid_not_a_panic() {
        assert!(matches!(parse_frame("hello"), Frame::Invalid(_)));
        assert!(matches!(parse_frame(""), Frame::Invalid(_)));
        assert!(matches!(parse_frame(r#"{"jsonrpc":"2.0"}"#), Frame::Invalid(_)));
    }

    #[test]
    fn round_trips_through_json() {
        let req = RpcRequest::new(json!(7), "tool.list", json!({}));
        let line = serde_json::to_string(&req).unwrap();
        match parse_frame(&line) {
            Frame::Request(r) => {
                assert_eq!(r.id.unwrap(), json!(7));
                assert_eq!(r.params.unwrap(), json!({}));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn error_response_carries_code() {
        let resp = RpcResponse::failure(json!(3), RpcError::new(PERMISSION_DENIED, "no"));
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["error"]["code"], -32003);
    }
}