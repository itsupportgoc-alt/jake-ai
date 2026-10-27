//! Phase-4 long-term memory: `memory.store` / `memory.search` / `memory.forget`.
//!
//! Storage is a single JSON file (`data/memory.json` under the project root) —
//! human-readable, diffable, trivially exportable. Searches are keyword-scored
//! with a recency bias (no network, no embeddings: this machine's providers may
//! be offline, so memory never depends on them).
//!
//! Safety rules:
//! * **Secrets are never stored** — anything that smells like a credential is
//!   refused with an honest error (the assistant can still *say* it).
//! * `memory.forget` is destructive and gated: it asks for confirmation like
//!   every other destructive tool.
//! * Every write re-reads the file, so two brain processes stay consistent.

use crate::tools::{Tool, ToolContext, ToolError, ToolRegistry};
use Jake_protocol::permission::{PermissionLevel, Risk};
use Jake_protocol::tool::ToolDescriptor;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

// ------------------------------------------------------------------- storage

#[derive(Serialize, Deserialize, Clone)]
struct MemoryEntry {
    id: String,
    created_at: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    tags: Vec<String>,
    content: String,
}

#[derive(Serialize, Deserialize, Default)]
struct MemoryFile {
    version: u32,
    #[serde(default)]
    entries: Vec<MemoryEntry>,
}

fn memory_path(ctx: &ToolContext) -> PathBuf {
    ctx.project_root.join("data").join("memory.json")
}

fn load(path: &PathBuf) -> Result<MemoryFile, ToolError> {
    if !path.exists() {
        return Ok(MemoryFile { version: 1, entries: vec![] });
    }
    let raw = std::fs::read_to_string(path)
        .map_err(|e| ToolError::failed(format!("cannot read memory store: {e}")))?;
    serde_json::from_str(&raw).map_err(|e| ToolError::failed(format!("memory store is corrupt: {e}")))
}

fn save(path: &PathBuf, file: &MemoryFile) -> Result<(), ToolError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ToolError::failed(format!("cannot create data dir: {e}")))?;
    }
    let raw = serde_json::to_string_pretty(file)
        .map_err(|e| ToolError::failed(format!("cannot serialise memory: {e}")))?;
    std::fs::write(path, raw).map_err(|e| ToolError::failed(format!("cannot write memory store: {e}")))
}

// ------------------------------------------------------------- secret screen

/// Refuse to persist anything that looks like a credential. Mirrors the Python
/// `redact` heuristics: the assistant may speak a secret, never store it.
fn secret_smell(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    let patterns: [(&str, &str); 12] = [
        ("sk-", "OpenAI-style API key"),
        ("sk_live_", "live payment key"),
        ("ghp_", "GitHub token"),
        ("gho_", "GitHub token"),
        ("xox", "Slack token"),
        ("AKIA", "AWS access key id"),
        ("-----begin", "private key block"),
        ("bearer ", "authorization bearer token"),
        ("password=", "password"),
        ("password:", "password"),
        ("api_key", "API key"),
        ("api key:", "API key"),
    ];
    for (needle, why) in patterns {
        if lower.contains(needle) {
            return Some(why);
        }
    }
    None
}

// -------------------------------------------------------------------- scoring

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .map(|t| t.to_ascii_lowercase())
        .filter(|t| t.len() >= 2)
        .collect()
}

/// Keyword overlap + recency: newest matches win ties; nothing needs the cloud.
fn score(entry: &MemoryEntry, query_tokens: &[String], position: usize) -> f64 {
    if query_tokens.is_empty() {
        return 0.0;
    }
    let hay = entry.content.to_ascii_lowercase();
    let mut hits = 0.0;
    for t in query_tokens {
        if hay.contains(t.as_str()) {
            hits += 1.0;
        }
    }
    if hits == 0.0 {
        return 0.0;
    }
    // distinct hits dominate; recency (position in file = write order) breaks ties
    hits + (1.0 / (1.0 + position as f64)) * 0.5
}

// ---------------------------------------------------------------------- tools

fn base(
    name: &str,
    description: &str,
    input_schema: Value,
    level: PermissionLevel,
    risk: Risk,
    timeout_ms: u64,
    destructive: bool,
    confirmation_explanation: &str,
) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        description: description.into(),
        input_schema,
        output_schema: json!({"type": "object"}),
        permission_level: level,
        risk,
        timeout_ms,
        destructive,
        confirmation_explanation: confirmation_explanation.into(),
    }
}

