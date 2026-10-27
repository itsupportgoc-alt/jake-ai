# Technology Selection & Dependency Plan

Selections are justified against a real need; popularity alone is not a reason.

## Language allocation

| Concern | Language | Why |
|---|---|---|
| Agent runtime, scheduling, permissions, IPC, tool execution, logging, config | **Rust** (tokio) | concurrency, reliability, low-level OS access, single binary for the installer |
| LLM orchestration, speech, vision/NLP, NLP pipelines, RAG | **Python 3.15** | ecosystem for AI/ML, fastest provider SDK iteration |
| Android companion | **Kotlin + Jetpack Compose** | native (never WebView), Accessibility/MediaProjection APIs |
| Desktop UI (later phase) | **Tauri 2** (Rust + web front-end) | native shell, small footprint, reuses the Rust core in-process |
| Android build | **Gradle wrapper** (checked in) | no system Gradle required (none installed) |

## Rust dependencies

| Crate | Used for | Justification |
|---|---|---|
| `tokio` (rt-multi-thread, macros, sync, time, process, io-util) | async runtime, subprocess supervision, timeouts | the standard async runtime; needed for cancellable agent steps |
| `serde` + `serde_json` | all serialisation (JSON-RPC, config, tool schemas) | ubiquitous, no alternative needed |
| `tracing`, `tracing-subscriber`, `tracing-appender` | structured logging with size-based rotation | structured fields, non-blocking rotation |
| `thiserror` | typed error enums | ergonomic, zero-cost |
| `uuid` | task/event/correlation IDs | standard |
| `dirs` | platform config/data directories | cross-platform paths |
| `chrono` | timestamps in logs/events | well-maintained |
| `clap` (derive) | `jarvis` CLI parsing | standard, only on the binary crate |
| `reqwest` (json, rustls) | outbound HTTP where Rust itself must call APIs (device gateway, updates) | rustls avoids OpenSSL on Windows; **not** used for the LLM — that stays in Python |
| `notify` | file watching for plugins/config hot-reload | only added when plugins land |

Deliberately **not** added now: `pyo3` (ABI/build friction with a brand-new CPython,
worse isolation than a subprocess), `uniffi`, `tauri` (until the UI phase),
`rusqlite` (until the memory phase), `openai`/`reqwest`-based LLM clients (provider
logic belongs to the Python layer), `mouse`/`enigo`/`rdev` (until Phase 3, chosen
after evaluating their Windows support), `axum`/`hyper` (no server in Phase 1).

## Python dependencies

Phase 1 ended up **stdlib-only at runtime** (decision recorded during implementation):
the brain needs nothing beyond `json`, `urllib`, `threading`, so the originally
planned `httpx` and `pydantic` were *not* added — fewer dependencies, same behaviour,
and zero risk from missing Python 3.15 wheels.

| Package | Used for | Justification |
|---|---|---|
| (none at runtime) | JSON-RPC framing, HTTP calls, orchestration | standard library |
| `pytest`, `pytest-timeout` | tests | dev-only, pinned in `requirements.txt` |

Later phases add (each re-verified for Python 3.15 wheel availability before adoption):

- Phase 2 voice: Windows built-in `System.Speech` via PowerShell for STT/TTS (zero
  install, offline), optional `sounddevice` for capture, optional `faster-whisper`.
- Phase 3 vision: `Pillow` + an OCR engine (Windows OCR via WinRT, or `rapidocr-onnxruntime`).
- Phase 4 memory: `sqlite3` (stdlib) + optional local embeddings.

Everything is pinned in `core/python/requirements.txt` and installed into a
project-local `.venv`, not the system interpreter.

## Protocols

| Interface | Choice | Why |
|---|---|---|
| Rust ↔ Python | **JSON-RPC 2.0 over stdio, newline-delimited** | no open ports, process isolation, trivially testable, matches "structured IPC" |
| Desktop UI ↔ core | in-process (Tauri commands) | zero IPC surface |
| Android ↔ desktop (Phase 6) | WebSocket over TLS + paired-device key exchange | survives NAT, streams well, encryptable |
| Config | TOML file + environment overrides | human-editable, no code needed |
| Secrets | OS keychain (Windows Credential Manager) with `.env` fallback for dev | never hardcode, never log |

## Environment-driven constraints

1. No Android SDK/Gradle/adb → Phase 5 prerequisites recorded; no Android claims made.
2. Python 3.15 wheel risk → start with stdlib/built-in Windows speech APIs; treat
   `faster-whisper` as an optional upgrade, not a hard dependency.
3. Home-directory git repository → project-local repo initialised.