# Phase 1 — Implementation Plan (Core)

Goal: a **real, running** assistant loop on this machine — user text in, provider call
out, tool calls gated by the permission system, honest results back — with tests.

## Deliverables

| # | Deliverable | Component | Acceptance test |
|---|---|---|---|
| 1 | Workspace + protocol types | `Jake-protocol` | `cargo test` passes |
| 2 | Config (TOML + `Jake_*` env) | `Jake-core::config` | unit tests: defaults, file load, env override |
| 3 | Structured logging with rotation + secret filter | `Jake-core::logging` | unit test: a value flagged secret never appears in the log file |
| 4 | Event bus (broadcast, typed events, correlation IDs) | `Jake-core::events` | pub/sub test incl. slow-subscriber isolation |
| 5 | Permission gate (levels 0–4 × autonomy 1–5 + confirmation) | `Jake-core::permissions` | exhaustive matrix test; level 4 never auto-approved |
| 6 | Tool registry + schema validation + timeouts | `Jake-core::tools` | unknown tool / bad args / timeout tests |
| 7 | Built-in tools (level 0–2): `sys.info`, `time.now`, `fs.list`, `fs.read`, `fs.search`, `app.list` | `Jake-core::tools::builtin` | each tool executed and asserted |
| 8 | Python supervisor (spawn, newline JSON-RPC, restart, shutdown) | `Jake-core::python` | integration test with a fake python script |
| 9 | JSON-RPC 2.0 framing | both sides | round-trip + malformed-frame tests |
| 10 | `LLMProvider` + OpenAI-compatible adapter + test provider | `Jake_py.llm` | unit tests with a stubbed HTTP transport |
| 11 | Redaction pipeline | `Jake_py.redact` | API-key-shaped strings masked |
| 12 | Brain orchestrator: chat turn → tool proposals → execution → verification → reply | `Jake_py.brain` | offline test using the test provider |
| 13 | Rust agent runtime driving the brain | `Jake-core::runtime` | end-to-end: `Jake chat "..."` returns a verified answer |
| 14 | CLI: `Jake chat`, `Jake doctor`, `Jake tools` | `Jake-cli` | manual + scripted smoke run |
| 15 | Docs build (this set) + README quickstart | `docs/` | a fresh clone can follow it |

## Order of work

1. Scaffold repo (git, workspace, `.gitignore`, `config/Jake.example.toml`).
2. `Jake-protocol` — types first so both sides agree on one schema.
3. `Jake-core`: config → logging → events → permissions → tools → python bridge.
4. `Jake_py`: rpc → config → redact → llm → brain, with tests.
5. `Jake-cli`: chat/doctor/tools, then the end-to-end run.
6. Tests green → report → continue to Phase 2.

## Out of scope for Phase 1 (documented as not implemented)

Voice, screenshots/OCR, keyboard/mouse injection, Android, cross-device gateway,
persistent long-term memory, automations, desktop UI, installer.

## Risk register

| Risk | Mitigation |
|---|---|
| No API key available | a deterministic `test` provider implements the same interface, so every layer is testable offline; OpenAI-compatible adapter is exercised against a stub transport |
| Python 3.15 incompatibilities | **stdlib-only at runtime** — the brain needs no third-party packages at all; `pytest` installed into `.venv` |
| Home-dir git repo | project-local `.git` initialised |
| Windows path/encoding issues | `PathBuf` everywhere, UTF-8 output, tests run in Git Bash |
| `x86_64-pc-windows-gnu` toolchain without a linker | MinGW-w64 installed via winget (`BrechtSanders.WinLibs.POSIX.UCRT`); `scripts/env.sh` exports it |

---

# Phase 1 Report — BUILD · TEST · VERIFY · REPORT

**Date:** 2026-10-24 · **Verdict: BUILD ✅ · TEST ✅ · VERIFY ✅**

## BUILD

- `cargo check --workspace --all-targets` → exit 0, **no warnings**
- `cargo build --release` profile configured (LTO, stripped binaries)
- Python brain imports cleanly under Python 3.15 with zero runtime dependencies

## TEST

| Suite | Count | Result |
|---|---|---|
| `cargo test --workspace` — protocol | 19 | ✅ all pass |
| `cargo test --workspace` — core (config, logging, events, permissions, tools, bridge, python supervisor, cancel) | 68 | ✅ all pass |
| `cargo test --workspace` — CLI unit | 4 | ✅ all pass |
| `cargo test --workspace` — **real e2e** (`tests/e2e_chat.rs`: spawns the binary + Python brain) | 8 | ✅ all pass |
| `pytest core/python/tests` (rpc, config, redact, llm, orchestrator) | 44 | ✅ all pass |
| **Total** | **143** | ✅ |

## VERIFY (manual end-to-end, this machine)

```text
Jake doctor          → 8/8 checks passed (python, brain health, tools, logs…)
Jake chat --once "what time is it?"
                      → tool `time.now` requested → succeeded in 1 ms
                      → "It is 02:30:18 UTC (…)" — a real clock, not a canned string
Jake chat --once "system info"
                      → "DESKTOP-H6IAEJ3 is running windows (x86_64), 16 CPU cores…"
Jake chat --once "list the files in the project"
                      → real directory listing (19 entries)
Jake chat --once "read Cargo.toml"
                      → real file bytes through fs.read
Jake chat --once "hello"   (no API key)
                      → honest degraded message + exit code 1 (never "Done.")
Jake chat (piped /stop)    → emergency stop, session re-arms and still works
logs/Jake.log.<date>  → structured lines with correlation ids; 0 secrets found
```

