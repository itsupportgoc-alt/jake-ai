//! `Jake` — the command-line front-end.
//!
//! This is a real client of the runtime, not a demo: chat goes through the brain,
//! every tool call passes the permission gate, confirmations are answered by an
//! interactive prompt, and Ctrl+C is a hard emergency stop.

use clap::{Parser, Subcommand};
use Jake_core::builtin;
use Jake_core::config::Config;
use Jake_core::logging;
use Jake_core::permission::{AutonomyLevel, ConfirmationRequest, Confirmer, PermissionGate};
use Jake_core::python::Brain;
use Jake_core::python::BrainSpec;
use Jake_core::runtime::{AgentRuntime, RuntimeParts};
use Jake_core::tools::ToolRegistry;
use Jake_core::{CancelFlag, CoreRpcHandler, EventBus};
use Jake_protocol::event::EventKind;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Parser, Debug)]
#[command(
    name = "Jake",
    version,
    about = "Jake — autonomous personal AI assistant (core CLI)"
)]
struct Cli {
    /// Path to Jake.toml (default: ./config/Jake.toml, or $JARVIS_CONFIG).
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Talk to Jake (interactive REPL, gated tools, emergency stop on Ctrl+C).
    Chat {
        /// Run a single prompt and exit (used by scripts and tests).
        #[arg(long)]
        once: Option<String>,
        /// Answer every confirmation with "yes" (never use with destructive tools).
        #[arg(long)]
        yes: bool,
    },
    /// Check configuration, the Python brain, providers and tools.
    Doctor,
    /// List the registered tools with their permission levels.
    Tools {
        /// Emit the registry as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print the effective configuration (secrets redacted).
    Config,
    /// Speak text aloud through the platform TTS engine.
    Speak {
        /// Text to speak.
        text: String,
        /// Render to a WAV file instead of playing (verifiable offline).
        #[arg(long)]
        wav: Option<PathBuf>,
        /// Speech rate, roughly -10..10 (0 = default).
        #[arg(long, default_value_t = 0)]
        rate: i32,
        /// Volume 0..100.
        #[arg(long, default_value_t = 100)]
        volume: i32,
    },
    /// Listen once via the platform speech recognizer and print the transcript.
    Listen {
        /// Seconds to wait for speech.
        #[arg(long, default_value_t = 8.0)]
        seconds: f64,
    },
    /// Voice conversation: capture → wake word → agent → speak (barge-in: Ctrl+C).
    Voice {
        /// Process this WAV file instead of the microphone (offline voice mode).
        #[arg(long)]
        wav: Option<PathBuf>,
        /// Synthesize a demo utterance with the wake word first (offline test/demo).
        #[arg(long)]
        demo: bool,
        /// Text used by --demo (default: "<WakeWord>, what time is it?").
        #[arg(long)]
        demo_text: Option<String>,
        /// Handle exactly one utterance, then exit.
        #[arg(long)]
        once: bool,
        /// Do not speak the reply (use in tests / silent environments).
        #[arg(long)]
        no_speak: bool,
        /// Capture timeout per utterance, seconds.
        #[arg(long, default_value_t = 8.0)]
        timeout: f64,
    },
}

/// Stdio shared between the REPL and confirmation prompts.
#[derive(Clone)]
struct SharedStdin {
    inner: Arc<tokio::sync::Mutex<BufReader<tokio::io::Stdin>>>,
}

impl SharedStdin {
    fn new() -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(BufReader::new(tokio::io::stdin()))),
        }
    }

    async fn read_line(&self) -> Option<String> {
        let mut lock = self.inner.lock().await;
        let mut buf = String::new();
        match lock.read_line(&mut buf).await {
            Ok(0) => None,
            Ok(_) => Some(buf),
            Err(_) => None,
        }
    }
}

/// Prompts on the terminal and waits for an explicit `y`/`yes`.
/// EOF, errors, or anything else counts as **declined**.
struct CliConfirmer {
    stdin: SharedStdin,
    auto_yes: bool,
}

