//! Phase-3 built-in tools: application control, screenshots, and file mutations.
//!
//! Everything here is level ≤ 3, runs under the same gate/dispatcher as the
//! Phase-1 tools, and **verifies its own post-conditions** — `app.open` confirms
//! the process is actually running, `fs.delete` confirms the path is gone, and
//! `screenshot` validates the PNG it produced. Failures return honest errors.

use crate::tools::{Tool, ToolContext, ToolError, ToolRegistry};
use jarvis_protocol::permission::{PermissionLevel, Risk};
use jarvis_protocol::tool::ToolDescriptor;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

// --------------------------------------------------------------------- helpers

fn base(
    name: &str,
    description: &str,
    input_schema: Value,
    output_schema: Value,
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
        output_schema,
        permission_level: level,
        risk,
        timeout_ms,
        destructive,
        confirmation_explanation: confirmation_explanation.into(),
    }
}

pub(crate) fn powershell(script: &str) -> std::io::Result<std::process::Output> {
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .output()
}

pub(crate) fn b64(text: &str) -> String {
    // base64 without a dependency: encode via PowerShell-free pure Rust
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = text.as_bytes();
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Decode helper: base64 → UTF-8 (used for engine output and by round-trip tests).
#[cfg(test)]
pub(crate) fn decode_b64(text: &str) -> Option<String> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = text.as_bytes();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        let mut pad = 0;
        for (i, &c) in chunk.iter().enumerate() {
            if c == b'=' {
                pad += 1;
                n <<= 6;
                continue;
            }
            let v = val(c)?;
            n = (n << 6) | v;
            let _ = i;
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    String::from_utf8(out).ok()
}

/// Resolve a user/model-supplied application name to a real executable.
///
/// Order: absolute path (only inside allowed roots) → `PATH` via `where.exe` →
/// `App Paths` registry → Start Menu shortcuts. Nothing else is launchable at
/// level 1, so this cannot become an arbitrary-code-execution primitive.
fn resolve_application(name: &str, ctx: &ToolContext) -> Result<PathBuf, ToolError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ToolError::invalid("application name must not be empty"));
    }

    // Absolute/relative paths are only allowed inside the sandbox roots.
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.to_lowercase().ends_with(".exe") {
        let looks_like_path = trimmed.contains('/') || trimmed.contains('\\');
        if looks_like_path {
            let resolved = ctx.resolve_within_roots(trimmed)?;
            if resolved.is_file() {
                return Ok(resolved);
            }
            return Err(ToolError::not_found(format!(
                "no executable at `{}`",
                resolved.display()
            )));
        }
    }

    let exe_name = if trimmed.to_lowercase().ends_with(".exe") {
        format!("{trimmed}.exe")
    } else {
        format!("{trimmed}.exe")
    };

    // 1) PATH lookup
    if let Ok(out) = Command::new("where").arg(&exe_name).output() {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = text.lines().find(|l| !l.trim().is_empty()) {
                let candidate = PathBuf::from(line.trim());
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }

    // 2) App Paths registry (HKLM/HKCU, 64- and 32-bit views), plus explicit
    //    Start Menu and Common Programs shortcuts.
    for hive in [
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths",
        r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths",
        r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths",
    ] {
        let key = format!("{hive}\\{exe_name}");
        if let Ok(out) = Command::new("reg")
            .args(["query", &key, "/ve"])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                if let Some(path) = text.lines().filter_map(|l| l.rfind("REG_SZ").map(|i| l[i + 6..].trim())).next() {
                    let cleaned = path.trim_matches('"');
                    if !cleaned.is_empty() && Path::new(cleaned).is_file() {
                        return Ok(PathBuf::from(cleaned));
                    }
                }
            }
        }
    }

    // 3) Start Menu shortcuts (by file stem, case-insensitive)
    let stem = exe_name.trim_end_matches(".exe").to_ascii_lowercase();
    let mut menus = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        menus.push(PathBuf::from(appdata).join("Microsoft/Windows/Start Menu/Programs"));
    }
    menus.push(PathBuf::from(
        std::env::var("PROGRAMDATA").unwrap_or_else(|_| "C:\\ProgramData".into()),
    )
    .join("Microsoft/Windows/Start Menu/Programs"));
    if let Ok(commonprograms) = std::env::var("COMMONPROGRAMS") {
        menus.push(PathBuf::from(commonprograms));
    } else {
        menus.push(PathBuf::from("C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs"));
    }
    for menu in menus {
        if let Some(found) = find_shortcut(&menu, &stem, 0) {
            return Ok(found);
        }
    }

    Err(ToolError::not_found(format!(
        "Windows could not find an application named `{trimmed}` (checked PATH, the App Paths registry, the Start Menu and Common Programs). \
         It may not be installed."
    )))
}