## REPORT — status of the Phase-1 deliverables

| Deliverable | Status |
|---|---|
| 1 Workspace + protocol types | **IMPLEMENTED** |
| 2 Config (TOML + env) incl. secret redaction in Debug/Display | **IMPLEMENTED** |
| 3 Structured rotating logging with output-stream redaction | **IMPLEMENTED** |
| 4 Event bus with correlation IDs + slow-subscriber isolation | **IMPLEMENTED** |
| 5 Permission gate (exhaustive matrix tests, level 4 never auto-approved) | **IMPLEMENTED** |
| 6 Tool registry, schema validation, timeouts, path sandbox | **IMPLEMENTED** |
| 7 Built-in tools `time.now` `sys.info` `fs.list` `fs.read` `fs.search` `app.list` | **IMPLEMENTED** |
| 8 Python supervisor (spawn, stdio JSON-RPC, restart, honest death) | **IMPLEMENTED** |
| 9 JSON-RPC 2.0 framing both sides | **IMPLEMENTED** |
| 10 `LLMProvider` + OpenAI-compatible + test provider | **IMPLEMENTED** (anthropic/local adapters: Phase 4) |
| 11 Redaction pipeline (Rust + Python, logs + cloud egress) | **IMPLEMENTED** |
| 12 Brain orchestrator (proposal → execution → verification → reply) | **IMPLEMENTED** |
| 13 Rust agent runtime loop with budgets/stop | **IMPLEMENTED** |
| 14 CLI `chat` / `doctor` / `tools` / `config` | **IMPLEMENTED** |
| 15 Docs + README quickstart | **IMPLEMENTED** |
| Voice, vision, input control, Android, gateway, persistent memory, automations, UI | **NOT IMPLEMENTED** (Phases 2–8) |

## Deviations from the original plan

1. `httpx`/`pydantic` dropped — stdlib proved sufficient (documented in `technology.md`).
2. MinGW-w64 installed on the machine (linker prerequisite, documented in `environment.md`).
3. Brain launched as `python -m Jake_py.main` (not by file path) so package-relative
   imports resolve.
---

## Phase 1 addendum: `Jake-ai` crate (from-scratch neural network)

**IMPLEMENTED** — the Phase 1 milestone (self-trained model learning from the
training corpus) is demonstrated end-to-end by `core/rust/crates/Jake-ai`
with **zero external ML dependencies** (serde/clap only):

| Component | Status | Notes |
|---|---|---|
| f32 `Mat` + xorshift `Rng` | **IMPLEMENTED** | dependency-free |
| Reverse-mode autograd tape (`Tape`) | **IMPLEMENTED** | gradcheck-tested ops |
| Character tokenizer (train/save/load, special tokens, UNK→U+FFFD) | **IMPLEMENTED** | vocab=48 on current corpus |
| Decoder-only Transformer (embeddings, causal multi-head attention, pre-LN, GELU FFN) | **IMPLEMENTED** | ~108K params default |
| AdamW + global grad-norm clipping | **IMPLEMENTED** | |
| Checkpoints (`config.json`, `vocabulary.json`, `weights.bin`) | **IMPLEMENTED** | |
| CLI `train` / `generate` / `eval` | **IMPLEMENTED** | |
| Gradient checks | **IMPLEMENTED** | `tests/gradcheck.rs`: 7 tests incl. causal-softmax masking |
| Learned-output demonstration | **IMPLEMENTED** | 30 epochs: loss 3.77 → 0.19 (old stride-32 loop) → retrained 0.60 with randomized windows |

### Training-loop finding (documented for reproducibility)

The first training run (fixed non-overlapping stride-`seq_len` windows) reached
cross-entropy ≈ 0.12–0.18 yet produced gibberish on re-alignment: the model
memorized the corpus *only at fixed window offsets*. The window could not slide
freely, so generation and any shifted prompt were out-of-distribution. Fix:
each training step samples a **random window start** (`rng.below`). Retrained
loss is now ≈ 0.60 (harder, honest objective — no position-aligned memorization),
generation from arbitrary prompts produces corpus-vocabulary fragments, and
`eval` on the corpus reports the same loss it trains on. Inference sanity test:
`tests/inference_sanity.rs` re-checks checkpoint CE vs a manual softmax at a
fixed position (guarding against loss/logits divergence).

Hardware note: 22,440 steps/epoch set in debug mode produced a >10 min hang;
use `cargo build --release` (33 s build) — release training of 30 epochs ≈ 4 min.

Usage:

```bash
export PATH="/c/msys64/mingw64/bin:$PATH"    # GNU toolchain for linking
cargo build --release -p Jake-ai
./target/release/Jake-ai.exe train --corpus datasets/raw/Jake_corpus.txt --output-dir models/Jake-small --epochs 30
./target/release/Jake-ai.exe generate --model-dir models/Jake-small --prompt "Jake" --max-tokens 200 --temperature 0.7
./target/release/Jake-ai.exe eval --model-dir models/Jake-small --corpus datasets/raw/Jake_corpus.txt
```
