//! The permission gate: the single place where a proposed action is allowed,
//! paused for confirmation, or refused.
//!
//! Rules are documented in `docs/permissions.md` and enforced here.

pub use jarvis_protocol::permission::*;

use jarvis_protocol::tool::ToolDescriptor;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Payload handed to the UI (or the CLI prompt) when the gate needs an answer.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmationRequest {
    pub tool: String,
    /// Pretty-printed arguments, truncated for display.
    pub arguments: String,
    pub level: PermissionLevel,
    pub risk: Risk,
    /// Why the gate asked (autonomy level, destructive flag, critical class).
    pub reason: String,
    /// Tool-authored explanation of what will happen.
    pub explanation: String,
}

pub type ConfirmationFuture = Pin<Box<dyn Future<Output = bool> + Send>>;

/// Anything that can ask the user "may I?" — the CLI prompt, the desktop UI, or an
/// automated test double. Implementations must treat a timeout/EOF as *decline*.
pub trait Confirmer: Send + Sync + 'static {
    fn confirm(&self, request: ConfirmationRequest) -> ConfirmationFuture;
}

/// Declines everything; used when no interactive channel is available.
pub struct NeverConfirm;

impl Confirmer for NeverConfirm {
    fn confirm(&self, _request: ConfirmationRequest) -> ConfirmationFuture {
        Box::pin(async { false })
    }
}

/// Accepts everything (tests only).
pub struct AlwaysConfirm;

impl Confirmer for AlwaysConfirm {
    fn confirm(&self, _request: ConfirmationRequest) -> ConfirmationFuture {
        Box::pin(async { true })
    }
}

/// Holds the autonomy level and the user's trusted rules.
/// Clonable so the runtime can snapshot it before an `.await` — a std mutex guard
/// must never be held across an await point.
#[derive(Debug, Clone)]
pub struct PermissionGate {
    autonomy: AutonomyLevel,
    rules: Vec<TrustedRule>,
}

impl PermissionGate {
    pub fn new(autonomy: AutonomyLevel) -> Self {
        Self {
            autonomy,
            rules: Vec::new(),
        }
    }

    pub fn autonomy(&self) -> AutonomyLevel {
        self.autonomy
    }

    pub fn set_autonomy(&mut self, level: AutonomyLevel) {
        self.autonomy = level;
    }

    pub fn rules(&self) -> &[TrustedRule] {
        &self.rules
    }

    pub fn add_rule(&mut self, rule: TrustedRule) {
        self.rules.push(rule);
    }

    /// Returns `false` when no such rule existed.
    pub fn remove_rule(&mut self, id: &str) -> bool {
        let before = self.rules.len();
        self.rules.retain(|r| r.id != id);
        self.rules.len() != before
    }

    /// Core decision function. Pure: same inputs → same decision (rule expiry uses
    /// the supplied `now`, so pass a fresh timestamp in production).
    pub fn evaluate(&self, tool: &ToolDescriptor, args: &Value) -> Decision {
        self.evaluate_inner(tool, args, &chrono::Utc::now().to_rfc3339())
    }