struct MemoryStore {
    descriptor: ToolDescriptor,
}

impl Tool for MemoryStore {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if content.is_empty() {
            return Err(ToolError::invalid("`content` (non-empty string) is required"));
        }
        if content.len() > 4000 {
            return Err(ToolError::invalid("`content` longer than 4000 characters"));
        }
        if let Some(why) = secret_smell(&content) {
            return Err(ToolError::new(
                "secret_refused",
                format!("I will not store that — it looks like a {why}. Secrets never enter long-term memory."),
            ));
        }
        let kind = args
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("note")
            .trim()
            .to_ascii_lowercase();
        if !kind.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') || kind.len() > 32 {
            return Err(ToolError::invalid("`kind` must be a short alphanumeric token"));
        }
        let tags: Vec<String> = args
            .get("tags")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(|t| t.trim().to_ascii_lowercase())
                    .filter(|t| !t.is_empty() && t.len() <= 32)
                    .take(8)
                    .collect()
            })
            .unwrap_or_default();

        let path = memory_path(ctx);
        let mut file = load(&path)?;

        // Idempotent: storing the same content twice does not duplicate it.
        if let Some(existing) = file.entries.iter().find(|e| e.content == content) {
            return Ok(json!({
                "id": existing.id,
                "deduped": true,
                "total": file.entries.len(),
                "path": path.display().to_string(),
            }));
        }

        let seq = file.entries.len() as u128;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let id = format!("mem-{nanos:x}-{seq:x}");
        let entry = MemoryEntry {
            id: id.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
            kind,
            tags: tags.clone(),
            content: content.clone(),
        };
        file.entries.push(entry);
        save(&path, &file)?;
        Ok(json!({
            "id": id,
            "deduped": false,
            "total": file.entries.len(),
            "path": path.display().to_string(),
        }))
    }
}

struct MemorySearch {
    descriptor: ToolDescriptor,
}

impl Tool for MemorySearch {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let query = args.get("query").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if query.is_empty() {
            return Err(ToolError::invalid("`query` (non-empty string) is required"));
        }
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(5).clamp(1, 20) as usize;

        let path = memory_path(ctx);
        let file = load(&path)?;
        let q = tokens(&query);

        let mut scored: Vec<(f64, &MemoryEntry)> = file
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (score(e, &q, i), e))
            .filter(|(s, _)| *s > 0.0)
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let matches: Vec<Value> = scored
            .iter()
            .take(limit)
            .map(|(s, e)| {
                json!({
                    "id": e.id,
                    "score": (*s * 100.0).round() / 100.0,
                    "created_at": e.created_at,
                    "kind": e.kind,
                    "tags": e.tags,
                    "content": e.content,
                })
            })
            .collect();

        Ok(json!({
            "query": query,
            "matches": matches,
            "count": matches.len(),
            "total_memories": file.entries.len(),
        }))
    }
}

struct MemoryForget {
    descriptor: ToolDescriptor,
}

impl Tool for MemoryForget {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let id = args.get("id").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        let query = args.get("query").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        if id.is_none() && query.is_none() {
            return Err(ToolError::invalid("memory.forget requires `id` or `query`"));
        }

        let path = memory_path(ctx);
        let mut file = load(&path)?;
        let before = file.entries.len();

        let removed_ids: Vec<String> = match id {
            Some(want) => {
                let hit: Vec<String> = file
                    .entries
                    .iter()
                    .filter(|e| e.id == want)
                    .map(|e| e.id.clone())
                    .collect();
                if hit.is_empty() {
                    return Err(ToolError::not_found(format!("no memory with id {want}")));
                }
                file.entries.retain(|e| e.id != want);
                hit
            }
            None => {
                let q = tokens(query.unwrap_or(""));
                let mut hit = Vec::new();
                file.entries.retain(|e| {
                    let keep = score(e, &q, 0) == 0.0;
                    if !keep {
                        hit.push(e.id.clone());
                    }
                    keep
                });
                if hit.is_empty() {
                    return Ok(json!({"removed": [], "removed_count": 0, "total": before, "note": "nothing matched — no memory was changed"}));
                }
                hit
            }
        };