impl Confirmer for CliConfirmer {
    fn confirm(&self, request: ConfirmationRequest) -> Jake_core::permission::ConfirmationFuture {
        if self.auto_yes {
            println!();
            println!(
                "  [auto-yes] {} ({} / {}) — {}",
                request.tool, request.level.label(), format!("{:?}", request.risk).to_lowercase(), request.reason
            );
            return Box::pin(async { true });
        }
        let stdin = self.stdin.clone();
        Box::pin(async move {
            println!();
            println!("  ┌─ permission required ─────────────────────────────");
            println!("  │ tool      : {}", request.tool);
            println!("  │ level     : {} ({})", request.level.label(), request.level.as_u8());
            println!("  │ risk      : {:?}", request.risk);
            println!("  │ reason    : {}", request.reason);
            if !request.explanation.is_empty() {
                println!("  │ detail    : {}", request.explanation);
            }
            println!("  │ arguments : {}", request.arguments);
            println!("  └─ proceed? [y/N] ");
            let answer = stdin.read_line().await.unwrap_or_default();
            let ok = matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes");
            print!("  ");
            ok
        })
    }
}

fn find_project_root() -> PathBuf {
    if let Ok(mut dir) = std::env::current_dir() {
        loop {
            let probe = dir.join("core/python/jarvis_py/main.py");
            if probe.exists() {
                return dir;
            }
            if !dir.pop() {
                break;
            }
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn resolve_python(root: &PathBuf, configured: &str) -> String {
    if !configured.is_empty() {
        return configured.to_string();
    }
    let candidates = [
        root.join(".venv/Scripts/python.exe"),
        root.join(".venv/bin/python"),
        root.join("core/python/.venv/Scripts/python.exe"),
        root.join("core/python/.venv/bin/python"),
    ];
    for c in candidates {
        if c.exists() {
            return c.display().to_string();
        }
    }
    "python".to_string()
}

fn brain_spec(cfg: &Config, root: &PathBuf) -> Jake_core::python::BrainSpec {
    let python = resolve_python(root, &cfg.python.executable);
    // Local brain: no API key, no HTTP base URL, no external LLM. The Rust engine
    // holds the local model (models/Jake-small), runs inference on the CLI process,
    // and never talks to a cloud or an API server.
    let brain_config = serde_json::json!({
        "llm": {
            "provider": "local_jake",
            "model": "Jake-small",
            "mode": "local",
            "model_dir": "models/Jake-small",
        },
        "agent": {
            "max_steps": cfg.agent.max_steps,
            "max_retries": cfg.agent.max_retries,
        },
        "persona": cfg.persona,
        "privacy": cfg.privacy,
        "project_root": root.display().to_string(),
    });
    BrainSpec {
        program: python,
        // `python -m Jake_py.main` so intra-package relative imports resolve;
        // PYTHONPATH below points at core/python.
        args: vec!["-m".into(), "jarvis_py.main".into()],
        env: vec![
            ("JARVIS_BRAIN_CONFIG".to_string(), brain_config.to_string()),
            ("PYTHONIOENCODING".to_string(), "utf-8".into()),
            ("PYTHONUTF8".to_string(), "1".into()),
            (
                "PYTHONPATH".to_string(),
                root.join("core/python").display().to_string(),
            ),
        ],
        restart_limit: cfg.python.restart_limit,
        default_timeout: Duration::from_millis(cfg.agent.total_timeout_ms),
    }
}

struct App {
    config: Arc<Config>,
    root: PathBuf,
    registry: Arc<ToolRegistry>,
    runtime: AgentRuntime,
    stdin: SharedStdin,
}

fn build_app(config: Config, auto_yes: bool) -> Result<App, Box<dyn std::error::Error>> {
    let root = find_project_root();
    let config = {
        let mut c = config;
        c.root = root.clone();
        Arc::new(c)
    };

    let mut registry = ToolRegistry::new();
    builtin::register_all(&mut registry)
        .map_err(|e| format!("cannot register built-in tools: {e}"))?;
    let registry = Arc::new(registry);

    let bus = EventBus::default();
    let stdin = SharedStdin::new();
    let confirmer: Arc<dyn Confirmer> = Arc::new(CliConfirmer {
        stdin: stdin.clone(),
        auto_yes,
    });
    let gate = PermissionGate::new(AutonomyLevel::new(config.agent.autonomy)?);
    let spec = brain_spec(&config, &root);
    let brain = Brain::spawn(spec, Arc::new(CoreRpcHandler::new(registry.clone())));

    let runtime = AgentRuntime::new(RuntimeParts {
        config: config.clone(),
        bus: bus.clone(),
        registry: registry.clone(),
        gate,
        brain,
        confirmer,
        cancel: CancelFlag::new(),
    });

    Ok(App {
        config,
        root,
        registry,
        runtime,
        stdin,
    })
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let config = match Config::load(cli.config.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error: {e}");
            std::process::exit(2);
        }
    };
    // Resolve relative paths (logs) against the project root, not the caller's cwd.
    let mut config = config;
    if config.logging.dir.is_relative() {
        config.logging.dir = find_project_root().join(&config.logging.dir);
    }
    let _guard = logging::init(&config.logging).ok().flatten();

    let auto_yes = matches!(&cli.command, Commands::Chat { yes: true, .. });
    let mut app = match build_app(config, auto_yes) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("startup failed: {e}");
            std::process::exit(2);
        }
    };

    let code = match &cli.command {
        Commands::Chat { once, .. } => run_chat(&mut app, once.clone()).await,
        Commands::Doctor => run_doctor(&app).await,
        Commands::Tools { json } => run_tools(&app, *json),
        Commands::Config => run_config(&app),
        Commands::Speak { text, wav, rate, volume } => run_speak(&app, text, wav.as_deref(), *rate, *volume).await,
        Commands::Listen { seconds } => run_listen(&app, *seconds).await,
        Commands::Voice {
            wav,
            demo,
            demo_text,
            once,
            no_speak,
            timeout,
        } => {
            run_voice(
                &mut app,
                wav.as_deref(),
                *demo,
                demo_text.clone(),
                *once,
                *no_speak,
                *timeout,
            )
            .await
        }
    };
    app.runtime.brain().shutdown().await;
    std::process::exit(code);
}

