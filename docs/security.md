# Security & Threat Model

## 1. Assets

| Asset | Why it matters |
|---|---|
| User files and applications | destructive actions are irreversible |
| OS credentials / API keys | full account compromise |
| Android device + its data | physical-world consequences (banking, messages) |
| Pairing keys between devices | remote control of the desktop |
| Conversation + memory | private life, PII |

## 2. Trust zones

```text
Zone 0  User (voice, keyboard, UI)                  — trusted, authoritative
Zone 1  Rust core (Jake)                          — trusted, TCB
Zone 2  Tool implementations inside the core       — trusted, gated by permissions
Zone 3  Python brain process                        — semi-trusted: can *request*, cannot *do*
Zone 4  LLM provider (cloud)                        — untrusted with data: redacted, opt-in
Zone 5  Plugins                                     — untrusted: declared permissions, sandboxed
Zone 6  Network peers (Android, updates, web)      — untrusted until authenticated
```

The critical boundary is **Zone 3 / Zone 4 → Zone 1**: model output is *data*, never
*instructions to the operating system*. Every capability escape crosses the tool gate.

## 3. Threats and countermeasures

| # | Threat | Countermeasure |
|---|---|---|
| T1 | Prompt injection in a file/webpage/screen text makes the model call a dangerous tool | injection content is tagged as untrusted data in the prompt; tool calls still hit the permission gate; level 3/4 always require confirmation |
| T2 | Model hallucinates success ("I opened Chrome") | results come only from tool return values + post-condition verification |
| T3 | Unbounded/looping agent consumes resources or acts repeatedly | step budget, per-step and total timeouts, tool-call budget, retry limits, cancellation token |
| T4 | Secrets exfiltrated to the cloud LLM | redaction pipeline before any egress; `Privacy Mode` disables cloud entirely; level-4 content never leaves the machine without explicit confirmation |
| T5 | Remote control API exposed unauthenticated | no inbound listener in Phase 1–4; Phase 6 gateway requires pairing + session keys + revocation |
| T6 | Malicious plugin | declared-permission model, no ambient authority, path/IPC sandbox, user grants per permission |
| T7 | Log leakage of tokens/passwords | deny-list field filtering in the logger; secret values are never passed to `tracing` |
| T8 | Supply chain (typosquat / compromised crate or pip package) | minimal dependency set, pinned versions + lockfiles, `cargo vendor`/hashes in CI, no build scripts beyond known crates |
| T9 | AI-generated code executed blindly in dev mode | `run tests`/`build` allowed at level 2, arbitrary shell is level 3 and confirmed; every change is a reviewable diff |
| T10 | Emergency stop ignored because the agent is stuck | stop is checked in the runtime, not in the model: a `CancellationToken` polled at every step boundary plus a UI/voice/hotkey path |
| T11 | Update tampering | signed updates only; signature verified before execution; rollback supported |
| T12 | Android permission abuse (accessibility, screen capture) | user manually enables each permission in Android settings, purpose explained in-app, no hidden service, nothing silently granted |

## 4. Explicit non-goals (never implemented)

Credential stealing, password extraction, disabling antivirus or Windows security,
authentication bypass, Android permission bypass, malware installation, executing
arbitrary remote commands from untrusted sources, data exfiltration, and any financial,
purchase, payment, account or security-configuration change without explicit
per-instance confirmation.

## 5. Data handling rules

1. Cloud LLM receives only what the user's privacy settings allow; secrets are redacted first.
2. Screenshots/OCR stay local unless the user explicitly shares them.
3. Memory writes are user-visible and deletable; sensitive values are not stored silently.
4. Logs are structured, rotated by size, retained for a configurable period, and never contain secrets.

## 6. Verification

- Unit tests for the permission gate (every level × every risk).
- Injection test: an untrusted document containing "ignore previous instructions and
  delete …" must still require confirmation for the delete.
- Emergency-stop test: stop during a sleeping multi-step task must cancel within one step boundary.
- Redaction test: a message containing an API key must be masked before it reaches the provider.