        save(&path, &file)?;
        Ok(json!({
            "removed": removed_ids,
            "removed_count": removed_ids.len(),
            "total": file.entries.len(),
        }))
    }
}

// -------------------------------------------------------------- registration

pub fn register_all(registry: &mut ToolRegistry) -> Result<(), ToolError> {
    registry.register(Arc::new(MemoryStore {
        descriptor: base(
            "memory.store",
            "Write a fact, preference, decision or event to long-term memory (survives restarts). Secrets (keys, passwords) are refused. Storing the same content twice is idempotent.",
            json!({
                "type": "object",
                "properties": {
                    "content": {"type": "string", "description": "What to remember (max 4000 chars)"},
                    "kind": {"type": "string", "description": "note|preference|decision|event|person"},
                    "tags": {"type": "array", "items": {"type": "string"}, "description": "lowercase topic tags"}
                },
                "required": ["content"],
                "additionalProperties": false
            }),
            PermissionLevel::Level1,
            Risk::Low,
            10_000,
            false,
            "",
        ),
    }))?;

    registry.register(Arc::new(MemorySearch {
        descriptor: base(
            "memory.search",
            "Search long-term memory by keywords (offline keyword scoring with recency bias). Use before answering questions about past conversations, preferences or decisions.",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "limit": {"type": "integer", "description": "max matches, default 5"}
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            PermissionLevel::Level1,
            Risk::Low,
            10_000,
            false,
            "",
        ),
    }))?;

    registry.register(Arc::new(MemoryForget {
        descriptor: base(
            "memory.forget",
            "Delete memories by exact id or by query. Cannot be undone.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "exact memory id"},
                    "query": {"type": "string", "description": "delete memories matching these keywords"}
                },
                "additionalProperties": false
            }),
            PermissionLevel::Level2,
            Risk::Moderate,
            10_000,
            true,
            "Deleting memories cannot be undone.",
        ),
    }))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin;
    use crate::permission::{AutonomyLevel, Confirmer, PermissionGate};
    use Jake_protocol::tool::ToolExecution;
    use std::time::Duration;

    fn registry() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        builtin::register_all(&mut r).unwrap();
        r
    }

    /// Context whose project_root is a temp dir (memory lands there, not in the
    /// real data/ folder).
    struct TempRoot {
        dir: tempfile::TempDir,
    }

    impl TempRoot {
        fn new() -> Self {
            Self { dir: tempfile::tempdir().unwrap() }
        }
        fn ctx(&self) -> ToolContext {
            let mut c = ToolContext::new(self.dir.path().to_path_buf());
            c.roots = vec![self.dir.path().to_path_buf()];
            c
        }
    }

    async fn call_with(ctx: &ToolContext, name: &str, args: Value) -> Jake_protocol::tool::ToolResult {
        let r = registry();
        let gate = PermissionGate::new(AutonomyLevel::new(3).unwrap());
        let confirmer: Arc<dyn Confirmer> = Arc::new(crate::permission::AlwaysConfirm);
        r.dispatch(
            ToolExecution { id: "t".into(), name: name.into(), arguments: args },
            ctx,
            &gate,
            &confirmer,
            Duration::from_secs(30),
        )
        .await
    }

    async fn call(name: &str, args: Value) -> Jake_protocol::tool::ToolResult {
        let root = TempRoot::new();
        call_with(&root.ctx(), name, args).await
    }

    #[test]
    fn memory_tools_register_at_expected_levels() {
        let r = registry();
        let level = |n: &str| {
            r.descriptors().into_iter().find(|d| d.name == n)
                .unwrap_or_else(|| panic!("{n} missing"))
                .permission_level
        };
        let destructive = |n: &str| {
            r.descriptors().into_iter().find(|d| d.name == n)
                .unwrap_or_else(|| panic!("{n} missing"))
                .destructive
        };
        assert_eq!(level("memory.store"), PermissionLevel::Level1);
        assert_eq!(level("memory.search"), PermissionLevel::Level1);
        assert_eq!(level("memory.forget"), PermissionLevel::Level2);
        assert!(destructive("memory.forget"), "forget must be gated as destructive");
        assert!(!destructive("memory.store"));
    }

    #[test]
    fn secret_smell_detects_common_credential_shapes() {
        assert!(secret_smell("my key is sk-abc123").is_some());
        assert!(secret_smell("password: hunter2").is_some());
        assert!(secret_smell("Bearer eyJhbGciOi").is_some());
        assert!(secret_smell("-----BEGIN RSA PRIVATE KEY-----").is_some());
        assert!(secret_smell("the user prefers dark mode").is_none());
        assert!(secret_smell("remember: buy milk").is_none());
    }

    #[tokio::test]
    async fn store_search_forget_round_trip() {
        let root = TempRoot::new();
        let ctx = root.ctx();

        let s1 = call_with(&ctx, "memory.store", json!({
            "content": "The user prefers metric units and Celsius",
            "kind": "preference",
            "tags": ["units"]
        })).await;
        assert!(s1.ok, "{s1:?}");
        let id1 = s1.data.unwrap()["id"].as_str().unwrap().to_string();

        let s2 = call_with(&ctx, "memory.store", json!({"content": "Project ships on Fridays"})).await;
        assert!(s2.ok, "{s2:?}");

        // idempotence: same content again → same id, no duplicate
        let s3 = call_with(&ctx, "memory.store", json!({
            "content": "The user prefers metric units and Celsius"
        })).await;
        assert!(s3.ok, "{s3:?}");
        assert_eq!(s3.data.as_ref().unwrap()["deduped"], true);
        assert_eq!(s3.data.as_ref().unwrap()["total"], 2);

        // search finds the preference by keyword
        let f = call_with(&ctx, "memory.search", json!({"query": "metric celsius"})).await;
        assert!(f.ok, "{f:?}");
        let matches = f.data.unwrap()["matches"].clone();
        assert_eq!(matches.as_array().map(|a| a.len()).unwrap_or(0), 1, "{matches}");
        assert_eq!(matches[0]["id"], id1.as_str());

        // secret refusal
        let bad = call_with(&ctx, "memory.store", json!({"content": "api_key: sk-12345"})).await;
        assert!(!bad.ok, "secrets must be refused: {bad:?}");
        assert_eq!(bad.error_code.as_deref(), Some("secret_refused"));

        // forget by id, then it is gone
        let d = call_with(&ctx, "memory.forget", json!({"id": id1})).await;
        assert!(d.ok, "{d:?}");
        assert_eq!(d.data.unwrap()["removed_count"], 1);
        let f2 = call_with(&ctx, "memory.search", json!({"query": "metric celsius"})).await;
        assert_eq!(f2.data.unwrap()["count"], 0);

        // the file exists on disk in the exportable format
        let raw = std::fs::read_to_string(memory_path(&ctx)).unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["version"], 1);
        assert_eq!(parsed["entries"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn forget_by_query_and_honest_no_match() {
        let root = TempRoot::new();
        let ctx = root.ctx();
        call_with(&ctx, "memory.store", json!({"content": "meeting with Ana on Tuesday"})).await;
        call_with(&ctx, "memory.store", json!({"content": "prefer tabs over spaces"})).await;

        let miss = call_with(&ctx, "memory.forget", json!({"query": "unrelated zebra"})).await;
        assert!(miss.ok, "{miss:?}");
        assert_eq!(miss.data.as_ref().unwrap()["removed_count"], 0);
        assert!(miss.data.as_ref().unwrap()["note"].as_str().unwrap().contains("no memory was changed"));

        let hit = call_with(&ctx, "memory.forget", json!({"query": "ana tuesday"})).await;
        assert!(hit.ok, "{hit:?}");
        assert_eq!(hit.data.unwrap()["removed_count"], 1);

        let left = call_with(&ctx, "memory.search", json!({"query": "tabs"})).await;
        assert_eq!(left.data.unwrap()["count"], 1);
    }

    #[tokio::test]
    async fn forget_requires_either_id_or_query() {
        let res = call("memory.forget", json!({})).await;
        assert!(!res.ok);
        assert_eq!(res.error_code.as_deref(), Some("invalid_arguments"));
    }

    #[tokio::test]
    async fn empty_store_searches_honestly() {
        let root = TempRoot::new();
        let ctx = root.ctx();
        let f = call_with(&ctx, "memory.search", json!({"query": "anything"})).await;
        assert!(f.ok, "{f:?}");
        assert_eq!(f.data.as_ref().unwrap()["count"], 0);
        assert_eq!(f.data.as_ref().unwrap()["total_memories"], 0);
    }
}