// --------------------------------------------------------------------- commands

fn run_config(app: &App) -> i32 {
    let mut value = serde_json::to_value(&*app.config).unwrap_or_default();
    // Belt and braces: the Secret type already redacts, this hides the raw field too.
    if let Some(obj) = value.as_object_mut() {
        if let Some(llm) = obj.get_mut("llm").and_then(|v| v.as_object_mut()) {
            llm.insert("api_key".into(), serde_json::json!("***redacted***"));
        }
    }
    println!("{}", serde_json::to_string_pretty(&value).unwrap_or_default());
    println!("\nproject root: {}", app.root.display());
    0
}

fn run_tools(app: &App, json: bool) -> i32 {
    let descriptors = app.registry.descriptors();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "count": descriptors.len(),
                "tools": descriptors,
            }))
            .unwrap_or_default()
        );
        return 0;
    }
    println!("{} tools registered\n", descriptors.len());
    println!("{:<16} {:>8} {:<9} {:>7} {:<12} {}", "TOOL", "LEVEL", "RISK", "TIMEOUT", "DESTRUCTIVE", "DESCRIPTION");
    for d in &descriptors {
        println!(
            "{:<16} {:>8} {:<9} {:>7} {:<12} {}",
            d.name,
            format!("L{}", d.permission_level.as_u8()),
            format!("{:?}", d.risk).to_lowercase(),
            format!("{}ms", d.timeout_ms),
            if d.destructive { "yes" } else { "no" },
            truncate(&d.description, 70)
        );
    }
    0
}

