# JAKE AI security model

## Threats considered

- A compromised or prompt-injected AI trying to run arbitrary commands.
- A scan or redirect escaping the authorized scope.
- Secrets leaking to the renderer, logs, reports or the AI provider.
- Tampering with the local database or report files.
- Malicious content in a scanned page influencing the agent.

## Controls

| Control | Implementation |
|---|---|
| Renderer isolation | `contextIsolation: true`, `sandbox: true`, `nodeIntegration: false`, `webSecurity: true` |
| IPC surface | Explicit channel allowlist in `shared/channels.ts`, re-checked in preload and main; sender-frame validation |
| CSP | Strict production policy (`script-src 'self'`, no object, no framing); permissions denied; navigation blocked |
| Secrets | Per-provider AI API keys and connection credentials encrypted with Electron `safeStorage` (Windows DPAPI). Keys are only read in main. |
| Redaction | Authorization headers, cookies, API-key patterns, private keys, long opaque blobs and password fields removed before logging, AI transmission and reports |
| Scope | Per-request allowlist, DNS/IP private-range block, redirect re-validation, protocol restriction |
| Rate limiting | Per-target token bucket shared by all modules |
| Approvals | Level 2+ prompts; 180 s timeout denies; every decision stored in `approvals` and `audit_logs` |
| Tool execution | Fixed argument builders, installed-tool checks, hostname/URL validation, timeout, output cap, audit entry |
| AI authority | No shell/filesystem/database access; only registered tools; results redacted |
| Emergency stop | Aborts scans and AI streaming, denies pending approvals, closes browser session |
| Persistence | SQLite with foreign keys and transactions; atomic file replacement |

## Credential handling

- Credentials are supplied by the operator in the connection editor.
- They are encrypted before being written to SQLite and are never returned to
  the renderer after saving (the UI only receives non-secret config).
- Account audits read aggregates; password/hash fields are detected but their
  values are never stored, logged, displayed or sent to the AI.
- The MySQL auditor reads only hash *prefixes* (four characters) to classify the
  algorithm, never full hashes.
- Authorized browser sessions store cookies in Chromium's encrypted profile
  directory; JAKE does not export cookies.

## Data sent to the AI provider

With the default **Ollama** provider, prompts, tool summaries and replies stay on
the local machine; no cloud account is involved. With a cloud provider (Qwen,
OpenRouter, Groq, OpenAI or a custom remote endpoint), chat messages, the system
context (target names/IDs, connection names, finding counts) and redacted tool
summaries are sent to that provider. Never sent: passwords, tokens, cookies,
hashes, raw account dumps, or filesystem contents beyond summarized audit
results. Account and permission summaries use masked identifiers.

Cloud providers are external services. Operators must not submit confidential
production data without appropriate authorization and a lawful basis. Selecting
a local provider removes this exposure entirely.

## Deliberate non-features

- No exploitation, payload delivery, brute force or denial-of-service.
- No authentication bypass, credential theft or session reuse.
- No destructive level-5 operations.
- No silent account modification: PHP patches and any administrative change
  require per-change approval and create a backup/audit record.
- RBAC testing never sends write methods.

## Residual risks

- Local administrators can read process memory and the encrypted secrets while
  the app runs; OS-level access is out of scope.
- Static analysis and black-box checks are heuristic and can miss issues.
- A malicious scanned page could contain prompt-injection text; the agent is
  constrained to the tool registry, so the worst case is a misleading summary,
  not command execution. Operators should treat page content as untrusted.
- The rate-limit sample can produce false negatives behind a WAF/CDN.
