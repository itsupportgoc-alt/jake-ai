//! Structured, rotating, secret-safe logging.
//!
//! * one line per event: timestamp, level, target, file:line, message, fields
//! * daily rotation into `config.logging.dir`, with a size guard at startup
//! * **redaction is applied to the output stream itself**, so a secret printed by any
//!   crate or any `format!` still cannot reach the log file

use crate::config::LoggingConfig;
use std::io::{self, Write};
use std::sync::OnceLock;
use tracing_subscriber::EnvFilter;

/// Key names whose values must never be logged, matched case-insensitively as
/// whole words and followed by `=` or `:`.
const SENSITIVE_KEYS: &[&str] = &[
    "api_key",
    "apikey",
    "api-key",
    "access_token",
    "refresh_token",
    "token",
    "password",
    "passwd",
    "passphrase",
    "secret",
    "client_secret",
    "authorization",
    "auth",
    "credential",
    "credentials",
    "private_key",
    "session",
    "cookie",
];

/// Prefixes that mark a bare token as a secret wherever they appear.
const SECRET_PREFIXES: &[&str] = &[
    "sk-ant-",
    "sk-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "github_pat_",
    "AKIA",
    "ASIA",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxs-",
    "eyJ", // JWT header
    "AIza", // Google API key
    "glpat-",
];

/// Characters that may appear inside a secret token.
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+' | '/' | '=')
}

/// Assignment values also run to the next punctuation, so `Authorization: Bearer
/// abc…` is masked as a whole rather than half-masked (over-redact, never leak).
fn is_value_char(c: char) -> bool {
    is_token_char(c) || c.is_whitespace()
}

/// Mask known secret shapes in arbitrary text. Pure string scanning — no regex
/// engine, no extra dependencies.
pub fn redact(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;

    while i < chars.len() {
        if let Some((consume, masked)) = match_sensitive_assignment(&chars, i) {
            out.push_str(&masked);
            i += consume;
            continue;
        }
        if let Some((consume, masked)) = match_secret_prefix(&chars, i) {
            out.push_str(&masked);
            i += consume;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// If a sensitive key name starts at `i` and is followed by `=`/`:` and a value,
/// return how many chars to consume and the masked replacement.
fn match_sensitive_assignment(chars: &[char], i: usize) -> Option<(usize, String)> {
    let rest: String = chars[i..].iter().collect();
    let lower = rest.to_ascii_lowercase();
    for key in SENSITIVE_KEYS {
        if !lower.starts_with(key) {
            continue;
        }
        // left word boundary: not part of a longer identifier
        if i > 0 && (chars[i - 1].is_ascii_alphanumeric() || chars[i - 1] == '_') {
            continue;
        }
        let mut j = i + key.len();
        // right word boundary
        if j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
            continue;
        }
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        if j >= chars.len() || !matches!(chars[j], '=' | ':') {
            continue;
        }
        j += 1;
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        if j >= chars.len() {
            return None;
        }
        let mut k = j;
        while k < chars.len() && is_value_char(chars[k]) {
            k += 1;
        }
        // trim trailing whitespace from the consumed value
        while k > j && chars[k - 1].is_whitespace() {
            k -= 1;
        }
        if k == j {
            return None;
        }
        let mut replacement = String::new();
        replacement.extend(&chars[i..j]);
        replacement.push_str("***redacted***");
        return Some((k - i, replacement));
    }
    None
}

/// If a known secret prefix starts at `i`, mask the token that follows it.
fn match_secret_prefix(chars: &[char], i: usize) -> Option<(usize, String)> {
    let rest: String = chars[i..].iter().collect();
    for prefix in SECRET_PREFIXES {
        if !rest.starts_with(prefix) {
            continue;
        }
        // `sk-` etc. must not match inside a longer word (e.g. "desk-")
        if i > 0 && prefix.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            let prev = chars[i - 1];
            if prev.is_ascii_alphanumeric() || prev == '_' || prev == '-' {
                continue;
            }
        }
        let mut j = i + prefix.len();
        while j < chars.len() && is_token_char(chars[j]) {
            j += 1;
        }
        let len = j - i;
        return Some((len, format!("***redacted{}***", if len > 24 { "-long" } else { "" })));
    }
    None
}

/// Truncate huge payloads so a runaway tool cannot fill the disk.
pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut cut = max;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}…(+{} bytes)", &s[..cut], s.len() - cut)
    }
}

/// Line-buffering writer that redacts each completed line before forwarding it.
pub struct LineRedactor<W: Write> {
    sink: W,
    buf: Vec<u8>,
}

impl<W: Write> Write for LineRedactor<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop(); // drop '\n'
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let text = String::from_utf8_lossy(&line);
            let safe = truncate(&redact(&text), 8192);
            self.sink.write_all(safe.as_bytes())?;
            self.sink.write_all(b"\n")?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            let text = String::from_utf8_lossy(&self.buf).to_string();
            self.buf.clear();
            self.sink.write_all(redact(&text).as_bytes())?;
        }
        self.sink.flush()
    }
}