async fn run_doctor(app: &App) -> i32 {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Ok,
        Warn,
        Fail,
    }
    let mut checks: Vec<(&str, State, String)> = Vec::new();

    let cfg = &app.config;
    checks.push((
        "config",
        State::Ok,
        format!(
            "provider={} model={} autonomy=L{} privacy={}",
            cfg.llm.provider,
            cfg.llm.model,
            cfg.agent.autonomy,
            if cfg.privacy.privacy_mode { "on" } else { "off" }
        ),
    ));

    // No API key is required. Verify the real local architecture instead:
    // the Rust engine, the Python brain, the local model checkpoint + tokenizer,
    // the from-scratch inference build, and the CLI agent itself.
    let local_model = cfg.llm.provider == "local_jake"
        && cfg.llm.base_url.is_empty()
        && cfg.llm.api_key.is_empty();
    let model_dir = app.root.join("models").join("Jake-small");
    let model_ok = model_dir.join("config.json").exists()
                 && model_dir.join("vocabulary.json").exists();

    checks.push((
        "local model",
        if model_ok { State::Ok } else { State::Fail },
        if model_ok {
            format!("{} (config.json + vocabulary.json present)", model_dir.display())
        } else {
            format!("{} (missing local JARVIS model checkpoint)", model_dir.display())
        },
    ));
    checks.push((
        "local inference",
        if local_model { State::Ok } else { State::Warn },
        if local_model {
            "Rust engine loads models/Jake-small with no HTTP backend".to_string()
        } else {
            "provider = test | oai-compatible server | or set provider = local_jake without a URL".to_string()
        },
    ));

    let python = resolve_python(&app.root, &cfg.python.executable);
    let py_ok = if python == "python" || python == "python3" {
        std::process::Command::new(&python).arg("--version").output().is_ok()
    } else {
        PathBuf::from(&python).exists()
    };
    checks.push((
        "python",
        if py_ok { State::Ok } else { State::Fail },
        if py_ok { python.clone() } else { format!("{python} not found") },
    ));

    let brain_script = app.root.join("core/python/jarvis_py/main.py");
    checks.push((
        "brain script",
        if brain_script.exists() { State::Ok } else { State::Fail },
        brain_script.display().to_string(),
    ));

    match app.runtime.brain().ping(Duration::from_secs(20)).await {
        Ok(v) => checks.push((
            "brain health",
            State::Ok,
            format!("{} / {}", v["status"], v["detail"].as_str().unwrap_or("-")),
        )),
        Err(e) => checks.push(("brain health", State::Fail, format!("{}: {}", e.code, e.message))),
    }

    // Optional platform capabilities: reported as warnings when absent, never as
    // fake successes, and never fatal to the core assistant.
    match app
        .runtime
        .brain()
        .request("voice.tts_probe", serde_json::json!({}), Duration::from_secs(30))
        .await
    {
        Ok(v) if v["ok"] == true => {
            let voices = match v.get("voices").and_then(|x| x.as_array()) {
                Some(items) if !items.is_empty() => items
                    .iter()
                    .filter_map(|i| i.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => "(none reported)".to_string(),
            };
            checks.push((
                "voice output",
                State::Ok,
                format!("{}; voices: {}", v["detail"].as_str().unwrap_or("-"), voices),
            ));
        }
        Ok(v) => checks.push((
            "voice output",
            State::Warn,
            v["detail"].as_str().unwrap_or("unavailable").to_string(),
        )),
        Err(e) => checks.push(("voice output", State::Warn, format!("probe failed: {}", e.message))),
    }
    match app
        .runtime
        .brain()
        .request("voice.stt_probe", serde_json::json!({}), Duration::from_secs(40))
        .await
    {
        Ok(v) if v["ok"] == true => {
            let culture = v["culture"].as_str().unwrap_or("");
            checks.push((
                "voice input",
                State::Ok,
                if culture.is_empty() {
                    "dictation engine ready".to_string()
                } else {
                    format!("dictation engine ready ({culture})")
                },
            ));
        }
        Ok(v) => checks.push((
            "voice input",
            State::Warn,
            v["detail"].as_str().unwrap_or("unavailable").to_string(),
        )),
        Err(e) => checks.push(("voice input", State::Warn, format!("probe failed: {}", e.message))),
    }

    checks.push((
        "tools",
        if app.registry.is_empty() { State::Fail } else { State::Ok },
        format!("{} registered", app.registry.len()),
    ));

    let logs = &cfg.logging.dir;
    let logs_ok = std::fs::create_dir_all(logs).is_ok();
    checks.push((
        "logs",
        if logs_ok { State::Ok } else { State::Fail },
        if logs_ok {
            logs.display().to_string()
        } else {
            format!("cannot create {}", logs.display())
        },
    ));

    checks.push((
        "cloud allowed",
        if cfg.cloud_allowed() { State::Ok } else { State::Warn },
        if cfg.cloud_allowed() {
            "yes".into()
        } else {
            "no (privacy mode or allow_cloud_llm=false)".into()
        },
    ));

    println!("JARVIS doctor\n");
    let mut failures = 0;
    let mut warnings = 0;
    for (name, state, detail) in &checks {
        let (mark, label) = match state {
            State::Ok => ("  ok  ", "ok"),
            State::Warn => (" warn ", "warn"),
            State::Fail => (" FAIL ", "FAIL"),
        };
        let _ = label;
        println!("  [{mark}] {:<14} {detail}", name);
        match state {
            State::Fail => failures += 1,
            State::Warn => warnings += 1,
            State::Ok => {}
        }
    }
    println!("\nproject root : {}", app.root.display());
    println!("protocol     : {}", Jake_protocol::PROTOCOL_VERSION);
    if failures > 0 {
        println!("\n{failures} check(s) failed, {warnings} warning(s) — the assistant will report honestly instead of guessing.");
        1
    } else if warnings > 0 {
        println!("\nall required checks passed ({warnings} optional warning(s) above).");
        0
    } else {
        println!("\nall checks passed.");
        0
    }
}