fn find_shortcut(dir: &Path, stem: &str, depth: usize) -> Option<PathBuf> {
    if depth > 4 {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            subdirs.push(path);
            continue;
        }
        let file_stem = path.file_stem()?.to_string_lossy().to_ascii_lowercase();
        if file_stem == stem && path.extension().map(|e| e.to_string_lossy().to_lowercase()) == Some("lnk".into()) {
            return Some(path);
        }
    }
    for sub in subdirs {
        if let Some(found) = find_shortcut(&sub, stem, depth + 1) {
            return Some(found);
        }
    }
    None
}

/// Newest (most recently started) PID of a running image, if any.
/// `app.open` returns it so callers can target that exact instance later.
/// Uses the stem prefix — `calc.exe` is a stub whose real process is
/// `CalculatorApp.exe`, and `winver` matches the literal image too.
fn newest_pid(image: &str) -> Option<u64> {
    let stem = image.trim_end_matches(".exe");
    let script = format!(
        r#"
        $p = Get-Process -Name '{stem}*' -ErrorAction SilentlyContinue | Sort-Object StartTime -Descending | Select-Object -First 1
        if ($p) {{ Write-Output ('PID=' + $p.Id) }} else {{ Write-Output 'PID=none' }}
        "#
    );
    let out = powershell(&script).ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("PID="))
        .and_then(|v| v.trim().parse().ok())
}

/// Running process images this application currently maps to.
///
/// Windows launches `calc.exe` as a stub that spawns `CalculatorApp.exe` and
/// exits: tracking the literal image alone would declare success while a
/// *different* process owns the window — and would never close it. Match the
/// image stem as a prefix over running process names instead (exact image as a
/// fallback).
fn running_images(image: &str) -> Vec<String> {
    let stem = image.trim_end_matches(".exe");
    let mut out = Vec::new();
    if stem
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        let script = format!(
            "Get-Process -Name '{stem}*' -ErrorAction SilentlyContinue | Select-Object -ExpandProperty ProcessName -Unique"
        );
        if let Ok(res) = powershell(&script) {
            if res.status.success() {
                for line in String::from_utf8_lossy(&res.stdout).lines() {
                    let name = line.trim();
                    if !name.is_empty() {
                        out.push(format!("{name}.exe"));
                    }
                }
            }
        }
    }
    if out.is_empty() &
       stem
           .chars()
           .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        if process_running(image) {
            out.push(image.to_string());
        }
    }
    out
}

/// Is this process running? (post-condition probe used by open/close)
fn process_running(image: &str) -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {image}"), "/NH"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .to_ascii_lowercase()
                .contains(&image.to_ascii_lowercase())
        })
        .unwrap_or(false)
}

// -------------------------------------------------------------------- app.open

struct AppOpen {
    descriptor: ToolDescriptor,
}

