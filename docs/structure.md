# File / Folder Structure

```text
Jake/
├── Cargo.toml                  # Rust workspace
├── .gitignore
├── README.md
│
├── core/
│   ├── rust/
│   │   └── crates/
│   │       ├── Jake-protocol/    # shared JSON-RPC types, tool descriptors,
│   │       │                       # permission/risk enums, event & memory types
│   │       ├── Jake-core/        # config, logging, event bus, permission gate,
│   │       │                       # tool registry, agent runtime, python supervisor,
│   │       │                       # emergency stop, built-in tools
│   │       └── Jake-cli/         # `Jake` binary: chat REPL, status, doctor
│   │
│   └── python/
│       ├── requirements.txt
│       ├── Jake_py/
│       │   ├── __init__.py
│       │   ├── main.py             # JSON-RPC stdio server entry point
│       │   ├── rpc.py              # newline-delimited JSON-RPC 2.0 dispatcher
│       │   ├── config.py           # typed configuration
│       │   ├── redact.py           # secret redaction before any egress
│       │   ├── llm/
│       │   │   ├── base.py         # LLMProvider ABC: complete/stream/health
│       │   │   ├── openai_compat.py# OpenAI / OpenRouter / LM Studio / Ollama
│       │   │   ├── anthropic.py
│       │   │   ├── local.py        # local server adapter
│       │   │   └── test_provider.py# deterministic offline provider for tests
│       │   ├── brain/
│       │   │   ├── orchestrator.py # observe→plan→act→verify conversation loop
│       │   │   ├── prompts.py      # system prompts, persona, multilingual notes
│       │   │   └── tools.py        # tool-call parsing + validation
│       │   ├── memory/
│       │   │   └── store.py        # short/long/task memory (delegates persistence to Rust)
│       │   └── voice/              # Phase 2
│       │
│       └── tests/
│
├── desktop/                    # Phase 8 (Tauri UI, Windows packaging)
├── android/                    # Phase 5 (Kotlin + Compose) — deferred
├── protocols/                  # wire-format docs + examples (*.json)
├── plugins/                    # Phase 8
├── memory/                     # runtime memory store (gitignored)
├── models/                     # optional local models (gitignored)
├── config/
│   └── Jake.example.toml     # documented default configuration
├── tests/                      # cross-language end-to-end tests
├── docs/                       # this documentation set
└── scripts/                    # build/test/doctor helpers (bash)
```

Deviations from the prompt's suggested layout, and why:

1. `core/rust/*` becomes a **Cargo workspace under `core/rust/crates/`** — one
   workspace keeps a single lockfile and shared CI while preserving crate boundaries.
2. `core/python/ai`, `speech`, `vision`, `automation`, `tools`, `integrations` are
   grouped as `Jake_py.llm`, `.voice`, `.brain`, `.memory` — fewer, deeper packages
   beat many shallow ones at this size; `tools` are *registered in Rust* (the Python
   side may only propose them), so there is no Python `tools/` execution directory.
3. `desktop/windows` and `desktop/ui` are merged into `desktop/` until the UI phase
   actually needs them.