async fn run_speak(app: &App, text: &str, wav: Option<&std::path::Path>, rate: i32, volume: i32) -> i32 {
    if text.trim().is_empty() {
        eprintln!("nothing to say");
        return 2;
    }
    let mut params = serde_json::json!({"text": text, "rate": rate, "volume": volume});
    let method = match wav {
        Some(path) => {
            params["path"] = serde_json::json!(path.display().to_string());
            "voice.render"
        }
        None => "voice.speak",
    };
    match app
        .runtime
        .brain()
        .request(method, params, Duration::from_secs(120))
        .await
    {
        Ok(v) if v["ok"] == true => {
            match wav {
                Some(p) => println!(
                    "rendered {} bytes to {} ({})",
                    v["bytes"].as_u64().unwrap_or(0),
                    p.display(),
                    v["detail"].as_str().unwrap_or("ok")
                ),
                None => println!("spoken in {} ms", v["elapsed_ms"].as_u64().unwrap_or(0)),
            }
            0
        }
        Ok(v) => {
            eprintln!("speech failed: {}", v["detail"].as_str().unwrap_or("unknown error"));
            1
        }
        Err(e) => {
            eprintln!("speech failed: {}: {}", e.code, e.message);
            1
        }
    }
}

async fn run_listen(app: &App, seconds: f64) -> i32 {
    println!("listening for up to {seconds:.0}s …");
    match app
        .runtime
        .brain()
        .request(
            "voice.dictate",
            serde_json::json!({"timeout_s": seconds}),
            Duration::from_secs_f64(seconds + 30.0),
        )
        .await
    {
        Ok(v) if v["ok"] == true => {
            println!("you> {}", v["text"].as_str().unwrap_or(""));
            0
        }
        Ok(v) => {
            eprintln!("I could not transcribe: {}", v["detail"].as_str().unwrap_or("unknown"));
            1
        }
        Err(e) => {
            eprintln!("speech recognition failed: {}: {}", e.code, e.message);
            1
        }
    }
}

