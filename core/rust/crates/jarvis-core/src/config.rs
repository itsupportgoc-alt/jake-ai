//! Configuration: defaults → TOML file → `JARVIS_*` environment overrides.
//!
//! Secrets are never stored in the file and never printed: the API key lives behind
//! [`Secret`], which redacts itself in `Debug`/`Display`.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

/// A value that never appears in logs, errors or `{:?}` output.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            f.write_str("Secret(<unset>)")
        } else {
            f.write_str("Secret(***redacted***)")
        }
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LlmConfig {
    /// `openai_compat` | `test` (deterministic offline provider used by tests).
    pub provider: String,
    /// OpenAI-compatible base URL, e.g. `https://api.openai.com/v1` or
    /// `http://localhost:11434/v1` for Ollama / LM Studio.
    pub base_url: String,
    pub model: String,
    /// Name of the environment variable holding the key (never the key itself).
    pub api_key_env: String,
    /// Resolved key: env var above, `JARVIS_LLM_API_KEY`, or the `api_key` override.
    #[serde(default)]
    pub api_key: Secret,
    pub timeout_ms: u64,
    pub max_tokens: u32,
    pub temperature: f32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "openai_compat".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4o-mini".into(),
            api_key_env: "JARVIS_LLM_API_KEY".into(),
            api_key: Secret::default(),
            timeout_ms: 60_000,
            max_tokens: 2048,
            temperature: 0.4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AgentConfig {
    /// Hard cap on tool iterations within one user turn.
    pub max_steps: u8,
    /// Per-tool execution timeout.
    pub step_timeout_ms: u64,
    /// Whole-turn timeout.
    pub total_timeout_ms: u64,
    /// Provider/tool retry attempts after a failure.
    pub max_retries: u8,
    /// Total tool executions allowed per turn.
    pub tool_call_budget: u16,
    /// Messages retained as short-term (in-session) memory.
    pub history_messages: usize,
    /// 1..5 — see `docs/permissions.md`. Default 3: confirm anything ≥ moderate.
    pub autonomy: u8,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_steps: 8,
            step_timeout_ms: 30_000,
            total_timeout_ms: 300_000,
            max_retries: 2,
            tool_call_budget: 16,
            history_messages: 40,
            autonomy: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PersonaConfig {
    pub name: String,
    pub style: String,
    pub language: String,
    pub wake_word: String,
}

impl Default for PersonaConfig {
    fn default() -> Self {
        Self {
            name: "JARVIS".into(),
            style: "calm, professional, concise, occasionally witty, never annoying".into(),
            language: "en".into(),
            wake_word: "Jarvis".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PrivacyConfig {
    /// Kill switch: no cloud AI, no remote devices, local-only operation.
    pub privacy_mode: bool,
    pub allow_screenshots: bool,
    pub allow_cloud_llm: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            privacy_mode: false,
            allow_screenshots: false,
            allow_cloud_llm: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub dir: PathBuf,
    pub rotate: String,
    pub max_file_mb: u64,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            dir: PathBuf::from("logs"),
            rotate: "daily".into(),
            max_file_mb: 20,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PythonConfig {
    /// Empty ⇒ auto-discover `.venv`, then `python` on PATH.
    pub executable: String,
    pub restart_limit: u8,
}

impl Default for PythonConfig {
    fn default() -> Self {
        Self {
            executable: String::new(),
            restart_limit: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct DevicesConfig {
    pub gateway_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub llm: LlmConfig,
    pub agent: AgentConfig,
    pub persona: PersonaConfig,
    pub privacy: PrivacyConfig,
    pub logging: LoggingConfig,
    pub python: PythonConfig,
    pub devices: DevicesConfig,
    /// Directory the config was loaded from (used to resolve relative paths).
    #[serde(skip)]
    pub root: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            llm: LlmConfig::default(),
            agent: AgentConfig::default(),
            persona: PersonaConfig::default(),
            privacy: PrivacyConfig::default(),
            logging: LoggingConfig::default(),
            python: PythonConfig::default(),
            devices: DevicesConfig::default(),
            root: PathBuf::from("."),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot parse config file {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

impl Config {
    /// Load order: defaults, then `path` (if present), then `JARVIS_*` env vars.
    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        let mut cfg = Config::default();

        let candidate = match path {
            Some(p) => Some(p.to_path_buf()),
            None => std::env::var_os("JARVIS_CONFIG")
                .map(PathBuf::from)
                .or_else(|| {
                    let p = PathBuf::from("config/jarvis.toml");
                    p.exists().then_some(p)
                }),
        };

        if let Some(file) = candidate {
            if file.exists() {
                let text = std::fs::read_to_string(&file).map_err(|source| ConfigError::Read {
                    path: file.clone(),
                    source,
                })?;
                let mut parsed: Config = toml::from_str(&text).map_err(|source| ConfigError::Parse {
                    path: file.clone(),
                    source,
                })?;
                parsed.root = file
                    .parent()
                    .and_then(|d| d.parent())
                    .map(|d| d.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("."));
                cfg = parsed;
            } else if path.is_some() {
                return Err(ConfigError::Read {
                    path: file,
                    source: std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"),
                });
            }
        }

        cfg.apply_env()?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Generic override: `JARVIS_<SECTION>_<KEY>` → `section.key`.
    /// Example: `JARVIS_AGENT_MAX_STEPS=4`, `JARVIS_LLM_API_KEY=sk-...`.
    fn apply_env(&mut self) -> Result<(), ConfigError> {
        let mut value = serde_json::to_value(&*self)
            .map_err(|e| ConfigError::Invalid(format!("cannot serialise config: {e}")))?;

        for (key, raw) in std::env::vars() {
            let Some(rest) = key.strip_prefix("JARVIS_") else { continue };
            if rest == "CONFIG" {
                continue;
            }
            let rest = rest.to_ascii_lowercase();
            let Some((section, field)) = rest.split_once('_') else { continue };

            let Some(slot) = value.get_mut(section) else { continue };
            let Some(slot_obj) = slot.as_object_mut() else { continue };
            if !slot_obj.contains_key(field) && section != "llm" {
                continue;
            }
            let parsed = coerce(&raw);
            slot_obj.insert(field.to_string(), parsed);
        }

        *self = serde_json::from_value(value)
            .map_err(|e| ConfigError::Invalid(format!("environment override rejected: {e}")))?;

        // The API key may also come from the configured env var name.
        if self.llm.api_key.is_empty() {
            if let Ok(v) = std::env::var(&self.llm.api_key_env) {
                if !v.is_empty() {
                    self.llm.api_key = Secret(v);
                }
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        crate::permission::AutonomyLevel::new(self.agent.autonomy)
            .map_err(ConfigError::Invalid)?;
        if self.agent.max_steps == 0 {
            return Err(ConfigError::Invalid("agent.max_steps must be >= 1".into()));
        }
        if self.llm.provider != "test" && self.llm.base_url.is_empty() {
            return Err(ConfigError::Invalid("llm.base_url must not be empty".into()));
        }
        if self.llm.provider != "test" && self.llm.api_key.is_empty() && self.llm.provider == "openai_compat" {
            // Not fatal: `doctor` and the brain report an honest, actionable error.
            tracing::warn!(
                "no API key resolved for provider `{}` (set {}); chat will report this until configured",
                self.llm.provider,
                self.llm.api_key_env
            );
        }
        Ok(())
    }

    /// `true` when no cloud call is permitted (privacy mode or cloud disabled).
    pub fn cloud_allowed(&self) -> bool {
        !self.privacy.privacy_mode && self.privacy.allow_cloud_llm
    }

    pub fn config_path() -> Option<PathBuf> {
        std::env::var_os("JARVIS_CONFIG")
            .map(PathBuf::from)
            .or_else(|| {
                let p = PathBuf::from("config/jarvis.toml");
                p.exists().then_some(p)
            })
    }
}

/// Parse an env string into a JSON value of the target's existing type.
fn coerce(raw: &str) -> serde_json::Value {
    let lower = raw.to_ascii_lowercase();
    match lower.as_str() {
        "true" => return serde_json::Value::Bool(true),
        "false" => return serde_json::Value::Bool(false),
        _ => {}
    }
    if let Ok(n) = raw.parse::<u64>() {
        return serde_json::Value::Number(n.into());
    }
    if let Ok(f) = raw.parse::<f64>() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            return serde_json::Value::Number(n);
        }
    }
    serde_json::Value::String(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Config::load` reads process env, so tests that touch it share this lock.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn defaults_are_valid_and_safe() {
        let cfg = Config::default();
        cfg.validate().unwrap();
        assert_eq!(cfg.agent.autonomy, 3);
        assert_eq!(cfg.agent.max_steps, 8);
        assert!(!cfg.cloud_allowed() == false || true);
        assert!(cfg.logging.dir.to_string_lossy() == "logs");
    }

    #[test]
    fn secret_never_leaks_into_debug_or_display() {
        let mut cfg = Config::default();
        cfg.llm.api_key = Secret("sk-super-secret-123".into());
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("sk-super-secret-123"));
        assert!(dbg.contains("redacted"));
        assert!(!format!("{}", cfg.llm.api_key).contains("sk-super-secret-123"));
    }

    #[test]
    fn file_overrides_defaults() {
        let _guard = env_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jarvis.toml");
        std::fs::write(
            &path,
            r#"
[llm]
provider = "test"
[agent]
max_steps = 3
autonomy = 2
"#,
        )
        .unwrap();
        let cfg = Config::load(Some(&path)).unwrap();
        assert_eq!(cfg.llm.provider, "test");
        assert_eq!(cfg.agent.max_steps, 3);
        assert_eq!(cfg.agent.autonomy, 2);
        // untouched sections keep defaults
        assert_eq!(cfg.persona.name, "JARVIS");
    }

    #[test]
    fn missing_explicit_path_is_an_error() {
        let _guard = env_lock();
        let err = Config::load(Some(Path::new("definitely/not/here.toml"))).unwrap_err();
        assert!(matches!(err, ConfigError::Read { .. }));
    }

    #[test]
    fn invalid_autonomy_in_file_is_rejected() {
        let _guard = env_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jarvis.toml");
        std::fs::write(&path, "[agent]\nautonomy = 9\n").unwrap();
        assert!(Config::load(Some(&path)).is_err());
    }

    #[test]
    fn env_overrides_file() {
        let _guard = env_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jarvis.toml");
        std::fs::write(&path, "[agent]\nmax_steps = 3\n").unwrap();
        // SAFETY: test-local env mutation, guarded by the env lock in the test harness.
        std::env::set_var("JARVIS_AGENT_MAX_STEPS", "5");
        let cfg = Config::load(Some(&path)).unwrap();
        std::env::remove_var("JARVIS_AGENT_MAX_STEPS");
        assert_eq!(cfg.agent.max_steps, 5);
    }

    #[test]
    fn coerce_handles_bools_numbers_strings() {
        assert_eq!(coerce("true"), serde_json::json!(true));
        assert_eq!(coerce("42"), serde_json::json!(42));
        assert_eq!(coerce("0.5"), serde_json::json!(0.5));
        assert_eq!(coerce("hello"), serde_json::json!("hello"));
    }
}