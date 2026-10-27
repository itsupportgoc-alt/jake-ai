//! Built-in tools — the first real capabilities the agent can exercise.
//!
//! Scope of Phase 1: read-only / low-risk (levels 0–1). Application control, input
//! injection, screenshots and destructive file operations arrive in Phase 3, gated
//! the same way.

use crate::tools::{Tool, ToolContext, ToolError, ToolRegistry};
use Jake_protocol::permission::{PermissionLevel, Risk};
use Jake_protocol::tool::ToolDescriptor;
use serde_json::{json, Value};

fn base(
    name: &str,
    description: &str,
    input_schema: Value,
    output_schema: Value,
    level: PermissionLevel,
    risk: Risk,
    timeout_ms: u64,
) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        description: description.into(),
        input_schema,
        output_schema,
        permission_level: level,
        risk,
        timeout_ms,
        destructive: false,
        confirmation_explanation: String::new(),
    }
}

// ---------------------------------------------------------------- time.now (L0)

struct TimeNow {
    descriptor: ToolDescriptor,
}

impl Tool for TimeNow {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    fn execute(&self, _args: &Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let now = chrono::Utc::now();
        Ok(json!({
            "utc": now.to_rfc3339(),
            "utc_date": now.format("%Y-%m-%d").to_string(),
            "utc_time": now.format("%H:%M:%S").to_string(),
            "local": chrono::Local::now().to_rfc3339(),
            "local_timezone": chrono::Local::now().offset().to_string(),
            "unix_ms": now.timestamp_millis(),
        }))
    }
}

// --------------------------------------------------------------- sys.info (L0)

struct SysInfo {
    descriptor: ToolDescriptor,
}

impl Tool for SysInfo {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    fn execute(&self, _args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0);
        let hostname = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "unknown".into());
        let user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default();
        let os = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        };
        // Working-set of this process only — we never report figures we cannot measure.
        let jarvis_rss_mb = process_rss_mb();
        Ok(json!({
            "hostname": hostname,
            "user": user,
            "os": os,
            "os_family": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "cpu_count": cpus,
            "cwd": std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
            "project_root": ctx.project_root.display().to_string(),
            "jarvis_version": env!("CARGO_PKG_VERSION"),
            "jarvis_rss_mb": jarvis_rss_mb,
        }))
    }
}

/// Resident memory of the JARVIS process on Windows, via `wmic`-free PowerShell.
fn process_rss_mb() -> Option<u64> {
    if !cfg!(target_os = "windows") {
        return None;
    }
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "(Get-Process -Id $PID).WorkingSet64/1MB",
        ])
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse::<f64>()
            .ok()
            .map(|v| v as u64),
        Err(_) => None,
    }
}

// ----------------------------------------------------------------- fs.list (L1)

struct FsList {
    descriptor: ToolDescriptor,
}

impl Tool for FsList {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid("`path` (string) is required"))?;
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(200).min(1000) as usize;
        let target = ctx.resolve_within_roots(path)?;

        if !target.exists() {
            return Err(ToolError::not_found(format!("no such path: {}", target.display())));
        }
        if target.is_file() {
            let meta = std::fs::metadata(&target)?;
            return Ok(json!({
                "path": target.display().to_string(),
                "is_dir": false,
                "entries": [],
                "note": "path is a file, not a directory",
                "size": meta.len(),
            }));
        }

        let mut entries = Vec::new();
        let mut total = 0usize;
        let rd = std::fs::read_dir(&target).map_err(|e| ToolError::failed(format!("cannot list: {e}")))?;
        for entry in rd {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            total += 1;
            if entries.len() >= limit {
                continue;
            }
            let meta = entry.metadata().ok();
            entries.push(json!({
                "name": entry.file_name().to_string_lossy(),
                "is_dir": meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
                "modified": meta.as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
            }));
        }
        entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        Ok(json!({
            "path": target.display().to_string(),
            "is_dir": true,
            "count": entries.len(),
            "total": total,
            "truncated": total > entries.len(),
            "entries": entries,
        }))
    }
}

// ---------------------------------------------------------------- fs.read (L1)

struct FsRead {
    descriptor: ToolDescriptor,
}