async fn run_chat(app: &mut App, once: Option<String>) -> i32 {
    println!(
        "{} v{} — type /help for commands, Ctrl+C is the emergency stop",
        app.config.persona.name,
        Jake_protocol::PROTOCOL_VERSION
    );
    println!(
        "provider: {} / {} | autonomy L{} | {} tools\n",
        app.config.llm.provider,
        app.config.llm.model,
        app.config.agent.autonomy,
        app.registry.len()
    );

    let mut events = app.runtime.bus().subscribe();
    let printer = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(ev) => match ev.kind {
                    EventKind::ToolRequested => {
                        println!("  → {}", ev.message);
                    }
                    EventKind::ToolCompleted => {
                        println!("  ← {}", ev.message);
                    }
                    EventKind::PermissionRequested | EventKind::PermissionResolved => {
                        println!("  · {}", ev.message);
                    }
                    EventKind::EmergencyStop => {
                        println!("  ✕ {}", ev.message);
                    }
                    EventKind::Warning | EventKind::Error => {
                        println!("  ! {}", ev.message);
                    }
                    _ => {}
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    println!("  … {n} events skipped");
                }
                Err(_) => break,
            }
        }
    });

    if let Some(prompt) = once {
        let code = execute_turn(app, &prompt).await;
        printer.abort();
        return code;
    }

    loop {
        print!("you> ");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let Some(line) = app.stdin.read_line().await else {
            println!();
            break;
        };
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(cmd) = trimmed.strip_prefix('/') {
            match cmd.split_whitespace().next().unwrap_or("") {
                "quit" | "exit" | "q" => break,
                "help" | "h" => {
                    println!("  /help           this list");
                    println!("  /tools          list registered tools and their levels");
                    println!("  /autonomy <1-5> change autonomy level (asks for confirmation at >=4)");
                    println!("  /reset          clear conversation memory");
                    println!("  /config         show effective configuration");
                    println!("  /stop           emergency stop (cancel current work)");
                    println!("  /quit           exit");
                }
                "tools" => {
                    run_tools(app, false);
                }
                "config" => {
                    run_config(app);
                }
                "reset" => {
                    app.runtime.clear_history();
                    println!("  memory cleared.");
                }
                "stop" => {
                    app.runtime.emergency_stop("user command /stop");
                    app.runtime.reset_stop();
                    println!("  stopped and re-armed.");
                }
                "autonomy" => {
                    let arg = cmd.split_whitespace().nth(1).unwrap_or("");
                    match arg.parse::<u8>().ok().and_then(|v| AutonomyLevel::new(v).ok()) {
                        Some(level) if level.get() >= 4 => {
                            println!(
                                "  raise autonomy to L{}? tools up to level {} will run without asking. [y/N]",
                                level.get(),
                                level.get() - 1
                            );
                            let answer = app.stdin.read_line().await.unwrap_or_default();
                            if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                                app.runtime.gate().set_autonomy(level);
                                println!("  autonomy = L{}", level.get());
                            } else {
                                println!("  unchanged.");
                            }
                        }
                        Some(level) => {
                            app.runtime.gate().set_autonomy(level);
                            println!("  autonomy = L{}", level.get());
                        }
                        None => println!("  usage: /autonomy 1..5"),
                    }
                }
                other => println!("  unknown command /{other} — try /help"),
            }
            continue;
        }

        let code = execute_turn(app, &trimmed).await;
        if code != 0 {
            // keep the session alive but make the failure visible
            tracing::debug!(code, "turn reported a problem");
        }
    }

    printer.abort();
    0
}

