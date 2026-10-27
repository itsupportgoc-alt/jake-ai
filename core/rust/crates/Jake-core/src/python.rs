//! Supervised JSON-RPC 2.0 bridge to the Python brain process.
//!
//! * newline-delimited JSON-RPC over the child's stdio — no ports, no sockets
//! * the child is restarted on crash (up to `restart_limit`), and pending requests
//!   fail **loudly** with `PEER_DISCONNECTED` rather than hanging forever
//! * the brain may call back into the runtime through [`RpcHandler`]
//!
//! The brain is deliberately *less* privileged than this process: it can request
//! tool executions, which still pass the permission gate here.

use Jake_protocol::rpc::{self, Frame, RpcError, RpcRequest, RpcResponse};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command as TokioCommand};
use tokio::sync::{mpsc, oneshot, Notify};

/// Handles requests *from* the brain (e.g. `tool.execute`, `memory.put`).
pub trait RpcHandler: Send + Sync + 'static {
    fn handle(
        &self,
        method: String,
        params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send>>;
}

/// A handler that answers every method with `method not found`.
pub struct NullHandler;

impl RpcHandler for NullHandler {
    fn handle(&self, method: String, _params: Value) -> Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send>> {
        Box::pin(async move {
            Err(RpcError::new(
                rpc::METHOD_NOT_FOUND,
                format!("method `{method}` is not handled by this runtime"),
            ))
        })
    }
}

/// How to start the brain process.
#[derive(Debug, Clone)]
pub struct BrainSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub restart_limit: u8,
    pub default_timeout: Duration,
}

enum BrainCmd {
    Request {
        id: i64,
        method: String,
        params: Value,
        reply: oneshot::Sender<Result<Value, RpcError>>,
    },
    Notify {
        method: String,
        params: Value,
    },
    Shutdown,
}

#[derive(Clone)]
pub struct Brain {
    cmd_tx: mpsc::Sender<BrainCmd>,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>>,
    next_id: Arc<AtomicI64>,
    /// Set when the supervisor gave up; requests fail immediately afterwards.
    dead: Arc<tokio::sync::watch::Sender<bool>>,
}

impl Brain {
    /// Start the supervisor. The child is spawned lazily on the first request so a
    /// config-only startup never launches a process we do not need.
    pub fn spawn(spec: BrainSpec, handler: Arc<dyn RpcHandler>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<BrainCmd>(256);
        let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let next_id = Arc::new(AtomicI64::new(1));
        let (dead_tx, dead_rx) = tokio::sync::watch::channel(false);
        let dead_tx = Arc::new(dead_tx);

        let brain = Self {
            cmd_tx: cmd_tx.clone(),
            pending: pending.clone(),
            next_id: next_id.clone(),
            dead: dead_tx.clone(),
        };

        tokio::spawn(supervise(spec, handler, cmd_rx, pending, dead_tx));
        let _ = dead_rx;
        brain
    }