impl Tool for FsRead {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid("`path` (string) is required"))?;
        let max_bytes = args
            .get("max_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(64 * 1024)
            .min(1024 * 1024);
        let target = ctx.resolve_within_roots(path)?;
        if !target.exists() {
            return Err(ToolError::not_found(format!("no such file: {}", target.display())));
        }
        if target.is_dir() {
            return Err(ToolError::invalid(format!(
                "{} is a directory — use fs.list",
                target.display()
            )));
        }
        let meta = std::fs::metadata(&target)?;
        if meta.len() > 64 * 1024 * 1024 {
            return Err(ToolError::invalid(format!(
                "file is {} MB; reading it whole is not supported — use fs.search or narrow the read",
                meta.len() / (1024 * 1024)
            )));
        }
        let bytes = std::fs::read(&target)?;
        let truncated = bytes.len() as u64 > max_bytes;
        let slice = if truncated { &bytes[..max_bytes as usize] } else { &bytes[..] };
        let content = String::from_utf8_lossy(slice);
        Ok(json!({
            "path": target.display().to_string(),
            "size": meta.len(),
            "bytes_read": slice.len(),
            "truncated": truncated,
            "content": content,
        }))
    }
}

// -------------------------------------------------------------- fs.search (L1)

struct FsSearch {
    descriptor: ToolDescriptor,
}

impl Tool for FsSearch {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid("`query` (string) is required"))?
            .to_lowercase();
        if query.is_empty() {
            return Err(ToolError::invalid("`query` must not be empty"));
        }
        let root = args
            .get("root")
            .and_then(Value::as_str)
            .map(|r| r.to_string())
            .unwrap_or_else(|| ".".into());
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(100).min(500) as usize;
        let max_depth = args.get("max_depth").and_then(Value::as_u64).unwrap_or(6).min(24) as usize;
        let start = ctx.resolve_within_roots(&root)?;

        const SKIP: &[&str] = &[".git", "target", "node_modules", ".venv", "venv", "__pycache__", ".cache", "logs", "dist", "build"];
        let mut hits = Vec::new();
        let mut scanned = 0usize;
        let mut stack = vec![(start.clone(), 0usize)];

        while let Some((dir, depth)) = stack.pop() {
            if ctx.cancel.is_cancelled() {
                return Err(ToolError::new("cancelled", "search cancelled"));
            }
            if depth > max_depth || hits.len() >= limit {
                continue;
            }
            let rd = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            for entry in rd.flatten() {
                if hits.len() >= limit {
                    break;
                }
                scanned += 1;
                if scanned > 50_000 {
                    break;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if is_dir {
                    if SKIP.contains(&name.as_str()) || name.starts_with('.') {
                        continue;
                    }
                    stack.push((path, depth + 1));
                } else if name.to_lowercase().contains(&query) {
                    let meta = entry.metadata().ok();
                    hits.push(json!({
                        "path": path.display().to_string(),
                        "name": name,
                        "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
                        "modified": meta.as_ref()
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs()),
                    }));
                }
            }
        }
        hits.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        Ok(json!({
            "query": query,
            "root": start.display().to_string(),
            "count": hits.len(),
            "truncated": hits.len() >= limit,
            "scanned_entries": scanned,
            "matches": hits,
        }))
    }
}

// --------------------------------------------------------------- app.list (L1)

struct AppList {
    descriptor: ToolDescriptor,
}

impl Tool for AppList {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        if !cfg!(target_os = "windows") {
            return Err(ToolError::failed("app.list is only implemented for Windows in Phase 1"));
        }
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(60).min(500) as usize;
        let _ = ctx;
        let out = std::process::Command::new("tasklist")
            .args(["/FO", "CSV", "/NH"])
            .output()
            .map_err(|e| ToolError::failed(format!("cannot run tasklist: {e}")))?;
        if !out.status.success() {
            return Err(ToolError::failed("tasklist failed"));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut rows = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let fields = parse_csv_line(line);
            if fields.len() < 2 {
                continue;
            }
            rows.push(json!({
                "name": fields[0],
                "pid": fields[1],
                "memory_kb": fields.get(4).map(|s| s.replace([' ', ','], "").parse::<u64>().ok()).flatten(),
            }));
            if rows.len() >= limit {
                break;
            }
        }
        Ok(json!({
            "count": rows.len(),
            "truncated": rows.len() >= limit,
            "processes": rows,
        }))
    }
}

/// Minimal CSV parser for `tasklist` output (quoted fields, escaped quotes).
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_quotes && chars.peek() == Some(&'"') {
                    chars.next();
                    cur.push('"');
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ',' if !in_quotes => out.push(std::mem::take(&mut cur)),
            other => cur.push(other),
        }
    }
    out.push(cur);
    out
}

