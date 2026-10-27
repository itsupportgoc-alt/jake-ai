# JARVIS — Architecture

Autonomous personal AI assistant: a native agent system for Windows (extensible to
Linux/macOS) plus a native Android companion. Not a chatbot wrapper, not a WebView.

## 1. Design principles

1. **The brain decides what should happen; the tool system decides what may happen.**
   The LLM never executes anything directly — it emits *tool calls*, the Rust core maps
   them to registered tools, checks the permission model, and only then executes.
2. **Two processes, one contract.** A Rust supervisor owns everything privileged; a
   sandboxed Python subprocess owns everything AI-shaped. They speak JSON-RPC 2.0 over
   stdio. Python has no implicit OS authority — every side effect must be requested
   from Rust as a tool call.
3. **Nothing is claimed without verification.** A tool returns a structured result
   (`ok`, evidence, error). The assistant's spoken/written answer is derived from that
   result, never from the model's optimism.
4. **Every dangerous path has a gate.** Permission levels 0–4, per-tool risk, explicit
   user confirmation, an autonomy level that widens or narrows automatic execution,
   and an emergency stop that outranks everything.
5. **Providers are pluggable.** LLM, STT, TTS, wake word and vision engines sit behind
   traits/abstract classes so no single vendor is baked in.

## 2. Process topology

```text
                 ┌────────────────────────────────────────────┐
   voice/text ──►│  jarvis (Rust binary)                      │
   UI            │                                            │
                 │  event bus ──► agent runtime ──► tool router│
                 │       │              │              │      │
                 │  scheduler      permission gate  tool impls │
                 │       │              │        (fs/app/kbd…) │
                 │  emergency stop ─────┘                      │
                 │       │  JSON-RPC 2.0 / stdio               │
                 │       ▼                                     │
                 │  python child process (jarvis brain)        │
                 │    LLMProvider (openai|anthropic|local|…)   │
                 │    planner / memory / NLP                   │
                 └────────────────────────────────────────────┘
                              ▲
                              │  encrypted WebSocket (Phase 6)
                       ┌──────┴──────┐
                       │ Android app │
                       └─────────────┘
```

Responsibility split:

| Layer | Owns | Must not own |
|---|---|---|
| Rust `jarvis-protocol` | shared JSON-RPC types, tool schemas, permission & risk enums, event types | behaviour |
| Rust `jarvis-core` | config, structured logging, event bus, permission gate, confirmation flow, tool registry, agent loop, scheduler, Python supervisor, emergency stop | prompt engineering |
| Rust `jarvis-cli` | terminal/chat front-end (later: Tauri UI) | business logic |
| Python `jarvis_py.brain` | intent → plan → tool selection → observation → reply loop | direct file/process access |
| Python `jarvis_py.llm` | provider adapters, streaming, retries, token budget | tool execution |
| Python `jarvis_py.voice` | VAD, STT, TTS, wake word | — |
| Python `jarvis_py.memory` | short/long/task/semantic memory CRUD | secrets |

## 3. Message contract (JSON-RPC 2.0 over stdio)

Rust → Python requests:

| Method | Purpose |
|---|---|
| `brain.chat` | one conversational turn: `{messages, context, tools, autonomy, memory}` → `{message, tool_calls[]}` |
| `brain.plan` | produce a plan for a multi-step task |
| `brain.cancel` | drop in-flight generation |
| `brain.health` | liveness/version/provider status |

Python → Rust requests (capability escapes; each is a permission-checked tool call):

| Method | Purpose |
|---|---|
| `tool.execute` | `{name, args}` → `{ok, data, error, confirmation_required}` |
| `tool.list` | declared tools with schemas and risk |
| `memory.get/search/put/delete` | memory access through Rust, which enforces redaction |
| `log.event` | structured log line routed into Rust's rotating logger |
| `speak` | enqueue TTS (Phase 2) |

Every frame is one JSON object per line (newline-delimited JSON) — trivially debuggable,
no length-prefix parsing, safe for subprocess stdio.

## 4. Agent loop (Rust-owned, brain-guided)

```text
OBSERVE  (context, screen, memory, device state)
  ↓
UNDERSTAND  (brain.chat → intent + proposed tool calls)
  ↓
PLAN     (brain.plan for multi-step; step budget enforced)
  ↓
GATE     (permission level, autonomy level, risk, trust rules)
  ↓        └─ needs confirmation? → pause, show UI, wait / cancel
ACT      (tool.execute, timeout, cancellation)
  ↓
VERIFY   (result.ok + post-condition probe; never trust the model's claim)
  ↓
LOOP or FINISH → respond (text/TTS)
```

Hard limits, all configurable: `max_steps`, `step_timeout_ms`, `total_timeout_ms`,
`max_retries`, `tool_call_budget`, cancellation token checked between every step.
"Jarvis, stop" / the UI stop button / SIGINT trip the emergency stop, which cancels the
task token, clears the queue, stops speech and releases input grabs.

## 5. Memory

| Layer | Store | Notes |
|---|---|---|
| Short-term | in-process ring buffer of the current conversation | never persisted unless the user opts in |
| Task memory | append-only JSONL per task | results, verifications, failures |
| Long-term | SQLite (`memory/`) | user-approved facts/preferences, CRUD + export |
| Semantic | SQLite + local embeddings (optional) | vector search when configured |

Redaction runs on write: anything matching the secret patterns (API keys, tokens,
passwords) is stripped or requires explicit confirmation. `forget`/`delete` is a
first-class command.

## 6. Cross-device (Phase 6)

Desktop runs a **Device Gateway** (WebSocket over TLS with paired-device Ed25519/X25519
keys). Pairing is user-confirmed with a short code; devices get IDs, sessions expire,
and revocation is immediate. There is never an unauthenticated remote-control surface.

## 7. Extensibility

- **Tools**: a directory of tool definitions (name, description, JSON input schema,
  output schema, permission level, risk, timeout) registered at startup; Python can
  *propose* new tool calls only from this registry.
- **Plugins** (Phase 8): processes/wasm modules declaring required permissions,
  granted explicitly, executed with the same gate as built-in tools.

## 8. What is explicitly out of scope for the core

No PHP/MySQL, no web-app server architecture, no WebView app shell. The desktop UI is
native (Tauri/egui or WinUI later); the Android app is Kotlin + Compose.