/// Stderr handle that can be cloned cheaply (std's `Stderr` is not `Clone`).
#[derive(Clone, Default)]
pub struct StderrSink;

impl Write for StderrSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        io::stderr().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        io::stderr().flush()
    }
}

/// `MakeWriter` factory so `fmt::layer().with_writer(...)` gets a redacting writer.
#[derive(Clone)]
pub struct RedactingSink<W: Clone> {
    inner: W,
}

impl<W: Clone> RedactingSink<W> {
    pub fn new(inner: W) -> Self {
        Self { inner }
    }
}

impl<'a, W> tracing_subscriber::fmt::MakeWriter<'a> for RedactingSink<W>
where
    W: Write + Clone + Send + 'static,
{
    type Writer = LineRedactor<W>;

    fn make_writer(&'a self) -> Self::Writer {
        LineRedactor {
            sink: self.inner.clone(),
            buf: Vec::new(),
        }
    }
}

static INIT: OnceLock<()> = OnceLock::new();

/// Install the global subscriber exactly once (idempotent — tests re-enter).
/// Returns the guard that keeps the file writer alive; drop it and logs stop.
pub fn init(cfg: &LoggingConfig) -> io::Result<Option<PathGuard>> {
    let first = INIT.set(());
    let dir = cfg.dir.clone();
    std::fs::create_dir_all(&dir)?;

    if first.is_err() {
        // Another caller already initialised; don't double-install.
        return Ok(None);
    }

    // Size guard: rotate away an already-huge file instead of appending forever.
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if let Ok(meta) = entry.metadata() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".log") && meta.len() > cfg.max_file_mb * 1024 * 1024 {
                let _ = std::fs::rename(
                    entry.path(),
                    dir.join(format!("{name}.oversized.{}", chrono::Utc::now().timestamp())),
                );
            }
        }
    }

    let file_appender = tracing_appender::rolling::daily(&dir, "jarvis.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let file_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(cfg.level.clone()))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    // Console stays quiet (warnings/errors only) so the front-end can render turn
    // progress cleanly; RUST_LOG overrides it for debugging.
    let console_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));

    use tracing_subscriber::prelude::*;

    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true)
        .with_writer(RedactingSink::new(non_blocking))
        .with_filter(file_filter);

    let console_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(RedactingSink::new(StderrSink))
        .with_filter(console_filter);

    tracing_subscriber::registry()
        .with(file_layer)
        .with(console_layer)
        .try_init()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    Ok(Some(PathGuard { _guard: guard }))
}

/// Keeps the log writer alive for the process duration.
pub struct PathGuard {
    _guard: tracing_appender::non_blocking::WorkerGuard,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_api_keys() {
        let s = redact("using key sk-abcdef1234567890abcdef1234567890 now");
        assert!(!s.contains("sk-abcdef"));
        assert!(s.contains("redacted"));
    }

    #[test]
    fn redacts_sensitive_assignments() {
        for text in [
            "api_key=sk-123456789012345678901234",
            "api_key: hunter2",
            "password = hunter2",
            "Authorization: Bearer abcdef1234567890",
            "TOKEN=abcdef1234567890",
        ] {
            let out = redact(text);
            assert!(!out.contains("hunter2"), "leaked in {out}");
            assert!(!out.contains("abcdef1234567890"), "leaked in {out}");
            assert!(!out.contains("sk-123456789012345678901234"), "leaked in {out}");
        }
    }

    #[test]
    fn redacts_github_jwt_and_google() {
        assert!(!redact("token ghp_abcdefghijklmnopqrstuvwx1234 leaked").contains("ghp_"));
        assert!(!redact("auth eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abc").contains("eyJhbGci"));
        assert!(!redact("key AIzaSyAbCdEf1234567890").contains("AIzaSy"));
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        let text = "Opened Chrome and read C:/Users/USER/report.txt at 12:04 (desk-1234)";
        assert_eq!(redact(text), text);
        assert_eq!(redact("the author wrote this"), "the author wrote this");
    }

    #[test]
    fn truncates_oversized_lines() {
        let big = "x".repeat(20_000);
        let out = truncate(&big, 100);
        assert!(out.len() < 200);
        assert!(out.contains("bytes"));
    }

    #[test]
    fn writer_redacts_complete_lines_only() {
        let mut sink: Vec<u8> = Vec::new();
        {
            let mut w = LineRedactor { sink: &mut sink, buf: Vec::new() };
            write!(w, "plain start api_key=").unwrap();
            write!(w, "hunter2 end\n").unwrap();
            w.flush().unwrap();
        }
        let out = String::from_utf8(sink).unwrap();
        assert!(!out.contains("hunter2"), "{out}");
        assert!(out.contains("plain start"));
        assert!(out.contains("redacted"));
    }

    #[test]
    fn init_is_idempotent() {
        let cfg = LoggingConfig::default();
        assert!(init(&cfg).unwrap().is_some());
        assert!(init(&cfg).unwrap().is_none());
    }
}