/// Register every built-in tool (Phase 1 + Phase 3).
pub fn register_all(registry: &mut ToolRegistry) -> Result<(), ToolError> {
    crate::builtin_ext::register_all(registry)?;
    crate::builtin_ui::register_all(registry)?;
    crate::builtin_memory::register_all(registry)?;
    registry.register(Arc::new(TimeNow {
        descriptor: base(
            "time.now",
            "Current date and time in UTC and local time. Use before any scheduling or 'today' reasoning.",
            json!({"type": "object", "properties": {}, "additionalProperties": false}),
            json!({"type": "object"}),
            PermissionLevel::Level0,
            Risk::Low,
            2_000,
        ),
    }))?;

    registry.register(Arc::new(SysInfo {
        descriptor: base(
            "sys.info",
            "Basic facts about this computer: hostname, OS, architecture, CPU count, project root. Read-only.",
            json!({"type": "object", "properties": {}, "additionalProperties": false}),
            json!({"type": "object"}),
            PermissionLevel::Level0,
            Risk::Low,
            10_000,
        ),
    }))?;

    registry.register(Arc::new(FsList {
        descriptor: base(
            "fs.list",
            "List the entries of a directory (or describe a single file). Paths are confined to the allowed roots.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory path, absolute or relative to the project root"},
                    "limit": {"type": "integer", "description": "Max entries to return (default 200)"}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            json!({"type": "object"}),
            PermissionLevel::Level1,
            Risk::Low,
            15_000,
        ),
    }))?;

    registry.register(Arc::new(FsRead {
        descriptor: base(
            "fs.read",
            "Read a UTF-8 text file (truncated at max_bytes, default 64 KB). Use for source files, reports, logs.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "max_bytes": {"type": "integer", "description": "Max bytes to return (default 65536, max 1048576)"}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            json!({"type": "object"}),
            PermissionLevel::Level1,
            Risk::Low,
            15_000,
        ),
    }))?;

    registry.register(Arc::new(FsSearch {
        descriptor: base(
            "fs.search",
            "Find files whose name contains a query (case-insensitive substring), skipping build/dependency folders.",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "root": {"type": "string", "description": "Directory to search from (default: project root)"},
                    "limit": {"type": "integer"},
                    "max_depth": {"type": "integer"}
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            json!({"type": "object"}),
            PermissionLevel::Level1,
            Risk::Low,
            30_000,
        ),
    }))?;

    registry.register(Arc::new(AppList {
        descriptor: base(
            "app.list",
            "List running processes on Windows (name, pid, memory). Use for 'what is using my RAM' style questions.",
            json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer"}
                },
                "additionalProperties": false
            }),
            json!({"type": "object"}),
            PermissionLevel::Level1,
            Risk::Low,
            15_000,
        ),
    }))?;

    Ok(())
}