    pub fn evaluate_inner(&self, tool: &ToolDescriptor, args: &Value, now: &str) -> Decision {
        let autonomy = self.autonomy.get();
        let level = tool.permission_level;
        let rule = self
            .rules
            .iter()
            .find(|r| !r.expired(now) && r.matches(&tool.name, args));

        // Level 4 (critical) is never auto-approved, trusted rule or not.
        if level == PermissionLevel::Level4 {
            return if autonomy >= 2 {
                Decision::RequireConfirmation {
                    reason: "level 4 (critical) operations always require explicit confirmation"
                        .into(),
                }
            } else {
                Decision::Deny {
                    reason: "autonomy level 1 is answer-only; critical operations are refused"
                        .into(),
                }
            };
        }

        // Autonomy 1 = answer only: nothing above level 0 may execute.
        if autonomy == 1 && level != PermissionLevel::Level0 {
            return Decision::Deny {
                reason: "autonomy level 1 permits informational lookups only".into(),
            };
        }

        // A matching user-authored trusted rule downgrades confirmation for <= L3.
        if let Some(rule) = rule {
            return Decision::Allow {
                reason: format!(
                    "trusted rule `{}` matched ({})",
                    rule.id,
                    if rule.note.is_empty() {
                        "no note"
                    } else {
                        &rule.note
                    }
                ),
            };
        }

        match level {
            PermissionLevel::Level0 => Decision::Allow {
                reason: "informational (level 0)".into(),
            },
            PermissionLevel::Level1 => {
                if autonomy >= 3 {
                    Decision::Allow {
                        reason: "low risk (level 1) at autonomy >= 3".into(),
                    }
                } else {
                    Decision::RequireConfirmation {
                        reason: "low risk (level 1) but autonomy level 2 asks before acting"
                            .into(),
                    }
                }
            }
            PermissionLevel::Level2 => {
                if autonomy >= 4 {
                    Decision::Allow {
                        reason: "moderate (level 2) at autonomy >= 4".into(),
                    }
                } else {
                    Decision::RequireConfirmation {
                        reason: "moderate risk (level 2) requires confirmation".into(),
                    }
                }
            }
            PermissionLevel::Level3 => {
                if autonomy >= 5 {
                    Decision::Allow {
                        reason: "trusted automation (level 5) covers high-risk (level 3) tools"
                            .into(),
                    }
                } else {
                    Decision::RequireConfirmation {
                        reason: if tool.destructive {
                            "high-risk destructive operation requires confirmation".into()
                        } else {
                            "high-risk operation requires confirmation".into()
                        },
                    }
                }
            }
            PermissionLevel::Level4 => unreachable!("handled above"),
        }
    }

