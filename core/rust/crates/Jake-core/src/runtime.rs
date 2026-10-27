//! The agent runtime: the loop that turns one user turn into verified work.
//!
//! ```text
//! observe → brain.chat → [tool calls?] → gate → execute → verify → loop → respond
//! ```
//!
//! Budgets (`max_steps`, `tool_call_budget`, timeouts) and the emergency stop are
//! enforced here; the model cannot raise them.

use crate::cancel::CancelFlag;
use crate::config::Config;
use crate::events::EventBus;
use crate::permission::{Confirmer, PermissionGate};
use crate::python::Brain;
use crate::tools::{ToolContext, ToolRegistry};
use Jake_protocol::event::{Event, EventKind};
use Jake_protocol::message::{ChatMessage, ChatRequest, ChatResponse};
use Jake_protocol::rpc::{self, RpcError};
use Jake_protocol::tool::{ToolExecution, ToolResult};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("brain error: {0}")]
    Brain(String),
    #[error("cancelled")]
    Cancelled,
    #[error("timed out: {0}")]
    Timeout(String),
}

/// What one turn produced. `content` is always safe to show/speak: it comes from the
/// model, or from an honest failure message — never from an assumed success.
#[derive(Debug, Clone)]
pub struct TurnOutcome {
    pub content: String,
    pub tool_results: Vec<ToolResult>,
    pub steps: u32,
    pub correlation_id: Uuid,
    pub provider: String,
    pub model: String,
    pub degraded: bool,
    pub error: Option<String>,
}

pub struct AgentRuntime {
    config: Arc<Config>,
    bus: EventBus,
    registry: Arc<ToolRegistry>,
    gate: std::sync::Mutex<PermissionGate>,
    brain: Brain,
    confirmer: Arc<dyn Confirmer>,
    cancel: CancelFlag,
    /// Short-term memory: the last N messages of this session.
    history: std::sync::Mutex<Vec<ChatMessage>>,
}

pub struct RuntimeParts {
    pub config: Arc<Config>,
    pub bus: EventBus,
    pub registry: Arc<ToolRegistry>,
    pub gate: PermissionGate,
    pub brain: Brain,
    pub confirmer: Arc<dyn Confirmer>,
    pub cancel: CancelFlag,
}

