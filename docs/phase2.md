# Phase 2: Context, Intent Recognition and Basic Reasoning

**IMPLEMENTED** — built on the Phase 1 from-scratch core (`jarvis-ai`), with no
external AI service at any point.

## Stronger checkpoint (user-requested scale-up)

Corpus expanded with `datasets/build_corpus.py` (deterministic template
expansion, 150 dialog blocks, ~17 KB): greetings, time/date, system info,
app open/close, file list/read/search, memory, tasks, refusals (honest limits),
emergency stop, help, small talk, and reasoning traces in a fixed
`Steps: 1 … 2 … Done.` format. The corpus doubles as the intent-label source.

Model grown and retrained as `models/jarvis-strong` (`JARVIS-NN-0.2`, step 26):

| | jarvis-small (Phase 1) | jarvis-strong |
|---|---|---|
| d_model / layers / heads | 64 / 2 / 4 | 128 / 4 / 4 |
| seq_len | 32 | 64 |
| parameters | ~108 K | ~820 K |
| corpus | 0.78 KB | 17.2 KB |
| eval cross-entropy | 0.68 | 1.28 |

Lower loss is not comparable across corpora (larger corpus = harder target); the
honest metric for the stronger model is its behaviour, shown in the demos below.

`train` now takes `--steps-per-epoch`, `--d-model`, `--layers`, `--heads`,
`--version`; default steps/epoch = 300 so batch count is decoupled from corpus
size (the Phase 1 loop recomputed every possible window — unbounded with a
larger corpus).

## Modules

### `context.rs` — conversation context window

Role-tagged turns (`System`/`User`/`Assistant`) held in a `ContextWindow` with a
character budget. Rendering is tested property-style: newest turns always
survive, oldest are dropped first, and trimming is always *visible* — the
renderer reserves room for a `[... earlier turns trimmed ...]` marker by
shedding the oldest kept line if it would not otherwise fit.

### `intent.rs` — likelihood-based intent classifier

Reuses the trained Transformer as a language model. For each of the 17 corpus
intents it scores the continuation `JARVIS: Intent <label>.` after
`User: <utterance>` with incremental forward passes (log-softmax per token,
mean log-prob ranks candidates). Confidence is explicit: a top1-top2 margin
below `reason::CLARIFY_MARGIN` (0.15) means "ask, don't guess".

**Deterministic emergency-stop guard.** A statistical classifier must never be
the only defence for `stop`: `is_emergency_utterance()` matches whole-word
stop/halt/cancel/abort/emergency keywords and short-circuits to the `stop`
intent (the Phase 1 finding halves here — a small LM *will* misroute untrained
stop phrasings, verified in tests: `"stop everything now"` scored `fs.search`
before the guard). Always-on even before training.

Measured with the checkpoint: `what time is it` → `time.now` (margin 1.14);
`open notepad` → `app.open` (margin 0.44); stop phrasings → guard.

### `reason.rs` — intent → plan

Deterministic planner: intents map to steps calling existing runtime tool names
(`time.now`, `sys.info`, `app.launch/stop`, `fs.list/read/search`,
`memory.put/get`, `task.put`). Slot parsing handles quoted strings, path-like
tokens, app keys, and a bare-word fallback for searches. When a slot is missing
the plan speaks a clarifying question instead of inventing a target. Mutation
plans (`app.*`, `fs.*`, `memory.put`, `task.put`) set `requires_confirmation`;
`stop` sets `emergency` and contains no tool steps.

### `generation.rs` — shared generator

The Phase 1 sampling loop moved into the library; `Generate` and `Reason
--generate` share one implementation.

## CLI

```bash
export PATH="/c/msys64/mingw64/bin:$PATH"    # GNU toolchain for linking
./target/release/jarvis-ai.exe intent  --text "open notepad"
./target/release/jarvis-ai.exe reason  --text "search for checkpoint"
./target/release/jarvis-ai.exe reason  --text "stop everything now"   # emergency
./target/release/jarvis-ai.exe generate --model-dir models/jarvis-strong --prompt "User: what time is it" --temperature 0.4
```

## Known limits (honest reporting)

- Generation from the LM is still partly fragmentary at 820K params on 17 KB —
  the classifier scores it directly rather than trusting generated text.
- Intent accuracy depends on corpus coverage; untrained phrasings route to the
  model's guess plus the clarify margin. Only `stop` is guaranteed (guard).
- No KV cache: classification cost is per-token re-encoding; fine at this scale.
- Voice, file/app automation execution, UI: **NOT IMPLEMENTED** (Phases 3–4).