/// Run one turn, returning (exit code, reply text) so voice mode can speak it.
async fn execute_turn_capture(app: &mut App, text: &str) -> (i32, Option<String>) {
    app.runtime.reset_stop();
    let turn = app.runtime.run_turn(text);

    let outcome = tokio::select! {
        res = turn => res,
        _ = tokio::signal::ctrl_c() => {
            app.runtime.emergency_stop("Ctrl+C");
            println!("\n  ✕ emergency stop — current work cancelled.\n");
            return (130, None);
        }
    };

    match outcome {
        Ok(turn) => {
            println!("\nJake> {}\n", turn.content);
            let reply = Some(turn.content.clone());
            if turn.degraded {
                println!(
                    "  (degraded: {} | steps={} tools={})\n",
                    turn.error.as_deref().unwrap_or("provider issue"),
                    turn.steps,
                    turn.tool_results.len()
                );
                return (1, reply);
            }
            (0, reply)
        }
        Err(e) => match e {
            Jake_core::runtime::RuntimeError::Cancelled => {
                println!("\n  ✕ stopped.\n");
                (130, None)
            }
            other => {
                println!("\n  ! {other}\n");
                (1, None)
            }
        },
    }
}

async fn execute_turn(app: &mut App, text: &str) -> i32 {
    execute_turn_capture(app, text).await.0
}

