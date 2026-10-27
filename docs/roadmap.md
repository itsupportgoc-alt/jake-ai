# Development Roadmap

Every phase ends with **BUILD → TEST → VERIFY → REPORT → CONTINUE**. A phase is only
"done" when the behaviour was exercised end-to-end, not when the code compiles.
Honesty rule: each deliverable is marked `IMPLEMENTED`, `PARTIALLY IMPLEMENTED` or
`NOT IMPLEMENTED` in the phase report.

## Phase 1 — Core ✅ COMPLETE (reported 2026-10-24)

- [x] Cargo workspace: `Jake-protocol`, `Jake-core`, `Jake-cli`
- [x] Config (TOML + `Jake_*` env overrides) and structured rotating logs (secret-safe)
- [x] Event bus (tokio broadcast) with correlation IDs
- [x] Permission gate (levels 0–4, autonomy 1–5, confirmation flow)
- [x] Tool registry + JSON schemas + validation + path sandbox
- [x] Python brain: `LLMProvider` abstraction with OpenAI-compatible adapter,
      a deterministic test provider, retries, timeouts, redaction
- [x] JSON-RPC 2.0 over stdio bridge (Rust ↔ Python), supervised with restart
- [x] Agent loop with budgets, cancellation, verification, honest error reporting
- [x] Chat REPL (`Jake chat`) — real end-to-end conversation through the brain
- [x] Tests: **99 Rust (incl. 8 real e2e) + 44 Python, all green**

See `docs/phase1.md` for the verification report.

## Phase 2 — Voice

Status 2026-10-24 (session 2):

- [x] `VoiceEngine` (`TtsEngine`) abstraction + Windows `System.Speech`
      implementation — **verified**: `Jake speak --wav` renders a real 154 KB
      RIFF/WAVE file; live playback executed end-to-end (exit 0, 6 s of speech)
- [x] `SpeechRecognizer` (`SttEngine`) abstraction + Windows dictation engine —
      **partially verified**: engine probes and runs, but this machine reports
      “No audio device is installed”, which `Jake listen` relays honestly
- [x] `voice.*` RPC surface (`speak`, `render`, `tts_probe`, `stt_probe`, `dictate`)
      + doctor probes (warn, never fake)
- [ ] Wake word (configurable, local) + VAD + microphone capture loop
- [ ] Voice conversation loop with barge-in ("stop" / "cancel that" / "repeat that")
- [ ] Optional faster-whisper STT (blocked: needs audio device + cp315 wheel check)

## Phase 3 — Computer control

Status 2026-10-24 (session 2):

- [x] Applications: `app.open` (PATH/App Paths/Start Menu resolution, launch
      **verified by process probe**), `app.close` (graceful first, level 3 +
      confirmation, exit verified) — e2e: notepad opened and closed for real
- [x] Files: `fs.create`, `fs.copy`, `fs.move` (never overwrite silently),
      `fs.delete` (level 3, confirmation e2e: declined without `--yes`, verified
      deletion with it)
- [x] Screenshot: `screenshot` with PNG validation + Privacy-Mode block —
      **partially verified**: this headless session has no desktop surface, and the
      tool/assistant report that honestly instead of faking a capture
- [ ] Keyboard & mouse injection (SendInput) — deferred: needs an interactive
      session to verify, otherwise it would be an unverified feature
- [ ] OCR + structured screen representation + UI-tree-first automation
- [ ] Rename/move edge cases (directory moves across volumes)

## Phase 4 — Agent

- Multi-step planning, task memory, verification probes, error recovery
- Long-term memory (SQLite CRUD + export + forget), semantic search when useful
- Automation rules, proactive mode (OFF by default), autonomy levels surfaced in UI

## Phase 5 — Android (requires Android SDK — not installed yet)

Prerequisite: Android cmdline-tools + platform + build-tools + `adb` (winget/manual),
Gradle wrapper checked into the repo.

- Native Kotlin/Compose app, foreground service, notifications, intents, device status
- Pairing + secure channel, voice commands, accessibility service with disclosure UI

## Phase 6 — Cross-device

- Device gateway (TLS WebSocket, pairing codes, revocation, session expiry)
- Remote commands, file transfer, device status, "take a screenshot on my PC" style flows

## Phase 7 — Automation

- Scheduled and event-triggered tasks, user-visible workflow manager

## Phase 8 — Polish

- Windows installer (`Jake-Setup.exe`), Android APK, Tauri UI, performance pass,
  security audit, signed updates with rollback, full docs set

## Environment prerequisites still outstanding

| Needed for | Item | Status |
|---|---|---|
| build/tests (any phase) | MinGW-w64 toolchain (`gcc`, `dlltool`) | ✅ installed (WinLibs via winget) — `source scripts/env.sh` |
| Phase 5 | Android SDK, platform-tools (`adb`), build-tools, Gradle wrapper | ❌ not installed — deferred by decision |
| Phase 2 (optional) | `faster-whisper` cp315 wheel or Windows Speech runtime | ⏳ evaluated in Phase 2 |
| Phase 4 | API key for chosen provider, or a local model server | ⏳ user-provided (`Jake_LLM_API_KEY`) |