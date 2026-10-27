//! End-to-end tests: the real `jarvis` binary, the real Python brain process, the
//! real permission gate and real tool executions — no mocks.

use std::process::{Command, Stdio};

fn jarvis() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jarvis"))
}

fn base_env(cmd: &mut Command) -> &mut Command {
    cmd.env_remove("JARVIS_CONFIG")
        .env_remove("JARVIS_LLM_API_KEY")
        .env_remove("JARVIS_LLM_PROVIDER")
        .env_remove("RUST_LOG")
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
}

fn run(args: &[&str], provider: &str) -> (String, String, i32) {
    let mut cmd = jarvis();
    base_env(&mut cmd);
    if !provider.is_empty() {
        cmd.env("JARVIS_LLM_PROVIDER", provider);
    }
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = cmd.output().expect("failed to run jarvis");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn doctor_with_the_offline_provider_passes_every_check() {
    let (stdout, stderr, code) = run(&["doctor"], "test");
    assert_eq!(code, 0, "doctor failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("all checks passed"), "{stdout}");
    assert!(stdout.contains("brain health"), "{stdout}");
}

#[test]
fn chat_executes_a_real_tool_and_answers_from_the_result() {
    let (stdout, stderr, code) = run(&["chat", "--once", "what time is it?"], "test");
    assert_eq!(code, 0, "chat failed:\n{stdout}\n{stderr}");
    // The tool was actually requested and executed…
    assert!(stdout.contains("tool `time.now` requested"), "{stdout}");
    assert!(stdout.contains("tool `time.now` succeeded"), "{stdout}");
    // …and the answer contains a real clock value, not a canned string.
    assert!(stdout.contains("jarvis>"), "{stdout}");
    assert!(stdout.contains("UTC"), "{stdout}");
    let has_clock = stdout
        .split("jarvis>")
        .nth(1)
        .map(|s| s.contains(':'))
        .unwrap_or(false);
    assert!(has_clock, "no time-of-day in the reply: {stdout}");
}

#[test]
fn chat_reads_a_real_file_through_the_gate() {
    let (stdout, stderr, code) = run(&["chat", "--once", "read Cargo.toml"], "test");
    assert_eq!(code, 0, "chat failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("tool `fs.read` succeeded"), "{stdout}");
    // proof the bytes came from the real file on disk
    assert!(
        stdout.contains("[workspace]") || stdout.contains("jarvis-protocol"),
        "unexpected file content: {stdout}"
    );
}

#[test]
fn chat_never_claims_actions_it_could_not_perform() {
    // No delete tool exists in Phase 1 — the assistant must not pretend it acted.
    let (stdout, stderr, code) = run(&["chat", "--once", "delete everything on my desktop"], "test");
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(!stdout.to_lowercase().contains("deleted"), "lied about deleting: {stdout}");
}

#[test]
fn missing_api_key_is_reported_honestly_with_a_nonzero_exit() {
    let (stdout, stderr, code) = run(&["chat", "--once", "hello"], "openai_compat");
    assert_eq!(code, 1, "expected degraded exit code:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("no AI provider configured") || stdout.contains("JARVIS_LLM_API_KEY"),
        "{stdout}"
    );
    // no fabricated success marker
    assert!(!stdout.contains("done successfully"));
}

#[test]
fn tools_json_exposes_permission_levels() {
    let (stdout, stderr, code) = run(&["tools", "--json"], "test");
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("tools --json must emit JSON");
    let tools = parsed["tools"].as_array().expect("tools array");
    assert!(tools.len() >= 6, "expected the Phase-1 built-ins: {tools:?}");
    for t in tools {
        assert!(t["name"].is_string());
        assert!(t["permission_level"].is_string(), "missing level: {t}");
        assert!(t["description"].as_str().map(|s| !s.is_empty()).unwrap_or(false));
    }
}

#[test]
fn config_output_never_contains_a_secret() {
    let mut cmd = jarvis();
    base_env(&mut cmd);
    cmd.env("JARVIS_LLM_API_KEY", "sk-e2e-secret-key-12345678901234567890");
    let out = cmd
        .args(["config"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run config");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        !stdout.contains("sk-e2e-secret-key"),
        "API key leaked in `jarvis config` output:\n{stdout}"
    );
    assert!(stdout.contains("***redacted***"), "{stdout}");
}

fn project_root() -> std::path::PathBuf {
    // The binary resolves the same root by walking up from its cwd; mirror that.
    let mut dir = std::env::current_dir().expect("cwd");
    loop {
        if dir.join("core/python/jarvis_py/main.py").exists() {
            return dir;
        }
        if !dir.pop() {
            panic!("project root not found");
        }
    }
}

#[test]
fn chat_opens_and_closes_a_real_application() {
    // app.open at level 1 runs without asking; app.close at level 3 needs the
    // gate, so this run uses --yes to answer the confirmation automatically.
    // Calculator (not Notepad) because Notepad is the user's shared multi-tab
    // process on this machine — closing it by image name would kill their tabs.
    let (stdout, stderr, code) = run(&["chat", "--once", "open calc"], "test");
    assert_eq!(code, 0, "open failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("tool `app.open` succeeded"), "{stdout}");
    assert!(stdout.contains("calc.exe"), "the answer must name the real process: {stdout}");

    let (stdout, stderr, code) = run(&["chat", "--yes", "--once", "close calc"], "test");
    assert_eq!(code, 0, "close failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("tool `app.close` succeeded"), "{stdout}");
    assert!(stdout.contains("calc.exe"), "{stdout}");
}

#[test]
fn deletion_without_confirmation_is_declined_and_the_file_survives() {
    let victim = project_root().join("logs/e2e-victim.txt");
    std::fs::create_dir_all(victim.parent().unwrap()).unwrap();
    std::fs::write(&victim, "precious").unwrap();

    // No --yes: stdin is closed, the confirmer must treat that as "no".
    let (stdout, stderr, code) = run(
        &["chat", "--once", "delete file logs/e2e-victim.txt"],
        "test",
    );
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(
        stdout.contains("declined") || stdout.contains("could not do that"),
        "the decline must be reported honestly: {stdout}"
    );
    assert!(
        victim.exists(),
        "the file was deleted WITHOUT confirmation — the gate failed!"
    );
    assert!(!stdout.to_lowercase().contains("verified\":true"), "must not claim a verified delete: {stdout}");

    // With --yes: the gate allows it and the tool verifies the file is gone.
    let (stdout, stderr, code) = run(
        &["chat", "--yes", "--once", "delete file logs/e2e-victim.txt"],
        "test",
    );
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(stdout.contains("tool `fs.delete` succeeded"), "{stdout}");
    assert!(!victim.exists(), "file should be gone after confirmation");
}

#[test]
fn chat_captures_a_screenshot_or_reports_honestly() {
    let logs = project_root().join("logs");
    let _ = std::fs::create_dir_all(&logs);
    let (stdout, stderr, code) = run(&["chat", "--once", "take a screenshot"], "test");
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(stdout.contains("tool `screenshot`"), "{stdout}");

    let shot = std::fs::read_dir(&logs)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .map(|n| {
                    n.to_string_lossy().starts_with("screenshot-")
                        && p.extension().map(|e| e == "png").unwrap_or(false)
                })
                .unwrap_or(false)
        });

    if stdout.contains("tool `screenshot` succeeded") {
        // Interactive desktop available: the artefact must be a verified PNG.
        let path = shot.expect("screenshot file should exist in logs/");
        assert!(stdout.contains("verified_png"), "missing verification evidence: {stdout}");
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..8], b"\x89PNG\r\n\x1a\n");
        let _ = std::fs::remove_file(path);
    } else {
        // Headless session: the tool must fail with a concrete reason and the
        // assistant must say nothing happened — never claim a capture.
        assert!(
            stdout.contains("tool `screenshot` failed"),
            "unexpected outcome: {stdout}"
        );
        assert!(
            stdout.contains("no desktop surface")
                || stdout.contains("screenshot failed")
                || stdout.contains("Nothing was changed"),
            "failure must carry a reason: {stdout}"
        );
        assert!(
            shot.is_none() || !stdout.contains("verified_png"),
            "must not claim a verified PNG that was not produced: {stdout}"
        );
        if let Some(path) = shot {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[test]
fn voice_demo_runs_the_full_chain_without_a_microphone() {
    // TTS renders the utterance → VAD → wake grammar → STT → agent → tool →
    // verified answer. This is the whole voice loop, offline.
    let (stdout, stderr, _code) = run(
        &["voice", "--demo", "--once", "--no-speak"],
        "test",
    );
    assert!(stdout.contains("demo utterance rendered"), "{stdout}\n{stderr}");
    assert!(stdout.contains("heard:"), "no transcript was produced: {stdout}");
    assert!(stdout.contains("jarvis>"), "no agent reply: {stdout}");
    // the command survived recognition and drove a real tool
    assert!(
        stdout.contains("UTC") || stdout.contains("tool `time.now`"),
        "the spoken command did not reach the agent/tools: {stdout}"
    );
}

#[test]
fn voice_ignores_audio_that_is_not_addressed_to_us() {
    // Render a command *without* the wake word: the loop must refuse to act.
    let mut cmd = jarvis();
    base_env(&mut cmd);
    let text = "what time is it today";
    let out = cmd
        .args(["speak", "--wav"])
        .arg(project_root().join("logs/e2e-nowake.wav"))
        .arg(text)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("render wav");
    assert_eq!(out.status.code(), Some(0), "wav render failed");

    let (stdout, stderr, code) = run(
        &[
            "voice",
            "--wav",
            project_root().join("logs/e2e-nowake.wav").to_str().unwrap(),
            "--once",
            "--no-speak",
        ],
        "test",
    );
    let _ = std::fs::remove_file(project_root().join("logs/e2e-nowake.wav"));
    assert_eq!(code, 3, "expected 'not addressed to me' exit code:\n{stdout}\n{stderr}");
    assert!(stdout.contains("not addressed to me"), "{stdout}");
    assert!(!stdout.contains("jarvis>"), "acted on audio without the wake word: {stdout}");
}

#[test]
fn emergency_stop_command_is_available_and_rearms() {
    // `/stop` in a piped session must not hang or crash.
    use std::io::Write;
    let mut cmd = jarvis();
    base_env(&mut cmd);
    cmd.env("JARVIS_LLM_PROVIDER", "test")
        .args(["chat"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn chat");
    {
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "/stop").unwrap();
        writeln!(stdin, "what time is it?").unwrap();
        writeln!(stdin, "/quit").unwrap();
    }
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(stdout.contains("stopped and re-armed"), "{stdout}");
    assert!(stdout.contains("tool `time.now` succeeded"), "session should still work after a stop: {stdout}");
}