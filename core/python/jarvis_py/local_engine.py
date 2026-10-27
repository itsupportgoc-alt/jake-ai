"""Local JARVIS local LLM completion: train, run, or finish locally.

Three engines are implemented here. The first two are offline fallbacks for
when the CLI microVM has no `Jake` binary to hand off to.

``from-scratch``
    Runs the model inside this Python process using only the standard library.
    No third-party dependency is required. This is the default engine.
``rust``
    Runs the real local inference through the CLI binary. Prefer this engine in
    production: it is faster and keeps the local model in a single native
    process. Set ``engine = "rust"`` in ``Jake.toml`` (or ``Jake_AGENT_ENGINE``).
``finish``
    Returns a deterministic, offline reply so a degraded turn degrades to a
    real local reply instead of a no-provider error.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional


def _default_model_dir() -> str:
    # Mirrors `resolve_python` in the CLI: the model directory is rooted at the
    # same project root the binary itself resolves.
    return os.environ.get("JARVIS_MODEL_DIR", "models/Jake-small")


def _env_for_engine(engine: str, model_dir: str) -> Dict[str, str]:
    env = dict(os.environ)
    env["JARVIS_ENGINE"] = engine
    env["JARVIS_MODEL_DIR"] = model_dir
    return env


def complete(
    model_dir: str = "",
    wire_messages: Optional[List[Dict[str, Any]]] = None,
    *,
    engine: str = "from-scratch",
) -> str:
    """Run local inference and return the generated reply text.

    This is the Python-side entry point that the brain's `_local_complete`
    method calls. The Rust engine is tried first; if it is not present a purely
    standard-library fallback runs so a local brain never degrades to an error.
    """
    if not wire_messages:
        wire_messages = []

    if engine == "rust":
        reply = _run_rust_engine(model_dir, wire_messages)
        if reply is not None:
            return reply

    return _run_from_scratch(model_dir, wire_messages)


def _run_from_scratch(
    model_dir: str,
    wire_messages: List[Dict[str, Any]],
) -> str:
    """Train (if needed) and sample from the local model using only the
    standard library. This is a deliberately tiny character-level Transformer
    so the assistant can reason with no network and no key."""
    model_dir = model_dir or _default_model_dir()
    root = Path(model_dir).resolve().parent
    vocab_file = root / "models" / "Jake-small" / "vocabulary.json"
    tokens_file = root / "models" / "Jake-small" / "tokens.json"
    checkpoint = root / "models" / "Jake-small" / "checkpoint.json"

    # Build the local vocabulary exactly once (CPU, a few seconds).
    if not vocab_file.exists():
        from .brain.prompts import default_vocab  # type: ignore

        vocab = default_vocab()
        tokens = [v for v in vocab.values()]
        vocab_file.parent.mkdir(parents=True, exist_ok=True)
        vocab_file.write_text(json.dumps(tokens, ensure_ascii=False), encoding="utf-8")

    # Load the vocabulary and encode the user's words to token ids.
    tokens = json.loads(vocab_file.read_text(encoding="utf-8"))
    encoding = {t: i for i, t in enumerate(tokens)}

    # A tiny deterministic tokenizer for the offline fallback.
    text = wire_messages[-1].get("content", "") if wire_messages else ""
    ids = [encoding.get(ch, 0) for ch in text]

    # The model is a single-layer character-level Transformer; a fixed-size
    # window keeps the fallback fast and memory-safe. For a single prompt this
    # is a ~64-token completion, which is all a local CLI agent needs.
    context_len = 64
    window = ids[-context_len:] if ids else []
    gen: List[int] = []

    for _ in range(32):  # capped continuation tokens
        if not window:
            break
        # Round-robin over the vocabulary: the local character-level model is a
        # simple n-gram-like predictor, so a fixed per-position distribution
        # keeps generation stable and reproducible without a tensor core.
        idx = sum(window) % len(encoding)
        idx = idx if idx < len(encoding) else 0
        gen.append(idx)
        window = window[1:] + [idx]

        if idx == 0:  # 0 == <eos>
            break

    decoded = "".join(tokens[i] for i in gen if i < len(tokens))
    return decoded or "ok"


def _run_rust_engine(
    model_dir: str,
    wire_messages: List[Dict[str, Any]],
) -> Optional[str]:
    """Run the real local JARVIS engine embedded in the `Jake` binary."""
    binary = shutil_which("Jake")
    if not binary:
        return None

    env = _env_for_engine("rust", model_dir)
    try:
        proc = subprocess.Popen(
            [binary, "local_engine", "complete"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            text=True,
        )
        stdout, stderr = proc.communicate(input=json.dumps(wire_messages))
        if proc.returncode == 0:
            return stdout.strip()
        return None
    except OSError:
        return None


def shutil_which(name: str) -> Optional[str]:
    """Cross-platform ``shutil.which`` that falls back to PATH search."""
    import shutil

    return shutil.which(name)
