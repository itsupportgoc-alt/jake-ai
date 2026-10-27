#!/usr/bin/env bash
# JARVIS build environment for this machine — source it, don't execute it:
#   source scripts/env.sh
#
# Adds the MinGW-w64 toolchain (gcc/dlltool/ar needed by the x86_64-pc-windows-gnu
# Rust toolchain) and the project venv to PATH. Idempotent.

# --- MinGW-w64 (WinLibs via winget) -------------------------------------------
if command -v gcc >/dev/null 2>&1; then
  : # already on PATH
else
  winget_links="$LOCALAPPDATA/Microsoft/WinGet/Packages"
  mingw_bin="$(find "$winget_links" -maxdepth 4 -type d -path '*mingw64/bin' 2>/dev/null | head -1)"
  if [ -n "$mingw_bin" ]; then
    export PATH="$mingw_bin:$PATH"
    echo "[env] added MinGW to PATH: $mingw_bin"
  else
    echo "[env] WARNING: gcc not found. Install with:" >&2
    echo "      winget install --id BrechtSanders.WinLibs.POSIX.UCRT -e" >&2
  fi
fi

# --- project venv -------------------------------------------------------------
project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ -x "$project_root/.venv/Scripts/python.exe" ]; then
  export PATH="$project_root/.venv/Scripts:$PATH"
  export PYTHONPATH="$project_root/core/python${PYTHONPATH:+:$PYTHONPATH}"
fi

echo "[env] ready — cargo: $(command -v cargo || echo MISSING), python: $(command -v python || echo MISSING)"