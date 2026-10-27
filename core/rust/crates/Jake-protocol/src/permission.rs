//! Permission levels, risk classification, autonomy levels and gate decisions.

use serde::{Deserialize, Serialize};

/// How much authority a tool needs. See `docs/permissions.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionLevel {
    /// Read-only information (system stats, time, listing registered tools).
    Level0 = 0,
    /// Low risk: open/switch an application, search files, read permitted files.
    Level1 = 1,
    /// Moderate: create/modify files, send messages, inject keyboard/mouse input.
    Level2 = 2,
    /// High risk: delete files, install software, change system settings, shell.
    Level3 = 3,
    /// Critical: financial transactions, account/security configuration changes.
    Level4 = 4,
}

impl PermissionLevel {
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Level0),
            1 => Some(Self::Level1),
            2 => Some(Self::Level2),
            3 => Some(Self::Level3),
            4 => Some(Self::Level4),
            _ => None,
        }
    }

    /// Short human label used in confirmation prompts and logs.
    pub fn label(self) -> &'static str {
        match self {
            Self::Level0 => "information",
            Self::Level1 => "low risk",
            Self::Level2 => "moderate",
            Self::Level3 => "high risk",
            Self::Level4 => "critical",
        }
    }
}

/// Consequence class of a tool, independent of its permission level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Low,
    Moderate,
    High,
    Critical,
}

/// How much the agent may do without asking. See `docs/permissions.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AutonomyLevel(pub u8);

impl AutonomyLevel {
    pub const MIN: u8 = 1;
    pub const MAX: u8 = 5;

    /// Default: level 3 — execute informational/low-risk actions, confirm the rest.
    pub const DEFAULT: Self = Self(3);

    pub fn new(v: u8) -> Result<Self, String> {
        if (Self::MIN..=Self::MAX).contains(&v) {
            Ok(Self(v))
        } else {
            Err(format!(
                "autonomy level must be {}..={}, got {v}",
                Self::MIN,
                Self::MAX
            ))
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

impl Default for AutonomyLevel {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Outcome of evaluating one tool execution against the permission model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Decision {
    /// Execute now. `reason` records why (autonomy, trusted rule, level 0).
    Allow { reason: String },
    /// Stop and ask the user; execute only after an explicit confirmation.
    RequireConfirmation { reason: String },
    /// Refuse outright. The brain must report this honestly, never pretend.
    Deny { reason: String },
}

impl Decision {
    pub fn allowed(&self) -> bool {
        matches!(self, Decision::Allow { .. })
    }

    pub fn needs_confirmation(&self) -> bool {
        matches!(self, Decision::RequireConfirmation { .. })
    }
}

/// A user-authored rule that skips repeated confirmations for a specific pattern.
/// Rules are created by the user, never by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrustedRule {
    pub id: String,
    /// Exact tool name, or `prefix.*` for a tool family.
    pub tool: String,
    /// Optional JSON-fragment match against the arguments (`{"paths":["docs/*"]}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args_contains: Option<serde_json::Value>,
    /// Optional expiry (RFC3339). Absent = until revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub created_at: String,
    pub note: String,
}

impl TrustedRule {
    pub fn matches(&self, tool: &str, args: &serde_json::Value) -> bool {
        let tool_match = if let Some(prefix) = self.tool.strip_suffix(".*") {
            tool.starts_with(prefix) && (tool.len() == prefix.len() || tool.as_bytes().get(prefix.len()) == Some(&b'.'))
        } else {
            tool == self.tool
        };
        if !tool_match {
            return false;
        }
        match &self.args_contains {
            None => true,
            Some(fragment) => subset_match(fragment, args),
        }
    }

    pub fn expired(&self, now_rfc3339: &str) -> bool {
        match &self.expires_at {
            None => false,
            Some(expiry) => expiry.as_str() <= now_rfc3339,
        }
    }
}

/// True when every key/value of `fragment` appears (recursively) in `value`.
fn subset_match(fragment: &serde_json::Value, value: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (fragment, value) {
        (Value::Object(f), Value::Object(v)) => f
            .iter()
            .all(|(k, fv)| v.get(k).map(|vv| subset_match(fv, vv)).unwrap_or(false)),
        (Value::Array(f), Value::Array(v)) => f.iter().all(|fv| v.iter().any(|vv| subset_match(fv, vv))),
        (Value::String(f), Value::String(v)) => glob_match(f, v),
        (a, b) => a == b,
    }
}

/// Minimal glob for rule patterns: `*` matches any run of characters (including
/// path separators), everything else is literal.
fn glob_match(pattern: &str, value: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == value;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    // leading literal must be a prefix, trailing literal must be a suffix, and the
    // middle literals must appear in order.
    let mut pos = 0usize;
    if let Some(first) = parts.first() {
        if !value.starts_with(first) {
            return false;
        }
        pos = first.len();
    }
    for part in parts.iter().skip(1).take(parts.len().saturating_sub(2)) {
        match value[pos..].find(part) {
            Some(idx) => pos += idx + part.len(),
            None => return false,
        }
    }
    if let Some(last) = parts.last() {
        if parts.len() > 1 && !value[pos..].ends_with(last) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn levels_are_ordered() {
        assert!(PermissionLevel::Level4 > PermissionLevel::Level3);
        assert!(PermissionLevel::Level0 < PermissionLevel::Level1);
        assert_eq!(PermissionLevel::Level2.as_u8(), 2);
        assert_eq!(PermissionLevel::from_u8(9), None);
    }

    #[test]
    fn autonomy_bounds() {
        assert!(AutonomyLevel::new(0).is_err());
        assert!(AutonomyLevel::new(6).is_err());
        assert_eq!(AutonomyLevel::new(3).unwrap().get(), 3);
        assert_eq!(AutonomyLevel::default().get(), 3);
    }

    #[test]
    fn rule_matches_tool_family_and_args() {
        let rule = TrustedRule {
            id: "r1".into(),
            tool: "fs.*".into(),
            args_contains: Some(json!({"paths": ["docs/*"]})),
            expires_at: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            note: "read docs".into(),
        };
        assert!(rule.matches("fs.read", &json!({"paths": ["docs/a.md"]})));
        assert!(rule.matches("fs.read", &json!({"paths": ["docs/deep/b.md"]})));
        assert!(!rule.matches("fs.read", &json!({"paths": ["secrets/a.md"]})));
        assert!(!rule.matches("fs.read", &json!({"paths": []})));
        assert!(!rule.matches("app.open", &json!({"paths": ["docs/*"]})));
    }

    #[test]
    fn glob_matching_is_predictable() {
        assert!(glob_match("docs/*", "docs/a.md"));
        assert!(glob_match("*.md", "notes.md"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("a*b*c", "axxbyyc"));
        assert!(!glob_match("a*b*c", "ac")); // middle literals must exist
        assert!(!glob_match("docs/*", "src/main.rs"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "exactly"));
    }

    #[test]
    fn rule_expiry() {
        let rule = TrustedRule {
            id: "r".into(),
            tool: "fs.read".into(),
            args_contains: None,
            expires_at: Some("2026-01-01T00:00:00Z".into()),
            created_at: "2025-01-01T00:00:00Z".into(),
            note: String::new(),
        };
        assert!(rule.expired("2026-06-01T00:00:00Z"));
        assert!(!rule.expired("2025-06-01T00:00:00Z"));
    }

    #[test]
    fn decision_tags_are_stable() {
        let d = Decision::Deny { reason: "no".into() };
        assert_eq!(
            serde_json::to_value(&d).unwrap(),
            json!({"type": "deny", "reason": "no"})
        );
    }
}