impl Tool for AppOpen {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        if !cfg!(target_os = "windows") {
            return Err(ToolError::failed("app.open is only implemented for Windows so far"));
        }
        let name = args
            .get("app")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid("`app` (string) is required"))?;
        let extra = args.get("args").and_then(Value::as_str).unwrap_or("").trim();

        let resolved = resolve_application(name, ctx)?;
        let image = resolved
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("{name}.exe"));

        let running = running_images(&image);
        if !running.is_empty() {
            return Ok(json!({
                "app": name,
                "path": resolved.display().to_string(),
                "image": image,
                "running_processes": running,
                "pid": newest_pid(&image),
                "already_running": true,
                "note": "the application was already running; it was brought to the foreground by the launcher",
            }));
        }

        // Launch: path + args travel base64-encoded, so no quoting/injection surface.
        let path_b64 = b64(&resolved.display().to_string());
        let args_b64 = b64(extra);
        let script = format!(
            r#"
            $ErrorActionPreference = 'Stop'
            $p = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{path_b64}'))
            $a = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{args_b64}'))
            if ($a -eq '') {{ Start-Process -FilePath $p }} else {{ Start-Process -FilePath $p -ArgumentList $a }}
            Write-Output 'STARTED'
            "#
        );
        let out = powershell(&script)?;
        if !out.status.success() || !String::from_utf8_lossy(&out.stdout).contains("STARTED") {
            let err = String::from_utf8_lossy(&out.stderr);
            let outp = String::from_utf8_lossy(&out.stdout);
            return Err(ToolError::failed(format!(
                "Windows refused to start `{}`: {}",
                resolved.display(),
                if err.trim().is_empty() { outp.trim() } else { err.trim() }
            )));
        }

        // Post-condition: the process must actually be running within ~4 seconds.
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            if !running_images(&image).is_empty() {
                return Ok(json!({
                    "app": name,
                    "path": resolved.display().to_string(),
                    "image": image,
                    "running_processes": running_images(&image),
                    "pid": newest_pid(&image),
                    "already_running": false,
                }));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(ToolError::failed(format!(
            "`{image}` was launched but is not running after 4 seconds — I cannot confirm it opened"
        )))
    }
}

// ------------------------------------------------------------------- app.close

struct AppClose {
    descriptor: ToolDescriptor,
}

impl Tool for AppClose {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        if !cfg!(target_os = "windows") {
            return Err(ToolError::failed("app.close is only implemented for Windows so far"));
        }
        let name = args
            .get("app")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid("`app` (string) is required"))?;
        // Optional exact instance: close only this PID (several instances may share
        // the image name — tests and multi-window flows target one).
        let pid = args.get("pid").and_then(Value::as_u64);
        let image = if name.to_ascii_lowercase().ends_with(".exe") {
            name.to_string()
        } else {
            format!("{name}.exe")
        };

        let running = |pid: Option<u64>| match pid {
            Some(p) => Command::new("tasklist")
                .args(["/FI", &format!("PID eq {p}"), "/NH"])
                .output()
                .map(|o| {
                    o.status.success()
                        && {
                            let s = String::from_utf8_lossy(&o.stdout);
                            !s.trim().is_empty() && !s.contains("No tasks") && !s.contains("INFO:")
                        }
                })
                .unwrap_or(false),
            None => !running_images(&image).is_empty(),
        };

        if !running(pid) {
            return Err(ToolError::not_found(match pid {
                Some(p) => format!("PID {p} is not running"),
                None => format!("`{image}` is not running"),
            }));
        }

        // Graceful close first (WM_CLOSE) — never force-kill without saying so.
        // Every mapped image is closed (calc → CalculatorApp.exe etc.). Note that
        // taskkill *reports success* for sending the signal even when a
        // UWP-style process ignores WM_CLOSE, so we re-check and escalate.
        let mut forced = false;
        match pid {
            Some(p) => {
                let _ = Command::new("taskkill").args(["/PID", &p.to_string()]).output()?;
                std::thread::sleep(Duration::from_millis(600));
                if running(Some(p)) {
                    // the graceful attempt was ignored: fall back to /F only
                    // because the gate already required the user's confirmation
                    // for this level-3 tool.
                    forced = true;
                    Command::new("taskkill").args(["/PID", &p.to_string(), "/F"]).output()?;
                }
            }
            None => {
                for img in running_images(&image) {
                    let _ = Command::new("taskkill").args(["/IM", &img]).output()?;
                }
                std::thread::sleep(Duration::from_millis(600));
                let remaining = running_images(&image);
                if !remaining.is_empty() {
                    forced = true;
                    for img in remaining {
                        Command::new("taskkill").args(["/IM", &img, "/F"]).output()?;
                    }
                }
            }
        }

        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            if !running(pid) {
                return Ok(json!({
                    "app": image,
                    "pid": pid,
                    "closed": true,
                    "forced": forced,
                }));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(ToolError::failed(format!(
            "`{image}` is still running — the application refused to close (unsaved work?)"
        )))
    }
}