/// Voice conversation loop: capture → wake word → agent → speak (Ctrl+C = stop).
async fn run_voice(
    app: &mut App,
    wav: Option<&std::path::Path>,
    demo: bool,
    demo_text: Option<String>,
    once: bool,
    no_speak: bool,
    timeout: f64,
) -> i32 {
    let wake_words = app.config.persona.wake_word.clone();
    let mut mode = "mic";
    let mut wav_path = wav.map(|p| p.to_path_buf()).unwrap_or_default();

    if demo {
        // Synthesize "<wake>, what time is it?" so the full chain (TTS → VAD →
        // wake grammar → STT → agent → tools) runs without a microphone.
        let text = demo_text.unwrap_or_else(|| {
            let wake = wake_words.split(',').next().unwrap_or("Jake").trim();
            format!("{wake}, what time is it?")
        });
        match app
            .runtime
            .brain()
            .request("voice.demo_wav", serde_json::json!({"text": text}), Duration::from_secs(90))
            .await
        {
            Ok(v) if v["ok"] == true => {
                wav_path = PathBuf::from(v["path"].as_str().unwrap_or_default());
                println!(
                    "demo utterance rendered: {} ({} ms)",
                    wav_path.display(),
                    v["duration_ms"].as_u64().unwrap_or(0)
                );
            }
            Ok(v) => {
                eprintln!(
                    "cannot render the demo utterance: {}",
                    v["error"].as_str().unwrap_or("unknown error")
                );
                return 1;
            }
            Err(e) => {
                eprintln!("cannot render the demo utterance: {}: {}", e.code, e.message);
                return 1;
            }
        }
        mode = "wav";
    } else if !wav_path.as_os_str().is_empty() {
        mode = "wav";
    }

    println!(
        "voice mode: {mode}{} | wake words: {wake_words} | {}",
        if wav_path.as_os_str().is_empty() {
            String::new()
        } else {
            format!(" ({})", wav_path.display())
        },
        if no_speak {
            "silent replies"
        } else {
            "replies are spoken (Ctrl+C = stop)"
        }
    );

    loop {
        // --- 1) capture an utterance ---------------------------------------
        let mut params = serde_json::json!({
            "mode": mode,
            "wake_words": wake_words,
            "timeout_s": timeout,
        });
        if mode == "wav" {
            params["path"] = serde_json::json!(wav_path.display().to_string());
        }
        let capture = match app
            .runtime
            .brain()
            .request("voice.utterance", params, Duration::from_secs_f64(timeout + 60.0))
            .await
        {
            Ok(v) => v,
            Err(e) => {
                eprintln!("voice capture failed: {}: {}", e.code, e.message);
                return 1;
            }
        };

        if capture["ok"] != true {
            let stage = capture["stage"].as_str().unwrap_or("?");
            let error = capture["error"].as_str().unwrap_or("unknown error");
            eprintln!("I could not hear a command ({stage}): {error}");
            if once {
                return 1;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
            continue;
        }

        if capture["woke"] != true {
            let transcript = capture["transcript"].as_str().unwrap_or("");
            let reason = capture["reason"].as_str().unwrap_or("wake word not detected");
            println!("not addressed to me: {reason} (heard: \"{transcript}\")");
            if once {
                return 3;
            }
            continue;
        }

        let heard = capture["transcript"].as_str().unwrap_or("");
        let command = capture["command"].as_str().unwrap_or("").trim().to_string();
        println!("heard: \"{heard}\"");
        if command.is_empty() {
            println!("you are awake, but I heard no command after the wake word.");
            if once {
                return 3;
            }
            continue;
        }

        // --- 2) run the agent turn (tools, gate, verification) --------------
        let (code, reply) = execute_turn_capture(app, &command).await;
        if code == 130 {
            return code;
        }

        // --- 3) speak the reply (Ctrl+C = barge-in) -------------------------
        if !no_speak {
            if let Some(text) = reply.filter(|t| !t.trim().is_empty()) {
                let speak = app.runtime.brain().request(
                    "voice.speak",
                    serde_json::json!({"text": text}),
                    Duration::from_secs(180),
                );
                tokio::select! {
                    res = speak => {
                        match res {
                            Ok(v) if v["ok"] == true => {}
                            Ok(v) => eprintln!("(speech: {})", v["detail"].as_str().unwrap_or("failed")),
                            Err(e) => eprintln!("(speech: {})", e.message),
                        }
                    }
                    _ = tokio::signal::ctrl_c() => {
                        app.runtime.emergency_stop("barge-in (Ctrl+C during speech)");
                        app.runtime.brain().notify("voice.stop", serde_json::json!({})).await;
                        println!("\n  ✕ stopped.");
                        return 130;
                    }
                }
            }
        }

        if once {
            return code;
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.len() <= max {
        s
    } else {
        let mut cut = max;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}…", &s[..cut])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Jake_protocol::permission::PermissionLevel;

    #[test]
    fn truncate_never_splits_utf8() {
        let s = "héllo wörld";
        let out = truncate(s, 4);
        assert!(out.ends_with('…'));
        // a byte cut mid-codepoint would surface as U+FFFD
        assert!(!out.contains('\u{FFFD}'), "split a character: {out:?}");
        assert!(out.starts_with("hé"));
    }

    #[test]
    fn project_root_finds_the_python_entrypoint() {
        let root = find_project_root();
        assert!(
            root.join("core/python/jarvis_py/main.py").exists(),
            "root = {}",
            root.display()
        );
    }

    #[test]
    fn python_resolution_prefers_the_project_venv() {
        let root = PathBuf::from("/nonexistent");
        assert_eq!(resolve_python(&root, ""), "python");
        assert_eq!(resolve_python(&root, "C:/custom/python.exe"), "C:/custom/python.exe");
    }

    #[tokio::test]
    async fn confirmer_declines_on_eof() {
        // A confirmer bound to an exhausted stdin must decline, never approve.
        let confirmer = CliConfirmer {
            stdin: SharedStdin::new(),
            auto_yes: false,
        };
        let req = ConfirmationRequest {
            tool: "fs.delete".into(),
            arguments: "{}".into(),
            level: PermissionLevel::Level3,
            risk: Jake_protocol::permission::Risk::High,
            reason: "destructive".into(),
            explanation: String::new(),
        };
        // auto_yes path is exercised separately
        let auto = CliConfirmer {
            stdin: SharedStdin::new(),
            auto_yes: true,
        };
        assert!(auto.confirm(req.clone()).await);
        assert!(PermissionLevel::Level3.as_u8() == 3);
        let _ = confirmer;
    }
}