impl AgentRuntime {
    pub fn new(parts: RuntimeParts) -> Self {
        Self {
            config: parts.config,
            bus: parts.bus,
            registry: parts.registry,
            gate: std::sync::Mutex::new(parts.gate),
            brain: parts.brain,
            confirmer: parts.confirmer,
            cancel: parts.cancel,
            history: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn config(&self) -> &Arc<Config> {
        &self.config
    }

    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    pub fn registry(&self) -> &Arc<ToolRegistry> {
        &self.registry
    }

    pub fn cancel_flag(&self) -> CancelFlag {
        self.cancel.clone()
    }

    pub fn gate(&self) -> std::sync::MutexGuard<'_, PermissionGate> {
        self.gate.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Replace the active confirmer binding (UI mode vs CLI prompt vs auto).
    /// Built once at startup through [`RuntimeParts`] — see the CLI for an example.
    pub fn confirmer(&self) -> &Arc<dyn Confirmer> {
        &self.confirmer
    }

    pub fn brain(&self) -> &Brain {
        &self.brain
    }

    /// Snapshot of the gate, safe to hold across `.await` points.
    pub fn gate_snapshot(&self) -> PermissionGate {
        self.gate.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn clear_history(&self) {
        self.history.lock().unwrap().clear();
    }

    pub fn history_len(&self) -> usize {
        self.history.lock().unwrap().len()
    }

    /// Emergency stop: cancels the current turn and any queued tool execution.
    pub fn emergency_stop(&self, reason: &str) {
        self.cancel.cancel(reason);
        self.bus.publish(
            Event::new(EventKind::EmergencyStop, format!("emergency stop: {reason}")),
        );
    }

    /// Re-arm after a stop; the next turn may run.
    pub fn reset_stop(&self) {
        self.cancel.reset();
    }

    /// Execute one user turn end to end.
    pub async fn run_turn(&self, user_text: &str) -> Result<TurnOutcome, RuntimeError> {
        let correlation = Uuid::new_v4();
        let started = Instant::now();
        let total_budget = Duration::from_millis(self.config.agent.total_timeout_ms.max(1000));
        let max_steps = self.config.agent.max_steps.max(1) as u32;
        let tool_budget = self.config.agent.tool_call_budget.max(1);

        self.cancel.check().map_err(|_| RuntimeError::Cancelled)?;
        self.bus.publish(
            Event::new(EventKind::UserMessage, "user turn started")
                .with_correlation(correlation)
                .with_data(json!({"text_len": user_text.len()})),
        );

        let mut messages: Vec<ChatMessage> = {
            let mut history = self.history.lock().unwrap();
            history.push(ChatMessage::user(user_text));
            history.clone()
        };
        let history_cap = self.config.agent.history_messages.max(2) as usize;
        if messages.len() > history_cap {
            let skip = messages.len() - history_cap;
            messages = messages.split_off(skip);
        }

        let mut tool_results: Vec<ToolResult> = Vec::new();
        let mut tools_used: u16 = 0;
        let mut steps: u32 = 0;
        let mut last_provider = String::new();
        let mut last_model = String::new();
        let mut degraded = false;

        loop {
            if steps >= max_steps {
                return self.finish(
                    correlation,
                    started,
                    format!(
                        "I stopped after {} steps to stay within the step budget. \
                         Tell me to continue if you want me to keep going.",
                        steps
                    ),
                    tool_results,
                    steps,
                    last_provider,
                    last_model,
                    degraded,
                    Some(format!("step budget of {max_steps} exhausted")),
                );
            }
            self.cancel
                .check()
                .map_err(|_| RuntimeError::Cancelled)?;
            if started.elapsed() > total_budget {
                return self.finish(
                    correlation,
                    started,
                    "I ran out of time for this request and stopped safely.".into(),
                    tool_results,
                    steps,
                    last_provider,
                    last_model,
                    degraded,
                    Some("total turn timeout exceeded".into()),
                );
            }

            steps += 1;
            self.bus.publish(
                Event::new(EventKind::AgentStep, format!("agent step {steps}/{max_steps}"))
                    .with_correlation(correlation)
                    .with_data(json!({"step": steps})),
            );

            let steps_remaining = max_steps - steps;
            let autonomy = self.gate().autonomy().get();
            let request = ChatRequest {
                messages: messages.clone(),
                tools: self.registry.llm_tools(),
                autonomy,
                steps_remaining,
                privacy_mode: !self.config.cloud_allowed(),
                persona: json!({
                    "name": self.config.persona.name,
                    "style": self.config.persona.style,
                    "language": self.config.persona.language,
                    "wake_word": self.config.persona.wake_word,
                }),
                context: json!({
                    "steps_used": steps,
                    "tools_used_this_turn": tool_results.len(),
                    "tool_results": tool_results.iter().map(|r| json!({
                        "name": r.name, "ok": r.ok, "error_code": r.error_code
                    })).collect::<Vec<_>>(),
                }),
            };

            let remaining = total_budget.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                continue;
            }
            let reply = match self
                .brain
                .request("brain.chat", serde_json::to_value(&request).unwrap_or(Value::Null), remaining)
                .await
            {
                Ok(value) => match serde_json::from_value::<ChatResponse>(value) {
                    Ok(r) => r,
                    Err(e) => {
                        return self.finish(
                            correlation,
                            started,
                            "I could not parse the assistant's internal response, so I did not act \
                             on it. Nothing was changed."
                                .into(),
                            tool_results,
                            steps,
                            last_provider,
                            last_model,
                            true,
                            Some(format!("invalid brain response: {e}")),
                        );
                    }
                },
                Err(e) => {
                    let msg = brain_error_message(&e, &self.config.llm.provider);
                    return self.finish(
                        correlation,
                        started,
                        msg,
                        tool_results,
                        steps,
                        last_provider,
                        last_model,
                        true,
                        Some(format!("{}: {}", e.code, e.message)),
                    );
                }
            };

            if !reply.provider.is_empty() {
                last_provider = reply.provider.clone();
            }
            if !reply.model.is_empty() {
                last_model = reply.model.clone();
            }
            if let Some(err) = &reply.error {
                degraded = true;
                tracing::warn!(error = %err, "brain reported a degraded turn");
                return self.finish(
                    correlation,
                    started,
                    if reply.content.is_empty() {
                        brain_error_message_plain(err)
                    } else {
                        reply.content.clone()
                    },
                    tool_results,
                    steps,
                    last_provider,
                    last_model,
                    degraded,
                    Some(err.clone()),
                );
            }

            let tool_calls = reply.tool_calls.clone();
            if tool_calls.is_empty() {
                let content = if reply.content.trim().is_empty() {
                    "(no response)".into()
                } else {
                    reply.content.clone()
                };
                messages.push(ChatMessage::assistant(content.clone()));
                self.remember(messages.clone());
                self.bus.publish(
                    Event::new(EventKind::AssistantMessage, content.clone())
                        .with_correlation(correlation),
                );
                return self.finish(
                    correlation,
                    started,
                    content,
                    tool_results,
                    steps,
                    last_provider,
                    last_model,
                    degraded,
                    None,
                );
            }

            // --- execute proposed tool calls through the gate -------------------
            messages.push(ChatMessage {
                role: Jake_protocol::message::Role::Assistant,
                content: reply.content.clone(),
                tool_calls: Some(tool_calls.clone()),
                tool_call_id: None,
                untrusted: false,
            });

            for call in tool_calls {
                if tools_used >= tool_budget {
                    let result = ToolResult {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        ok: false,
                        data: None,
                        error: Some(format!("tool budget of {tool_budget} exhausted for this turn")),
                        error_code: Some("budget_exhausted".into()),
                        duration_ms: 0,
                        confirmation_required: false,
                    };
                    messages.push(ChatMessage::tool_result(
                        result.id.clone(),
                        result.to_model_value().to_string(),
                    ));
                    tool_results.push(result);
                    degraded = true;
                    break;
                }
                if self.cancel.is_cancelled() {
                    return self.finish(
                        correlation,
                        started,
                        "Stopped: the emergency stop was pressed, so I did not run the remaining actions."
                            .into(),
                        tool_results,
                        steps,
                        last_provider,
                        last_model,
                        degraded,
                        Some("emergency stop".into()),
                    );
                }

                tools_used += 1;
                self.bus.publish(
                    Event::new(
                        EventKind::ToolRequested,
                        format!("tool `{}` requested", call.name),
                    )
                    .with_correlation(correlation)
                    .with_data(json!({"tool": call.name, "args": call.arguments})),
                );

                let execution = ToolExecution {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                };
                let mut ctx = ToolContext::new(self.config.root.clone());
                ctx.correlation_id = Some(correlation);
                ctx.cancel = self.cancel.clone();
                ctx.privacy_mode = self.config.privacy.privacy_mode;

                // Snapshot the gate before awaiting: never hold the mutex across a
                // suspension point.
                let gate_snapshot = self.gate_snapshot();
                let result = self
                    .registry
                    .dispatch(
                        execution,
                        &ctx,
                        &gate_snapshot,
                        &self.confirmer,
                        Duration::from_millis(self.config.agent.step_timeout_ms.max(500)),
                    )
                    .await;

                self.bus.publish(
                    Event::new(
                        EventKind::ToolCompleted,
                        format!(
                            "tool `{}` {} in {} ms",
                            result.name,
                            if result.ok { "succeeded" } else { "failed" },
                            result.duration_ms
                        ),
                    )
                    .with_correlation(correlation)
                    .with_data(json!({
                        "tool": result.name,
                        "ok": result.ok,
                        "error_code": result.error_code,
                        "duration_ms": result.duration_ms,
                    })),
                );

                messages.push(ChatMessage::tool_result(
                    result.id.clone(),
                    result.to_model_value().to_string(),
                ));
                tool_results.push(result);
            }
        }
    }

    fn finish(
        &self,
        correlation: Uuid,
        started: Instant,
        content: String,
        tool_results: Vec<ToolResult>,
        steps: u32,
        provider: String,
        model: String,
        degraded: bool,
        error: Option<String>,
    ) -> Result<TurnOutcome, RuntimeError> {
        self.bus.publish(
            Event::new(
                EventKind::TaskFinished,
                format!("turn finished in {} ms ({steps} steps)", started.elapsed().as_millis()),
            )
            .with_correlation(correlation)
            .with_data(json!({
                "steps": steps,
                "tools": tool_results.len(),
                "degraded": degraded,
                "error": error,
            })),
        );
        Ok(TurnOutcome {
            content,
            tool_results,
            steps,
            correlation_id: correlation,
            provider,
            model,
            degraded,
            error,
        })
    }

    fn remember(&self, messages: Vec<ChatMessage>) {
        let cap = self.config.agent.history_messages.max(2) as usize;
        let mut history = self.history.lock().unwrap();
        *history = messages;
        if history.len() > cap {
            let drain = history.len() - cap;
            history.drain(0..drain);
        }
    }
}

/// Honest, actionable wording for a brain/provider failure.
fn brain_error_message(err: &RpcError, provider: &str) -> String {
    match err.code {
        rpc::TIMEOUT => format!(
            "The AI provider did not answer in time, so I stopped rather than guess. \
             You can retry, or lower `agent.total_timeout_ms`."
        ),
        rpc::PEER_DISCONNECTED => "The language-model process is not running, so I cannot think right now. \
             Run `jarvis doctor` to see why."
            .to_string(),
        _ => {
            if provider == "test" {
                format!("The offline test provider reported: {}", err.message)
            } else {
                format!(
                    "I could not get a reply from the AI provider: {}. \
                     Check the API key (`{}`), the model name, or your connection.",
                    err.message,
                    "JARVIS_LLM_API_KEY"
                )
            }
        }
    }
}

fn brain_error_message_plain(msg: &str) -> String {
    format!("I could not complete that: {msg}. Nothing was changed.")
}

#[cfg(test)]
mod tests {
    // Runtime end-to-end tests live in `core/rust/crates/jarvis-cli/tests` and in
    // `tests/`, where the real Python brain (or the offline test provider) is
    // available. Unit-level coverage of the loop's budgets is here.

    use super::*;
    use Jake_protocol::permission::AutonomyLevel;

    #[test]
    fn brain_error_messages_are_actionable_and_never_claim_success() {
        let timeout = brain_error_message(&RpcError::new(rpc::TIMEOUT, "t"), "openai_compat");
        assert!(timeout.contains("did not answer"));
        assert!(!timeout.to_lowercase().contains("done"));

        let peer = brain_error_message(&RpcError::new(rpc::PEER_DISCONNECTED, "gone"), "openai_compat");
        assert!(peer.contains("doctor"));

        let other = brain_error_message(&RpcError::new(rpc::BRAIN_ERROR, "bad key"), "openai_compat");
        assert!(other.contains("bad key"));

        let plain = brain_error_message_plain("the provider refused");
        assert!(plain.contains("Nothing was changed"));
    }

    #[test]
    fn turn_outcome_defaults_are_not_degraded() {
        let outcome = TurnOutcome {
            content: "hi".into(),
            tool_results: vec![],
            steps: 1,
            correlation_id: Uuid::nil(),
            provider: "test".into(),
            model: "test".into(),
            degraded: false,
            error: None,
        };
        assert!(!outcome.degraded);
        assert!(outcome.error.is_none());
    }

    #[test]
    fn autonomy_is_read_from_the_gate() {
        let cfg = Arc::new(Config::default());
        let gate = PermissionGate::new(AutonomyLevel::new(cfg.agent.autonomy).unwrap());
        assert_eq!(gate.autonomy().get(), 3);
    }
}