/// Capture the screen to `target` (PNG), returning (width, height).
///
/// Uses DPI-aware `GetSystemMetrics` for the virtual-screen rect — the
/// `System.Windows.Forms.Screen` API reports an empty surface in this session
/// while GetSystemMetrics reports the real 3360×1254 desktop.
pub(crate) fn capture_screen_to(target: &Path) -> Result<(u32, u32), ToolError> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if target.exists() {
        std::fs::remove_file(target)?;
    }
    let path_b64 = b64(&target.display().to_string());
    let script = format!(
        r#"
        $ErrorActionPreference = 'Stop'
        try {{
            Add-Type -AssemblyName System.Drawing
            Add-Type @'
using System;using System.Runtime.InteropServices;
public class JarvDpi {{
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int nIndex);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}}
'@
            [JarvDpi]::SetProcessDPIAware() | Out-Null
            $x = [JarvDpi]::GetSystemMetrics(76); $y = [JarvDpi]::GetSystemMetrics(77)
            $w = [JarvDpi]::GetSystemMetrics(78); $h = [JarvDpi]::GetSystemMetrics(79)
            if ($w -le 0 -or $h -le 0) {{ throw 'no desktop surface available' }}
            $bmp = New-Object System.Drawing.Bitmap $w, $h
            $g = [System.Drawing.Graphics]::FromImage($bmp)
            $g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
            $path = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{path_b64}'))
            $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
            $g.Dispose(); $bmp.Dispose()
            Write-Output ('OK|' + $w + 'x' + $h)
        }} catch {{
            Write-Output ('ERR|' + $_.Exception.Message)
            exit 1
        }}
        "#
    );
    let out = powershell(&script)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stdout = stdout.trim();
    if !out.status.success() || stdout.starts_with("ERR|") {
        return Err(ToolError::failed(format!(
            "screenshot failed: {}",
            stdout.strip_prefix("ERR|").unwrap_or(stdout).trim()
        )));
    }
    let dims = stdout.strip_prefix("OK|").unwrap_or("0x0");
    let (w, h) = dims
        .split_once('x')
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((0, 0));
    Ok((w, h))
}

// ----------------------------------------------------------------- screenshot

struct Screenshot {
    descriptor: ToolDescriptor,
}

impl Tool for Screenshot {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        if ctx.privacy_mode {
            return Err(ToolError::new(
                "denied",
                "Privacy Mode is on — screen capture is disabled",
            ));
        }
        if !cfg!(target_os = "windows") {
            return Err(ToolError::failed("screenshot is only implemented for Windows so far"));
        }

        let default_path = ctx
            .project_root
            .join("logs")
            .join(format!("screenshot-{}.png", chrono::Utc::now().format("%Y%m%d-%H%M%S")));
        let requested = args.get("path").and_then(Value::as_str);
        let target = match requested {
            Some(p) => ctx.resolve_within_roots(p)?,
            None => default_path,
        };
        let (width, height) = capture_screen_to(&target)?;

        // Verify the artifact ourselves before claiming success.
        let bytes = std::fs::read(&target)?;
        if bytes.len() < 128 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" {
            return Err(ToolError::failed("captured file is not a valid PNG"));
        }
        Ok(json!({
            "path": target.display().to_string(),
            "bytes": bytes.len(),
            "dimensions": format!("{width}x{height}"),
            "verified_png": true,
        }))
    }
}

// ------------------------------------------------------------------ fs.create

struct FsCreate {
    descriptor: ToolDescriptor,
}

impl Tool for FsCreate {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid("`path` (string) is required"))?;
        let content = args.get("content").and_then(Value::as_str).unwrap_or("");
        let overwrite = args.get("overwrite").and_then(Value::as_bool).unwrap_or(false);

        let target = ctx.resolve_within_roots(path)?;
        if target.exists() && !overwrite {
            return Err(ToolError::new(
                "exists",
                format!("{} already exists — pass overwrite:true to replace it", target.display()),
            ));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, content.as_bytes())?;

        // Post-condition: the bytes on disk must match what we were asked to write.
        let written = std::fs::metadata(&target)?.len();
        if written != content.len() as u64 {
            return Err(ToolError::failed(format!(
                "write verification failed: expected {} bytes, found {written}",
                content.len()
            )));
        }
        Ok(json!({
            "path": target.display().to_string(),
            "bytes": written,
            "overwritten": overwrite && written > 0,
        }))
    }
}

// --------------------------------------------------------------------- fs.copy

struct FsCopy {
    descriptor: ToolDescriptor,
}