use std::sync::Arc;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{AutonomyLevel, Confirmer, NeverConfirm, PermissionGate};
    use Jake_protocol::tool::ToolExecution;
    use std::time::Duration;

    fn registry() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        register_all(&mut r).unwrap();
        r
    }

    #[test]
    fn all_builtins_register_without_name_clashes() {
        let r = registry();
        assert!(r.len() >= 6, "expected >= 6 tools, got {}", r.len());
        for name in ["time.now", "sys.info", "fs.list", "fs.read", "fs.search", "app.list"] {
            assert!(r.get(name).is_some(), "missing {name}");
        }
    }

    #[test]
    fn builtins_declare_sane_permission_levels() {
        let r = registry();
        for d in r.descriptors() {
            // No built-in tool is ever level 4 (critical) — that class belongs to
            // future integrations and must be added deliberately.
            assert!(
                d.permission_level <= PermissionLevel::Level3,
                "{} must not be level 4 by default",
                d.name
            );
            assert!(!d.description.is_empty(), "{} needs a description", d.name);
            assert!(d.timeout_ms > 0);
        }
        // Read-only information tools stay low-privilege.
        for name in ["time.now", "sys.info", "fs.list", "fs.read", "fs.search", "app.list"] {
            let d = r.descriptors().into_iter().find(|d| d.name == name).unwrap();
            assert!(
                d.permission_level <= PermissionLevel::Level1,
                "{name} should be level 0/1"
            );
        }
    }

    #[tokio::test]
    async fn time_now_returns_rfc3339_and_is_executable() {
        let r = registry();
        let gate = PermissionGate::new(AutonomyLevel::new(3).unwrap());
        let confirmer: std::sync::Arc<dyn Confirmer> = std::sync::Arc::new(NeverConfirm);
        let res = r
            .dispatch(
                ToolExecution { id: "1".into(), name: "time.now".into(), arguments: json!({}) },
                &ToolContext::new(std::env::temp_dir()),
                &gate,
                &confirmer,
                Duration::from_secs(5),
            )
            .await;
        assert!(res.ok, "{res:?}");
        assert!(res.data.unwrap()["utc"].as_str().unwrap().contains('T'));
    }

    #[tokio::test]
    async fn fs_list_reads_a_real_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "hi").unwrap();
        let mut ctx = ToolContext::new(dir.path().to_path_buf());
        ctx.roots = vec![dir.path().to_path_buf()];
        let r = registry();
        let gate = PermissionGate::new(AutonomyLevel::new(3).unwrap());
        let confirmer: std::sync::Arc<dyn Confirmer> = std::sync::Arc::new(NeverConfirm);
        let res = r
            .dispatch(
                ToolExecution {
                    id: "1".into(),
                    name: "fs.list".into(),
                    arguments: json!({"path": "."}),
                },
                &ctx,
                &gate,
                &confirmer,
                Duration::from_secs(5),
            )
            .await;
        assert!(res.ok, "{res:?}");
        let data = res.data.unwrap();
        assert_eq!(data["count"], 1);
        assert_eq!(data["entries"][0]["name"], "hello.txt");
    }

    #[tokio::test]
    async fn fs_read_returns_content_and_honours_max_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.txt");
        std::fs::write(&file, "0123456789").unwrap();
        let mut ctx = ToolContext::new(dir.path().to_path_buf());
        ctx.roots = vec![dir.path().to_path_buf()];
        let r = registry();
        let gate = PermissionGate::new(AutonomyLevel::new(3).unwrap());
        let confirmer: std::sync::Arc<dyn Confirmer> = std::sync::Arc::new(NeverConfirm);
        let res = r
            .dispatch(
                ToolExecution {
                    id: "1".into(),
                    name: "fs.read".into(),
                    arguments: json!({"path": "note.txt", "max_bytes": 4}),
                },
                &ctx,
                &gate,
                &confirmer,
                Duration::from_secs(5),
            )
            .await;
        assert!(res.ok, "{res:?}");
        let data = res.data.unwrap();
        assert_eq!(data["content"], "0123");
        assert_eq!(data["truncated"], true);
    }

    #[tokio::test]
    async fn fs_read_of_missing_file_reports_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ToolContext::new(dir.path().to_path_buf());
        ctx.roots = vec![dir.path().to_path_buf()];
        let r = registry();
        let gate = PermissionGate::new(AutonomyLevel::new(3).unwrap());
        let confirmer: std::sync::Arc<dyn Confirmer> = std::sync::Arc::new(NeverConfirm);
        let res = r
            .dispatch(
                ToolExecution {
                    id: "1".into(),
                    name: "fs.read".into(),
                    arguments: json!({"path": "missing.txt"}),
                },
                &ctx,
                &gate,
                &confirmer,
                Duration::from_secs(5),
            )
            .await;
        assert!(!res.ok);
        assert_eq!(res.error_code.as_deref(), Some("not_found"));
        assert!(res.error.unwrap().contains("no such file"));
    }

    #[test]
    fn csv_parser_handles_quotes_and_commas() {
        let fields = parse_csv_line(r#""chrome.exe","1234","Console","1","50,123 K""#);
        assert_eq!(fields[0], "chrome.exe");
        assert_eq!(fields[1], "1234");
        assert_eq!(fields[4], "50,123 K");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn app_list_returns_real_processes() {
        let r = registry();
        let gate = PermissionGate::new(AutonomyLevel::new(3).unwrap());
        let confirmer: std::sync::Arc<dyn Confirmer> = std::sync::Arc::new(NeverConfirm);
        let res = r
            .dispatch(
                ToolExecution { id: "1".into(), name: "app.list".into(), arguments: json!({"limit": 10}) },
                &ToolContext::new(std::env::temp_dir()),
                &gate,
                &confirmer,
                Duration::from_secs(10),
            )
            .await;
        assert!(res.ok, "{res:?}");
        assert!(res.data.unwrap()["count"].as_u64().unwrap() > 0);
    }
}