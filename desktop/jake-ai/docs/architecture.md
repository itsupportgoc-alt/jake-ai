# JAKE AI architecture

## Process model

```
┌─────────────────────────────────────────────────────────────┐
│ Renderer (React + Tailwind + Monaco)                        │
│ contextIsolation, sandbox, no Node, strict CSP              │
│ 16 screens · typed IPC client · approval modal · event feed  │
└──────────────────────────┬──────────────────────────────────┘
                           │ window.jake.invoke(channel, payload)
                           │ allowlisted channels only
┌──────────────────────────▼──────────────────────────────────┐
│ Preload (contextBridge)                                     │
└──────────────────────────┬──────────────────────────────────┘
                           │ ipcRenderer.invoke('jake:invoke')
┌──────────────────────────▼──────────────────────────────────┐
│ Main process                                                │
│  ipc.ts — sender validation, envelope responses             │
│  AppContext — guarded operations, approvals, cancellation    │
│  Agent (OpenAI-compatible AI) ── tool registry ── Approval   │
│  Scanners: recon · vuln · accounts · rbac · api · php · mysql│
│  scope.ts — allowlist, DNS/IP, redirects, rate limit, HTTP   │
│  Store — settings + safeStorage secrets                     │
│  SQLite (sql.js/WASM) — schema, FKs, transactions            │
└─────────────────────────────────────────────────────────────┘
```

The AI never touches the operating system. It can only call the named tools in
`electron/services/agent.ts`, each of which maps to a deterministic service
function and a permission level.

The AI transport is provider-agnostic (`electron/services/ai.ts`): Ollama, LM
Studio, Qwen/DashScope, OpenRouter, Groq and OpenAI all use the same
OpenAI-compatible streaming protocol. Provider presets and model suggestions
live in `shared/providers.ts`; the active base URL, model and per-provider
encrypted key are resolved in the main process.

## Request lifecycle

1. Renderer invokes a channel; preload checks the allowlist.
2. `ipc.ts` validates the sender frame and payload, then calls a handler.
3. Privileged handlers go through `AppContext.guarded()`:
   - create assessment row,
   - ask `ApprovalManager` (level 1 runs; higher levels emit an approval event
     and wait for the modal, with a 180 s timeout that denies),
   - create an `AbortController` for emergency stop,
   - run the module, persist findings,
   - finish the assessment with a summary or error.
4. Events (`progress`, `approval`, `chat:delta`, `access`, `stopped`) are pushed
   to the renderer on `jake:event`.

## Scope enforcement

`assertUrlInScope()` is called for the target URL, every redirect hop and every
REST/API request:

- host must equal the target host or match an allowlisted pattern (`*.example.com`),
- DNS is resolved and private/loopback/link-local addresses are rejected unless
  the target explicitly allows private networks,
- only HTTP(S) is accepted,
- `scopedFetch()` applies a per-target token bucket, timeout, response-size cap
  and manual redirect handling.

External tools receive a validated hostname/URL and fixed argument arrays only.

## Data model (SQLite)

| Table | Purpose |
|---|---|
| `meta` | schema version |
| `settings` | non-secret settings; encrypted secret blobs |
| `conversations`, `messages` | AI memory (user/assistant/tool traces) |
| `targets` | authorized scope and authorization basis |
| `assessments` | one row per module run |
| `findings` | vulnerabilities with evidence and remediation |
| `recon_results` | raw passive recon data |
| `account_audits` | account and PHP audit summaries |
| `rbac_results` | role/route access matrix |
| `api_results` | API observations |
| `tool_runs` | external tool command, status, redacted output |
| `approvals` | every approval checkpoint and decision |
| `audit_logs` | chronological action log |
| `reports` | generated report paths |
| `connections` | non-secret profile metadata + encrypted credential blob |
| `access_captures` | authorized page-structure captures |

Foreign keys are on (`ON DELETE CASCADE` for assessment-owned rows). Writes are
debounced and persisted atomically via temp file + rename.

## Agent tool registry

`list_targets`, `list_connections`, `start_recon`, `start_vulnerability_scan`,
`audit_accounts`, `audit_rbac`, `scan_api`, `audit_php`, `audit_mysql`,
`list_findings`, `get_assessment`, `list_security_tools`, `run_external_tool`,
`generate_report`, `open_authorized_login`, `capture_authenticated_page`.

Tool results are redacted and truncated before being appended to the model
context. The loop is capped at six steps per request, can be cancelled, and
never fabricates a result: a failed tool returns an error to the model.

## Module boundaries

- **recon.ts** – passive data collection and header/cookie/TLS analysis.
- **vuln.ts** – non-destructive checks; no exploit payloads, no writes.
- **accounts.ts** – generic admin-API mapping; aggregates and masked identifiers only.
- **rbac.ts** – unauthenticated/viewer/admin comparison on GET/HEAD/OPTIONS.
- **apiScan.ts** – route-level auth, exposure, CORS, cache, docs and rate-limit sample.
- **php.ts** – line-oriented static rules with proposed patches; no file writes.
- **mysql.ts** – read-only session; metadata, grants, hash prefixes only.
- **reports.ts** – HTML/JSON/PDF from stored data with redaction.
- **access.ts** – headed system browser for manual login and structure capture.

## Testing

`tests/` covers redaction, scope/wildcard/private-IP rules, redirect escape
prevention with a live local HTTP server, PHP rules with a prepared-statement
false-positive check, and SQLite schema/FK/transaction behaviour.
`scripts/e2e-smoke.mjs` drives the real Electron app over CDP with an isolated
data directory and a local target, then verifies findings and all three report
formats.
