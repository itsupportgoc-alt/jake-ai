# Current Environment Analysis

**Audited:** 2026-10-24 by running each binary on the host machine.

## Operating system

| Item | Value |
|---|---|
| OS | Windows 11 (build 26300), reported as `MINGW64_NT-10.0-26300` |
| Shell | Git Bash / MSYS — all commands in this project use bash syntax |
| Architecture | x86_64 |
| Free disk | ~266 GB free on `C:` |

## Installed toolchain

| Tool | Version | Status |
|---|---|---|
| `rustc` / `cargo` | 1.99.0 (2026-08/09) | ✅ available |
| `python` | 3.15.0 (64-bit, Oct 2026) | ✅ available |
| `pip` | 26.2.1 (via `python -m pip`) | ✅ available |
| `node` / `npm` | v24.19.0 / 11.17.0 | ✅ available (not required by core) |
| `java` (JDK) | Temurin OpenJDK 17.0.20.1 | ✅ available |
| `gradle` | — | ❌ not installed |
| Android SDK | — | ❌ not installed (`ANDROID_HOME` unset) |
| `adb` | — | ❌ not installed |
| Chrome | installed | ✅ usable as browser automation target |

## Network

| Endpoint | Result |
|---|---|
| `https://pypi.org/simple/` | 200 ✅ (pip installs work) |
| `https://crates.io/` | reachable (403 on bare `curl /` is normal UA filtering) — cargo fetch will be verified at first build |
| `https://www.google.com` | 200 ✅ |
| Package managers | `winget` 1.29 available; no choco/scoop |
| MinGW-w64 toolchain (`gcc`, `dlltool`) | ❌ not installed at audit → **installed during Phase 1** via `winget install BrechtSanders.WinLibs.POSIX.UCRT` (required by the `x86_64-pc-windows-gnu` Rust toolchain to link tests/binaries); `scripts/env.sh` puts it on `PATH` |

## Python site-packages

No third-party packages installed at audit time (`numpy`, `requests`, `httpx`, `openai`,
`whisper`, etc. all absent). Dependencies are declared in
`core/python/requirements.txt` and installed into a project-local virtualenv
(`.venv/`) so the system interpreter stays clean.

## Consequences for the plan

1. **Rust + Python are both present** → hybrid architecture (Rust runtime, Python AI
   layer) is viable on this machine today.
2. **Android cannot be built here yet** → Android is deferred per user decision.
   Phase 5 requires Android Studio command-line tools, a platform, build-tools and
   `adb`; the roadmap records this as an environment prerequisite, and no Android
   feature will be claimed as implemented until it builds.
3. **JDK 17 exists but Gradle does not** → when Android work starts, use the Gradle
   wrapper (checked into the repo) rather than a system Gradle.
4. **Python 3.15 is very new** → some binary wheels (whisper/ctranslate2, sounddevice)
   may not publish `cp315` builds. Phase 2 therefore starts with Windows' built-in
   speech APIs (zero-install, offline) behind a swappable `SpeechRecognizer` /
   `VoiceEngine` interface, with optional faster-whisper as a later add-on.
5. **Home-directory git repo** → a project-local repository is initialised (see
   `docs/repository-analysis.md`).
6. **Rust links with the GNU toolchain** (`rustc -vV` → `x86_64-pc-windows-gnu`) and
   the machine had no `gcc`/`dlltool`, so `cargo test` could not link until MinGW-w64
   was installed (WinLibs via winget, no MSVC/Windows SDK present). Source
   `scripts/env.sh` before building; the toolchain lives under
   `%LOCALAPPDATA%\Microsoft\WinGet\Packages\…\mingw64\bin`.