    /// Request/response call with a hard timeout.
    pub async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, RpcError> {
        if *self.dead.borrow() {
            return Err(RpcError::new(
                rpc::PEER_DISCONNECTED,
                "the brain process could not be started or has failed repeatedly",
            ));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(BrainCmd::Request {
                id,
                method: method.to_string(),
                params,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RpcError::new(rpc::PEER_DISCONNECTED, "brain supervisor is gone"))?;

        match tokio::time::timeout(timeout, reply_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(RpcError::new(rpc::PEER_DISCONNECTED, "brain dropped the request")),                Err(_) => {
                    let dropped = self.pending.lock().unwrap().remove(&id);
                    drop(dropped);
                    Err(RpcError::new(
                    rpc::TIMEOUT,
                    format!("`{method}` timed out after {:?}", timeout),
                ))
            }
        }
    }

    /// Fire-and-forget notification (log lines, telemetry).
    pub async fn notify(&self, method: &str, params: Value) {
        let _ = self
            .cmd_tx
            .send(BrainCmd::Notify {
                method: method.to_string(),
                params,
            })
            .await;
    }

    pub async fn shutdown(&self) {
        let _ = self.cmd_tx.send(BrainCmd::Shutdown).await;
    }

    /// Best-effort readiness signal (used by `jarvis doctor`).
    pub async fn ping(&self, timeout: Duration) -> Result<Value, RpcError> {
        self.request("brain.health", serde_json::json!({}), timeout).await
    }
}

async fn supervise(
    spec: BrainSpec,
    handler: Arc<dyn RpcHandler>,
    mut cmd_rx: mpsc::Receiver<BrainCmd>,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>>,
    dead_tx: Arc<tokio::sync::watch::Sender<bool>>,
) {
    let mut restarts = 0u8;
    let mut failed_permanently = false;

    'outer: loop {
        // --- spawn ---------------------------------------------------------
        let child = if failed_permanently {
            None
        } else {
            match launch(&spec) {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::error!(error = %e, "cannot start brain process");
                    restarts += 1;
                    if restarts > spec.restart_limit {
                        tracing::error!(restarts, "brain restart limit reached; giving up");
                        failed_permanently = true;
                        None
                    } else {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        continue 'outer;
                    }
                }
            }
        };

        let Some(mut child) = child else {
            // Permanently failed: answer every future request honestly.
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    BrainCmd::Request { id: _, method, params: _, reply } => {
                        let _ = reply.send(Err(RpcError::new(
                            rpc::PEER_DISCONNECTED,
                            "brain process unavailable (start-up failed repeatedly)",
                        )));
                        let _ = method;
                    }
                    BrainCmd::Notify { .. } => {}
                    BrainCmd::Shutdown => break 'outer,
                }
            }
            break;
        };

        let mut stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");
        let (line_tx, mut line_rx) = mpsc::channel::<String>(256);
        let death = Arc::new(Notify::new());

        // writer: owns stdin
        let writer = {
            let death = death.clone();
            tokio::spawn(async move {
                while let Some(line) = line_rx.recv().await {
                    if stdin.write_all(line.as_bytes()).await.is_err()
                        || stdin.write_all(b"\n").await.is_err()
                    {
                        break;
                    }
                    let _ = stdin.flush().await;
                }
                let _ = stdin.shutdown().await;
                // notify_one stores a permit, so a death that happens before the
                // session loop starts selecting is never lost.
                death.notify_one();
            })
        };

        // reader: resolves responses, dispatches incoming requests
        let reader_pending = pending.clone();
        let reader_handler = handler.clone();
        let reader_tx = line_tx.clone();
        let reader_death = death.clone();
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        match rpc::parse_frame(&line) {
                            Frame::Response(resp) => {
                                let id = resp.id.as_i64().or_else(|| resp.id.as_f64().map(|f| f as i64));
                                let outcome = resp.into_result();
                                if let Some(id) = id {
                                    let slot = reader_pending.lock().unwrap().remove(&id);
                                    match slot {
                                        Some(tx) => {
                                            let _ = tx.send(outcome);
                                        }
                                        None => {
                                            tracing::debug!(id, "response for an expired request");
                                        }
                                    }
                                }
                            }
                            Frame::Request(req) => {
                                let handler = reader_handler.clone();
                                let tx = reader_tx.clone();
                                let is_notification = req.is_notification();
                                let id = req.id.clone();
                                let method = req.method.clone();
                                let params = req.params.clone().unwrap_or(Value::Null);
                                tokio::spawn(async move {
                                    let result = handler.handle(method.clone(), params).await;
                                    if is_notification {
                                        return;
                                    }
                                    let frame = match result {
                                        Ok(v) => RpcResponse::success(id.unwrap_or(Value::Null), v),
                                        Err(e) => RpcResponse::failure(id.unwrap_or(Value::Null), e),
                                    };
                                    match serde_json::to_string(&frame) {
                                        Ok(line) => {
                                            let _ = tx.send(line).await;
                                        }
                                        Err(e) => {
                                            tracing::error!(error = %e, "cannot serialise response to brain");
                                        }
                                    }
                                });
                            }
                            Frame::Invalid(reason) => {
                                tracing::warn!(%reason, "malformed frame from brain");
                            }
                        }
                    }
                    Ok(None) => break, // EOF: child exited
                    Err(e) => {
                        tracing::warn!(error = %e, "stdout read error from brain");
                        break;
                    }
                }
            }
            reader_death.notify_one();
        });

        // reap the child (and kill it on request)
        let (kill_tx, mut kill_rx) = tokio::sync::watch::channel(false);
        let reap = {
            let death = death.clone();
            tokio::spawn(async move {
                let mut child = child;
                tokio::select! {
                    status = child.wait() => {
                        tracing::debug!(?status, "brain process exited");
                    }
                    _ = kill_rx.changed() => {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        tracing::debug!("brain process killed by runtime");
                    }
                }
                death.notify_one();
            })
        };

        // --- session loop --------------------------------------------------
        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => {
                    match cmd {
                        None | Some(BrainCmd::Shutdown) => {
                            let _ = kill_tx.send(true);
                            let _ = reap.abort();
                            let _ = writer.abort();
                            let _ = reader.abort();
                            drain_pending(&pending, "runtime shut down");
                            break 'outer;
                        }
                        Some(BrainCmd::Request { id, method, params, reply }) => {
                            if failed_permanently {
                                let _ = reply.send(Err(RpcError::new(
                                    rpc::PEER_DISCONNECTED,
                                    "brain process unavailable",
                                )));
                                continue;
                            }
                            pending.lock().unwrap().insert(id, reply);
                            let frame = RpcRequest::new(Value::from(id), method, params);
                            match serde_json::to_string(&frame) {
                                Ok(line) => {
                                    if line_tx.send(line).await.is_err() {
                                        drain_pending(&pending, "brain stdin closed");
                                    }
                                }
                                Err(e) => {
                                    if let Some(tx) = pending.lock().unwrap().remove(&id) {
                                        let _ = tx.send(Err(RpcError::new(rpc::INTERNAL_ERROR, e.to_string())));
                                    }
                                }
                            }
                        }
                        Some(BrainCmd::Notify { method, params }) => {
                            let frame = RpcRequest::notification(method, params);
                            if let Ok(line) = serde_json::to_string(&frame) {
                                let _ = line_tx.send(line).await;
                            }
                        }
                    }
                }
                _ = death.notified() => {
                    tracing::warn!("brain session ended; pending requests will fail");
                    break;
                }
            }
        }

        let _ = kill_tx.send(true);
        let _ = reap.abort();
        let _ = writer.abort();
        let _ = reader.abort();
        drain_pending(&pending, "brain process terminated");

        if failed_permanently {
            break;
        }
        restarts += 1;
        if restarts > spec.restart_limit {
            tracing::error!(
                restarts,
                limit = spec.restart_limit,
                "brain failed repeatedly — disabled for this session"
            );
            failed_permanently = true;
        }
    }
    let _ = dead_tx.send_replace(true);
    drain_pending(&pending, "brain supervisor stopped");
}