    /// Convenience for a tool *name* that may not be registered yet: the gate only
    /// decides on descriptors, so unknown tools are the registry's problem.
    pub fn evaluate_named(
        &self,
        tool: &ToolDescriptor,
        args: &Value,
    ) -> Decision {
        self.evaluate(tool, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(level: PermissionLevel, destructive: bool) -> ToolDescriptor {
        ToolDescriptor {
            name: "test.tool".into(),
            description: "test".into(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
            permission_level: level,
            risk: Risk::Low,
            timeout_ms: 1000,
            destructive,
            confirmation_explanation: String::new(),
        }
    }

    fn gate(autonomy: u8) -> PermissionGate {
        PermissionGate::new(AutonomyLevel::new(autonomy).unwrap())
    }

    #[test]
    fn level0_always_allowed_from_autonomy_1() {
        for a in 1..=5 {
            let d = gate(a).evaluate(&tool(PermissionLevel::Level0, false), &json!({}));
            assert!(d.allowed(), "autonomy {a}: {d:?}");
        }
    }

    #[test]
    fn autonomy_1_denies_everything_above_level0() {
        for lvl in [PermissionLevel::Level1, PermissionLevel::Level2, PermissionLevel::Level3, PermissionLevel::Level4] {
            let d = gate(1).evaluate(&tool(lvl, false), &json!({}));
            assert!(matches!(d, Decision::Deny { .. }), "{lvl:?} => {d:?}");
        }
    }

    #[test]
    fn autonomy_3_allows_low_confirms_moderate_and_high() {
        let g = gate(3);
        assert!(g.evaluate(&tool(PermissionLevel::Level1, false), &json!({})).allowed());
        assert!(g
            .evaluate(&tool(PermissionLevel::Level2, false), &json!({}))
            .needs_confirmation());
        assert!(g
            .evaluate(&tool(PermissionLevel::Level3, true), &json!({}))
            .needs_confirmation());
    }

    #[test]
    fn autonomy_4_allows_moderate_but_confirms_high() {
        let g = gate(4);
        assert!(g.evaluate(&tool(PermissionLevel::Level2, false), &json!({})).allowed());
        assert!(g
            .evaluate(&tool(PermissionLevel::Level3, false), &json!({}))
            .needs_confirmation());
    }

    #[test]
    fn level4_never_auto_approved_at_any_autonomy() {
        for a in 1..=5 {
            let d = gate(a).evaluate(&tool(PermissionLevel::Level4, true), &json!({}));
            assert!(!d.allowed(), "autonomy {a} auto-approved level 4: {d:?}");
        }
    }

    #[test]
    fn level4_needs_confirmation_not_denial_from_autonomy_2_up() {
        for a in 2..=5 {
            let d = gate(a).evaluate(&tool(PermissionLevel::Level4, true), &json!({}));
            assert!(d.needs_confirmation(), "autonomy {a}: {d:?}");
        }
    }

    #[test]
    fn trusted_rule_allows_repeated_low_moderate_operations() {
        let mut g = gate(3);
        g.add_rule(TrustedRule {
            id: "rule-1".into(),
            tool: "test.tool".into(),
            args_contains: None,
            expires_at: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            note: "user approved test tool".into(),
        });
        let d = g.evaluate(&tool(PermissionLevel::Level3, true), &json!({}));
        assert!(d.allowed(), "{d:?}");
        // but it does not cover a different tool
        let mut other = tool(PermissionLevel::Level3, true);
        other.name = "other.tool".into();
        assert!(g.evaluate(&other, &json!({})).needs_confirmation());
    }

    #[test]
    fn trusted_rule_cannot_bypass_level4() {
        let mut g = gate(5);
        g.add_rule(TrustedRule {
            id: "rule-4".into(),
            tool: "test.tool".into(),
            args_contains: None,
            expires_at: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            note: "should not matter".into(),
        });
        let d = g.evaluate(&tool(PermissionLevel::Level4, true), &json!({}));
        assert!(d.needs_confirmation(), "rule bypassed level 4: {d:?}");
    }

    #[test]
    fn expired_rule_does_not_apply() {
        // Autonomy 4 still asks for level 3 unless a valid rule covers it.
        let mut g = gate(4);
        g.add_rule(TrustedRule {
            id: "old".into(),
            tool: "test.tool".into(),
            args_contains: None,
            expires_at: Some("2020-01-01T00:00:00Z".into()),
            created_at: "2019-01-01T00:00:00Z".into(),
            note: String::new(),
        });
        let d = g.evaluate(&tool(PermissionLevel::Level3, true), &json!({}));
        assert!(d.needs_confirmation(), "expired rule was honoured: {d:?}");

        // The same rule, still valid, removes the prompt.
        g.rules.clear();
        g.add_rule(TrustedRule {
            id: "valid".into(),
            tool: "test.tool".into(),
            args_contains: None,
            expires_at: Some("2099-01-01T00:00:00Z".into()),
            created_at: "2019-01-01T00:00:00Z".into(),
            note: "still valid".into(),
        });
        let d = g.evaluate(&tool(PermissionLevel::Level3, true), &json!({}));
        assert!(d.allowed(), "valid rule should allow: {d:?}");
    }

    #[test]
    fn rules_can_be_removed() {
        let mut g = gate(3);
        g.add_rule(TrustedRule {
            id: "x".into(),
            tool: "a.b".into(),
            args_contains: None,
            expires_at: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            note: String::new(),
        });
        assert!(g.remove_rule("x"));
        assert!(!g.remove_rule("x"));
        assert!(g.rules().is_empty());
    }

    #[tokio::test]
    async fn confirmer_defaults_are_safe() {
        let req = ConfirmationRequest {
            tool: "fs.delete".into(),
            arguments: "{}".into(),
            level: PermissionLevel::Level3,
            risk: Risk::High,
            reason: "destructive".into(),
            explanation: "delete files".into(),
        };
        assert!(!NeverConfirm.confirm(req.clone()).await);
        assert!(AlwaysConfirm.confirm(req).await);
    }
}