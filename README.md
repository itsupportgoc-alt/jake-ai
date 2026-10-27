# JARVIS

An autonomous personal AI assistant for Windows (extensible to Linux/macOS) with a
native Android companion. Not a chatbot wrapper, not a WebView — a real agent:
the AI brain *proposes*, the permission-controlled Rust tool system *disposes*.

```
you → chat/voice → JARVIS BRAIN (Python: providers, planning, memory)
                       │ JSON-RPC 2.0 over stdio
                       ▼
              jarvis core (Rust: event bus → agent loop → permission gate → tools)
                       │
              apps · files · input · screen · Android gateway
```

> Status legend used throughout the docs: **IMPLEMENTED**, **PARTIALLY IMPLEMENTED**,
> **NOT IMPLEMENTED**. Nothing is claimed as working until a test exercises it.

## Current status — Phases 1–3 (core): see table

| Capability | Status |
|---|---|
| Rust agent runtime: event bus, budgets, timeouts, cancellation, emergency stop | IMPLEMENTED |
| Permission model: levels 0–4 × autonomy 1–5, confirmation flow, trusted rules | IMPLEMENTED |
| Tool registry + JSON-Schema validation + timeouts + path sandbox | IMPLEMENTED |
| Tools: `time.now` `sys.info` `fs.list` `fs.read` `fs.search` `fs.create` `fs.copy` `fs.move` `fs.delete` `app.list` `app.open` `app.close` `screenshot` | IMPLEMENTED |
| Python brain over supervised JSON-RPC stdio (restart, honest failure) | IMPLEMENTED |
| `LLMProvider` abstraction + OpenAI-compatible adapter + offline test provider | IMPLEMENTED |
| Secret redaction (logs + cloud egress) and structured rotating logs | IMPLEMENTED |
| `jarvis chat` / `doctor` / `tools` / `config` / `speak` / `listen` CLI | IMPLEMENTED |
| Conversation memory (short-term in-session) | IMPLEMENTED |
| Voice **output** (TTS: `jarvis speak`, WAV-render verification) | IMPLEMENTED |
| Voice **input** (STT: `jarvis listen`) | PARTIALLY IMPLEMENTED — engine runs; this machine reports “no audio device” honestly |
| Screen capture (`screenshot`, PNG-verified, blocked in Privacy Mode) | PARTIALLY IMPLEMENTED — verified to fail honestly in this headless session; needs an interactive desktop to actually capture |
| Wake word · VAD · mic loop · voice conversation | NOT IMPLEMENTED (needs an audio capture stack) |
| Keyboard/mouse injection · OCR · UI-tree automation | NOT IMPLEMENTED (Phase 3 continuation) |
| Persistent memory · automations · Android · cross-device · desktop UI · installer | NOT IMPLEMENTED (Phases 4–8, see `docs/roadmap.md`) |

## Prerequisites

- Windows 10/11 (developed and tested on Windows 11, x86_64)
- Rust (stable, `x86_64-pc-windows-gnu` toolchain) + a MinGW-w64 toolchain
  (`gcc`, `dlltool`) on `PATH` — e.g. WinLibs via `winget install BrechtSanders.WinLibs.POSIX.UCRT`
- Python 3.11+ (developed on 3.15) — `python` on `PATH`
- Optional: an OpenAI-compatible API key (OpenAI, OpenRouter, LM Studio, Ollama…)

## Quickstart

```bash
# 0) toolchain paths for this machine (MinGW + venv)
source scripts/env.sh

# 1) build
cargo build --release

# 2) offline check — no API key needed
python -m venv .venv
.venv/Scripts/python.exe -m pip install -r core/python/requirements.txt
JARVIS_LLM_PROVIDER=test ./target/release/jarvis doctor

# 3) talk to it (offline test provider drives real tools)
JARVIS_LLM_PROVIDER=test ./target/release/jarvis chat

# 4) full conversation: set your key, then
export JARVIS_LLM_API_KEY=sk-...
./target/release/jarvis chat
```

Useful commands:

```text
jarvis chat [--once "..."] [--yes]   interactive assistant (Ctrl+C = emergency stop)
jarvis doctor                        configuration / brain / provider health checks
jarvis tools [--json]                registered tools with permission levels
jarvis config                        effective configuration (secrets redacted)
```

In-chat: `/help`, `/tools`, `/autonomy 1..5`, `/reset`, `/stop`, `/quit`.

Voice:

```bash
jarvis speak "System status nominal"              # speak through the Windows TTS
jarvis speak --wav out.wav "rendered check"       # offline-verifiable render
jarvis listen --seconds 8                         # one dictation pass (needs a mic)
```

## Configuration

Copy `config/jarvis.example.toml` to `config/jarvis.toml` and edit. Every value has
an environment override: `JARVIS_<SECTION>_<KEY>` (e.g. `JARVIS_AGENT_MAX_STEPS=4`).
API keys are **never** stored in files — use `JARVIS_LLM_API_KEY`. See
`docs/permissions.md` and `docs/security.md` for the safety model.

## Development

```bash
source scripts/env.sh
cargo test --workspace                        # 113 Rust tests incl. 11 real e2e
PYTHONPATH=core/python .venv/Scripts/python.exe -m pytest core/python/tests  # 51 Python tests
```

## Documentation

`docs/architecture.md` · `docs/technology.md` · `docs/security.md` ·
`docs/permissions.md` · `docs/roadmap.md` · `docs/phase1.md` · `docs/structure.md` ·
`docs/environment.md` · `docs/repository-analysis.md` · `docs/dependencies.md`
"# jake-ai" 