fn drain_pending(
    pending: &Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>>,
    reason: &str,
) {
    let drained: Vec<_> = {
        let mut map = pending.lock().unwrap();
        map.drain().collect()
    };
    for (_, tx) in drained {
        let _ = tx.send(Err(RpcError::new(rpc::PEER_DISCONNECTED, reason)));
    }
}

fn launch(spec: &BrainSpec) -> std::io::Result<Child> {
    let mut cmd = TokioCommand::new(&spec.program);
    cmd.args(&spec.args)
        .envs(spec.env.iter().cloned())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW: never flash a console window for the brain.
        cmd.creation_flags(0x0800_0000);
    }
    cmd.spawn()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    /// A fake "brain": answers health, echoes, dies on demand, and can call back.
    struct FakeBrain;

    impl RpcHandler for FakeBrain {
        fn handle(
            &self,
            method: String,
            params: Value,
        ) -> Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send>> {
            Box::pin(async move {
                match method.as_str() {
                    "brain.health" => Ok(serde_json::json!({"status": "ok", "version": "fake-1"}),
                    ),
                    "echo" => Ok(params),
                    "sleep" => {
                        let ms = params.get("ms").and_then(Value::as_u64).unwrap_or(50);
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                        Ok(serde_json::json!({"slept_ms": ms}))
                    }
                    "fail" => Err(RpcError::new(rpc::BRAIN_ERROR, "intentional")),
                    other => Err(RpcError::new(rpc::METHOD_NOT_FOUND, other.to_string())),
                }
            })
        }
    }

    /// Minimal python-free child: reads JSON-RPC lines and replies, for bridge tests.
    fn echo_script() -> String {
        r#"import sys, json
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    req = json.loads(line)
    if req.get("method") == "brain.health":
        resp = {"jsonrpc":"2.0","id":req.get("id"),"result":{"status":"ok","provider":"fake"}}
    elif req.get("method") == "echo":
        resp = {"jsonrpc":"2.0","id":req.get("id"),"result":req.get("params")}
    elif req.get("method") == "boom":
        sys.exit(3)
    else:
        resp = {"jsonrpc":"2.0","id":req.get("id"),"error":{"code":-32601,"message":"nope"}}
    sys.stdout.write(json.dumps(resp)+"\n"); sys.stdout.flush()
"#
        .to_string()
    }

    fn spec_with(program: String, args: Vec<String>) -> BrainSpec {
        BrainSpec {
            program,
            args,
            env: vec![],
            restart_limit: 2,
            default_timeout: Duration::from_secs(5),
        }
    }

    fn python() -> String {
        std::env::var("JARVIS_TEST_PYTHON").unwrap_or_else(|_| "python".into())
    }

    #[tokio::test]
    async fn round_trip_over_stdio() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake.py");
        std::fs::write(&script, echo_script()).unwrap();

        let brain = Brain::spawn(
            spec_with(python(), vec![script.display().to_string()]),
            Arc::new(FakeBrain),
        );
        let res = brain
            .request("brain.health", serde_json::json!({}), Duration::from_secs(10))
            .await
            .expect("health should succeed");
        assert_eq!(res["status"], "ok");

        let echo = brain
            .request("echo", serde_json::json!({"a": 1}), Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(echo["a"], 1);

        let unknown = brain
            .request("nope", serde_json::json!({}), Duration::from_secs(5))
            .await;
        assert_eq!(unwrap_err(unknown).code, rpc::METHOD_NOT_FOUND);

        brain.shutdown().await;
    }

    #[tokio::test]
    async fn request_timeout_is_reported_not_hung() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("slow.py");
        // A child that accepts the request but never answers.
        std::fs::write(
            &script,
            "import sys, json, time\nfor line in sys.stdin:\n    time.sleep(30)\n",
        )
        .unwrap();
        let brain = Brain::spawn(
            spec_with(python(), vec![script.display().to_string()]),
            Arc::new(FakeBrain),
        );
        let started = std::time::Instant::now();
        let err = brain
            .request("brain.chat", serde_json::json!({}), Duration::from_millis(400))
            .await
            .expect_err("must time out");
        assert_eq!(err.code, rpc::TIMEOUT);
        assert!(started.elapsed() < Duration::from_secs(5));
        brain.shutdown().await;
    }

    #[tokio::test]
    async fn crashing_child_fails_requests_loudly_then_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("crash.py");
        std::fs::write(&script, echo_script()).unwrap();

        let brain = Brain::spawn(
            spec_with(python(), vec![script.display().to_string()]),
            Arc::new(FakeBrain),
        );
        // First call establishes the session.
        brain
            .request("brain.health", serde_json::json!({}), Duration::from_secs(10))
            .await
            .unwrap();
        // This one kills the child mid-request.
        let err = brain
            .request("boom", serde_json::json!({}), Duration::from_secs(10))
            .await
            .expect_err("child died");
        assert_eq!(err.code, rpc::PEER_DISCONNECTED);

        // Supervisor restarts: next call succeeds again.
        let again = brain
            .request("brain.health", serde_json::json!({}), Duration::from_secs(10))
            .await
            .expect("should recover after restart");
        assert_eq!(again["status"], "ok");
        brain.shutdown().await;
    }

    #[tokio::test]
    async fn missing_program_yields_honest_error() {
        let brain = Brain::spawn(
            spec_with("definitely-not-a-real-binary-jarvis".into(), vec![]),
            Arc::new(FakeBrain),
        );
        let err = brain
            .request("brain.health", serde_json::json!({}), Duration::from_secs(5))
            .await
            .expect_err("must fail");
        assert_eq!(err.code, rpc::PEER_DISCONNECTED);
        brain.shutdown().await;
    }

    #[tokio::test]
    async fn inbound_requests_reach_the_handler_and_answer() {
        use Jake_protocol::rpc::RpcRequest as Req;
        static CALLS: AtomicU32 = AtomicU32::new(0);
        struct Counting;
        impl RpcHandler for Counting {
            fn handle(
                &self,
                method: String,
                _params: Value,
            ) -> Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send>> {
                Box::pin(async move {
                    CALLS.fetch_add(1, Ordering::SeqCst);
                    Ok(serde_json::json!({"method": method}))
                })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("caller.py");
        // Child sends one request to us, then answers health forever.
        std::fs::write(
            &script,
            r#"import sys, json
req = {"jsonrpc":"2.0","id":99,"method":"tool.list","params":{}}
sys.stdout.write(json.dumps(req)+"\n"); sys.stdout.flush()
for line in sys.stdin:
    line=line.strip()
    if not line: continue
    r=json.loads(line)
    if r.get("method")=="brain.health":
        sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":r["id"],"result":{"status":"ok"}})+"\n"); sys.stdout.flush()
"#,
        )
        .unwrap();

        let brain = Brain::spawn(
            spec_with(python(), vec![script.display().to_string()]),
            Arc::new(Counting),
        );
        brain
            .request("brain.health", serde_json::json!({}), Duration::from_secs(10))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(CALLS.load(Ordering::SeqCst) >= 1, "inbound request never handled");
        let _ = Req::notification("noop", Value::Null);
        brain.shutdown().await;
    }

    fn unwrap_err(r: Result<Value, RpcError>) -> RpcError {
        match r {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        }
    }
}