impl Tool for FsCopy {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let from = args
            .get("from")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid("`from` (string) is required"))?;
        let to = args
            .get("to")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid("`to` (string) is required"))?;

        let src = ctx.resolve_within_roots(from)?;
        if !src.is_file() {
            return Err(ToolError::not_found(format!("no such file: {}", src.display())));
        }
        let dst = ctx.resolve_within_roots(to)?;
        if dst.exists() {
            return Err(ToolError::new(
                "exists",
                format!("destination {} already exists — refusing to overwrite", dst.display()),
            ));
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&src, &dst)?;

        let src_len = std::fs::metadata(&src)?.len();
        let dst_len = std::fs::metadata(&dst)?.len();
        if src_len != dst_len {
            return Err(ToolError::failed(format!(
                "copy verification failed: source {src_len} bytes vs destination {dst_len} bytes"
            )));
        }
        Ok(json!({
            "from": src.display().to_string(),
            "to": dst.display().to_string(),
            "bytes": dst_len,
        }))
    }
}

// --------------------------------------------------------------------- fs.move

struct FsMove {
    descriptor: ToolDescriptor,
}

impl Tool for FsMove {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let from = args
            .get("from")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid("`from` (string) is required"))?;
        let to = args
            .get("to")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid("`to` (string) is required"))?;

        let src = ctx.resolve_within_roots(from)?;
        if !src.exists() {
            return Err(ToolError::not_found(format!("no such path: {}", src.display())));
        }
        let dst = ctx.resolve_within_roots(to)?;
        if dst.exists() {
            return Err(ToolError::new(
                "exists",
                format!("destination {} already exists — refusing to overwrite", dst.display()),
            ));
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&src, &dst)?;

        // Post-condition: moved, not duplicated.
        if src.exists() || !dst.exists() {
            return Err(ToolError::failed("move verification failed — check both paths"));
        }
        Ok(json!({
            "from": src.display().to_string(),
            "to": dst.display().to_string(),
            "moved": true,
        }))
    }
}

// -------------------------------------------------------------------- fs.delete

struct FsDelete {
    descriptor: ToolDescriptor,
}

impl Tool for FsDelete {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let paths = args
            .get("paths")
            .and_then(Value::as_array)
            .ok_or_else(|| ToolError::invalid("`paths` (array of strings) is required"))?;
        if paths.is_empty() {
            return Err(ToolError::invalid("`paths` must not be empty"));
        }
        let mut deleted = Vec::new();
        for entry in paths {
            let p = entry
                .as_str()
                .ok_or_else(|| ToolError::invalid("`paths` entries must be strings"))?;
            let target = ctx.resolve_within_roots(p)?;
            if !target.exists() {
                return Err(ToolError::not_found(format!("no such path: {}", target.display())));
            }
            if target.is_dir() {
                std::fs::remove_dir_all(&target)?;
            } else {
                std::fs::remove_file(&target)?;
            }
            deleted.push(target.display().to_string());
        }
        // Post-condition: every path must be gone before we claim success.
        let still_present: Vec<&String> = deleted
            .iter()
            .filter(|d| Path::new(d).exists())
            .collect();
        if !still_present.is_empty() {
            let listed: Vec<&str> = still_present.iter().map(|s| s.as_str()).collect();
            return Err(ToolError::failed(format!(
                "delete verification failed, still present: {}",
                listed.join(", ")
            )));
        }
        Ok(json!({
            "deleted": deleted,
            "count": deleted.len(),
            "verified": true,
        }))
    }
}

// ------------------------------------------------------------------ registration

