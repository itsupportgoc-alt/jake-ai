# Dependency List

Pinned manifests live next to the code:

- `core/rust/` → `Cargo.toml` (workspace) + `Cargo.lock`
- `core/python/requirements.txt` → installed into `.venv/`

## Rust (Phase 1) — resolved versions (from `Cargo.lock`)

| Crate | Version (resolved) | Layer |
|---|---|---|
| `tokio` | 1.54.1 | async runtime, process supervision, timeouts |
| `serde` | 1.0.229 | serialisation |
| `tracing` / `tracing-subscriber` / `tracing-appender` | 0.1.44 / 0.3.23 / 0.2.5 | structured logging + rotation |
| `thiserror` | 1.0.69 | error types |
| `uuid` | 1.29.0 | correlation IDs |
| `chrono` | 0.4.45 | timestamps |
| `dirs` | 5.0.1 | config/data paths |
| `clap` (derive) | 4.6.7 | CLI (binary crate only) |
| `toml` | 0.8.23 | config parsing |
| `tempfile` | 3.x (dev-only) | test scratch dirs |

## Python (Phase 1)

| Package | Version (installed) | Purpose | Notes |
|---|---|---|---|
| (standard library only at runtime) | — | JSON-RPC, HTTP, redaction, orchestration | the brain runs on stock Python |
| `pytest` | 9.1.1 | tests | dev-only |
| `pytest-timeout` | 2.4.0 | tests | dev-only |

## Rules

1. Nothing is added "because it is popular" — every entry above is tied to a
   requirement in `docs/technology.md`.
2. Versions are locked (`Cargo.lock` committed; `requirements.txt` pinned with `==`).
3. Any new dependency must be re-checked for Python 3.15 wheel availability first.
4. Install commands are documented in `docs/installation.md` (Phase 8) and mirrored by
   `scripts/`.

> The brain itself uses **only the Python standard library** in Phase 1 (`json`,
> `urllib`, `threading`) — so `httpx`/`pydantic` from the original plan were dropped
> rather than added, keeping the runtime dependency count at zero. Resolved versions
> above are authoritative; `Cargo.lock` and `core/python/requirements.txt` are
> committed.