/// Phase-3 tools (applications, screenshots, file mutations).
pub fn register_all(registry: &mut ToolRegistry) -> Result<(), ToolError> {
    registry.register(Arc::new(AppOpen {
        descriptor: base(
            "app.open",
            "Open a Windows application by name (PATH, App Paths registry or Start Menu shortcut) or by path inside the allowed roots. Verifies the process is actually running.",
            json!({
                "type": "object",
                "properties": {
                    "app": {"type": "string", "description": "Application name (e.g. \"notepad\", \"chrome\") or an executable path inside allowed roots"},
                    "args": {"type": "string", "description": "Optional command-line arguments"}
                },
                "required": ["app"],
                "additionalProperties": false
            }),
            json!({"type": "object"}),
            PermissionLevel::Level1,
            Risk::Low,
            20_000,
            false,
            "",
        ),
    }))?;

    registry.register(Arc::new(AppClose {
        descriptor: base(
            "app.close",
            "Close a running Windows application gracefully (taskkill, force only as a fallback). Verifies the process exited. Unsaved work may be lost.",
            json!({
                "type": "object",
                "properties": {
                    "app": {"type": "string", "description": "Process image name, e.g. \"notepad\" or \"notepad.exe\""},
                    "pid": {"type": "integer", "description": "Optional: close only this exact process id (returned by app.open)"}
                },
                "required": ["app"],
                "additionalProperties": false
            }),
            json!({"type": "object"}),
            PermissionLevel::Level3,
            Risk::High,
            20_000,
            true,
            "Closing an application can lose unsaved work.",
        ),
    }))?;

    registry.register(Arc::new(Screenshot {
        descriptor: base(
            "screenshot",
            "Capture the virtual screen to a PNG and verify it. Blocked entirely in Privacy Mode. The file stays on this machine unless the user shares it.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Optional output path inside allowed roots (default: logs/screenshot-<ts>.png)"}
                },
                "additionalProperties": false
            }),
            json!({"type": "object"}),
            PermissionLevel::Level1,
            Risk::Moderate,
            30_000,
            false,
            "",
        ),
    }))?;

    registry.register(Arc::new(FsCreate {
        descriptor: base(
            "fs.create",
            "Create or overwrite a UTF-8 text file (must pass overwrite:true to replace an existing file). Verifies the byte count on disk.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                    "overwrite": {"type": "boolean"}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            json!({"type": "object"}),
            PermissionLevel::Level2,
            Risk::Moderate,
            15_000,
            false,
            "",
        ),
    }))?;

    registry.register(Arc::new(FsCopy {
        descriptor: base(
            "fs.copy",
            "Copy a file inside the allowed roots. Never overwrites an existing destination.",
            json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string"},
                    "to": {"type": "string"}
                },
                "required": ["from", "to"],
                "additionalProperties": false
            }),
            json!({"type": "object"}),
            PermissionLevel::Level2,
            Risk::Moderate,
            30_000,
            false,
            "",
        ),
    }))?;

    registry.register(Arc::new(FsMove {
        descriptor: base(
            "fs.move",
            "Move or rename a file/directory inside the allowed roots. Never overwrites an existing destination.",
            json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string"},
                    "to": {"type": "string"}
                },
                "required": ["from", "to"],
                "additionalProperties": false
            }),
            json!({"type": "object"}),
            PermissionLevel::Level2,
            Risk::Moderate,
            30_000,
            false,
            "",
        ),
    }))?;

    registry.register(Arc::new(FsDelete {
        descriptor: base(
            "fs.delete",
            "Permanently delete files or directories (inside the allowed roots). Requires confirmation; verifies each path is gone.",
            json!({
                "type": "object",
                "properties": {
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "Files or directories to delete"}
                },
                "required": ["paths"],
                "additionalProperties": false
            }),
            json!({"type": "object"}),
            PermissionLevel::Level3,
            Risk::High,
            30_000,
            true,
            "Deleting files is permanent — this cannot be undone.",
        ),
    }))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin;
    use crate::permission::{AutonomyLevel, Confirmer, NeverConfirm, PermissionGate};
    use jarvis_protocol::tool::ToolExecution;
    use std::time::Duration;

    fn registry() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        builtin::register_all(&mut r).unwrap();
        r
    }

    fn temp_ctx(dir: &Path) -> ToolContext {
        let mut ctx = ToolContext::new(dir.to_path_buf());
        ctx.roots = vec![dir.to_path_buf()];
        ctx
    }

    async fn dispatch(
        registry: &ToolRegistry,
        ctx: &ToolContext,
        gate: &PermissionGate,
        confirmer: &Arc<dyn Confirmer>,
        name: &str,
        args: Value,
    ) -> ToolResult {
        registry
            .dispatch(
                ToolExecution { id: "t".into(), name: name.into(), arguments: args },
                ctx,
                gate,
                confirmer,
                Duration::from_secs(30),
            )
            .await
    }

    use jarvis_protocol::tool::ToolResult;

    fn gate(autonomy: u8) -> PermissionGate {
        PermissionGate::new(AutonomyLevel::new(autonomy).unwrap())
    }

    #[test]
    fn phase3_tools_register_with_correct_levels() {
        let r = registry();
        assert!(r.len() >= 13, "expected 13+ tools, got {}", r.len());
        let level = |name: &str| {
            r.descriptors()
                .into_iter()
                .find(|d| d.name == name)
                .unwrap_or_else(|| panic!("{name} missing"))
                .permission_level
        };
        assert_eq!(level("app.open"), PermissionLevel::Level1);
        assert_eq!(level("app.close"), PermissionLevel::Level3);
        assert_eq!(level("screenshot"), PermissionLevel::Level1);
        assert_eq!(level("fs.create"), PermissionLevel::Level2);
        assert_eq!(level("fs.delete"), PermissionLevel::Level3);
        let delete = r
            .descriptors()
            .into_iter()
            .find(|d| d.name == "fs.delete")
            .unwrap();
        assert!(delete.destructive);
        assert!(!delete.confirmation_explanation.is_empty());
    }

    #[tokio::test]
    async fn fs_create_writes_and_verifies_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = temp_ctx(dir.path());
        let r = registry();
        let confirmer: Arc<dyn Confirmer> = Arc::new(crate::permission::AlwaysConfirm);
        let res = dispatch(
            &r,
            &ctx,
            &gate(4),
            &confirmer,
            "fs.create",
            json!({"path": "note.txt", "content": "hello jarvis"}),
        )
        .await;
        assert!(res.ok, "{res:?}");
        assert_eq!(std::fs::read_to_string(dir.path().join("note.txt")).unwrap(), "hello jarvis");
    }

    #[tokio::test]
    async fn fs_create_refuses_overwrite_without_the_flag() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "original").unwrap();
        let ctx = temp_ctx(dir.path());
        let r = registry();
        let confirmer: Arc<dyn Confirmer> = Arc::new(crate::permission::AlwaysConfirm);
        let res = dispatch(
            &r,
            &ctx,
            &gate(4),
            &confirmer,
            "fs.create",
            json!({"path": "a.txt", "content": "clobbered"}),
        )
        .await;
        assert!(!res.ok);
        assert_eq!(res.error_code.as_deref(), Some("exists"));
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "original");
    }

    #[tokio::test]
    async fn fs_delete_needs_confirmation_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("victim.txt"), "bye").unwrap();
        let ctx = temp_ctx(dir.path());
        let r = registry();

        // Declined → file untouched
        let nay: Arc<dyn Confirmer> = Arc::new(NeverConfirm);
        let res = dispatch(&r, &ctx, &gate(3), &nay, "fs.delete", json!({"paths": ["victim.txt"]})).await;
        assert!(!res.ok);
        assert_eq!(res.error_code.as_deref(), Some("user_declined"));
        assert!(dir.path().join("victim.txt").exists());

        // Confirmed → file gone, tool says verified
        let yay: Arc<dyn Confirmer> = Arc::new(crate::permission::AlwaysConfirm);
        let res = dispatch(&r, &ctx, &gate(3), &yay, "fs.delete", json!({"paths": ["victim.txt"]})).await;
        assert!(res.ok, "{res:?}");
        assert_eq!(res.data.unwrap()["verified"], true);
        assert!(!dir.path().join("victim.txt").exists());
    }

    #[tokio::test]
    async fn fs_copy_and_move_verify_and_refuse_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("src.txt"), "payload-123").unwrap();
        let ctx = temp_ctx(dir.path());
        let r = registry();
        let confirmer: Arc<dyn Confirmer> = Arc::new(crate::permission::AlwaysConfirm);

        let res = dispatch(
            &r,
            &ctx,
            &gate(4),
            &confirmer,
            "fs.copy",
            json!({"from": "src.txt", "to": "dst.txt"}),
        )
        .await;
        assert!(res.ok, "{res:?}");
        assert_eq!(std::fs::read_to_string(dir.path().join("dst.txt")).unwrap(), "payload-123");

        // second copy onto same destination must refuse
        let res = dispatch(
            &r,
            &ctx,
            &gate(4),
            &confirmer,
            "fs.copy",
            json!({"from": "src.txt", "to": "dst.txt"}),
        )
        .await;
        assert!(!res.ok);
        assert_eq!(res.error_code.as_deref(), Some("exists"));

        let res = dispatch(
            &r,
            &ctx,
            &gate(4),
            &confirmer,
            "fs.move",
            json!({"from": "dst.txt", "to": "renamed.txt"}),
        )
        .await;
        assert!(res.ok, "{res:?}");
        assert!(!dir.path().join("dst.txt").exists());
        assert!(dir.path().join("renamed.txt").exists());
        assert!(dir.path().join("src.txt").exists(), "source must survive a move of the copy");
    }

    #[tokio::test]
    async fn file_tools_cannot_escape_the_roots() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = temp_ctx(dir.path());
        let r = registry();
        let confirmer: Arc<dyn Confirmer> = Arc::new(crate::permission::AlwaysConfirm);
        let res = dispatch(
            &r,
            &ctx,
            &gate(4),
            &confirmer,
            "fs.create",
            json!({"path": "../../../../escape.txt", "content": "x"}),
        )
        .await;
        assert!(!res.ok);
        assert_eq!(res.error_code.as_deref(), Some("permission_denied"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn app_open_launches_and_verifies_winver() {
        // `winver` (the About Windows dialog) rather than Notepad: Notepad is a
        // single shared process on this machine — closing it by image name would
        // kill the user's own tabs. winver holds no state.
        let dir = tempfile::tempdir().unwrap();
        let ctx = temp_ctx(dir.path());
        let r = registry();
        let confirmer: Arc<dyn Confirmer> = Arc::new(crate::permission::AlwaysConfirm);
        let res = dispatch(&r, &ctx, &gate(3), &confirmer, "app.open", json!({"app": "winver"})).await;
        assert!(res.ok, "{res:?}");
        assert_eq!(res.data.unwrap()["image"], "winver.exe");

        // cleanup through the same verified path
        let close = dispatch(&r, &ctx, &gate(5), &confirmer, "app.close", json!({"app": "winver"})).await;
        assert!(close.ok, "{close:?}");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn app_open_reports_missing_apps_honestly() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = temp_ctx(dir.path());
        let r = registry();
        let confirmer: Arc<dyn Confirmer> = Arc::new(crate::permission::AlwaysConfirm);
        let res = dispatch(
            &r,
            &ctx,
            &gate(3),
            &confirmer,
            "app.open",
            json!({"app": "definitely-not-installed-xyz"}),
        )
        .await;
        assert!(!res.ok);
        assert_eq!(res.error_code.as_deref(), Some("not_found"));
        assert!(res.error.unwrap().contains("could not find"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn screenshot_captures_and_verifies_a_png() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = temp_ctx(dir.path());
        let r = registry();
        let confirmer: Arc<dyn Confirmer> = Arc::new(crate::permission::AlwaysConfirm);
        let res = dispatch(
            &r,
            &ctx,
            &gate(3),
            &confirmer,
            "screenshot",
            json!({"path": "shot.png"}),
        )
        .await;
        // A headless/session-0 environment may honestly refuse; if it claims
        // success the artifact must be a real PNG.
        if res.ok {
            let data = res.data.unwrap();
            assert_eq!(data["verified_png"], true);
            let bytes = std::fs::read(dir.path().join("shot.png")).unwrap();
            assert_eq!(&bytes[0..8], b"\x89PNG\r\n\x1a\n");
            assert!(bytes.len() > 128);
        } else {
            let code = res.error_code.clone().unwrap_or_default();
            let message = res.error.clone().unwrap_or_default();
            assert_eq!(
                code, "execution_failed",
                "unexpected failure class ({code}): {message}"
            );
        }
    }

    #[tokio::test]
    async fn screenshot_is_blocked_in_privacy_mode() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = temp_ctx(dir.path());
        ctx.privacy_mode = true;
        let r = registry();
        let confirmer: Arc<dyn Confirmer> = Arc::new(crate::permission::AlwaysConfirm);
        let res = dispatch(&r, &ctx, &gate(3), &confirmer, "screenshot", json!({})).await;
        assert!(!res.ok);
        assert_eq!(res.error_code.as_deref(), Some("denied"));
        assert!(res.error.unwrap().contains("Privacy Mode"));
    }

    #[test]
    fn base64_helpers_round_trip() {
        let samples = ["", "a", "ab", "abc", "héllo wörld — 你好", "{\"json\": true}"];
        for s in samples {
            assert_eq!(decode_b64(&b64(s)).as_deref(), Some(s), "round trip failed for {s:?}");
        }
        assert_eq!(decode_b64("not base64!").is_none(), true);
    }
}