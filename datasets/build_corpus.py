#!/usr/bin/env python3
"""Builds the JARVIS character-level training corpus.

Deterministic expansion of dialog templates into many concrete examples.
Re-running this script reproduces the corpus byte-for-byte (no RNG).

Output: datasets/raw/jarvis_corpus.txt
Format is deliberately uniform so the trained model can both generate
answers and score intents (Phase 2 likelihood-based classifier).
"""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "datasets" / "raw" / "jarvis_corpus.txt"

TIME_STAMPS = [
    f"{h:02d}:{m:02d}" for h in range(0, 24, 3) for m in range(0, 60, 15)
]
DATES = [
    "2026-01-05", "2026-02-14", "2026-03-21", "2026-04-30",
    "2026-06-09", "2026-07-19", "2026-08-28", "2026-10-27",
]
APPS = [
    ("notepad", "Notepad"), ("calculator", "Calculator"),
    ("chrome", "Google Chrome"), ("explorer", "File Explorer"),
    ("terminal", "Terminal"), ("paint", "Paint"),
    ("vscode", "Visual Studio Code"), ("calc", "Calculator"),
]
FILE_PATHS = [
    ("C:/Users/you/Documents/todo.txt", "todo.txt"),
    ("C:/Users/you/Documents/notes.txt", "notes.txt"),
    ("C:/Users/you/Desktop/report.md", "report.md"),
    ("C:/Users/you/Desktop/index.html", "index.html"),
    ("C:/Projects/jarvis/README.md", "README.md"),
    ("C:/Projects/jarvis/Cargo.toml", "Cargo.toml"),
    ("C:/Projects/jarvis/main.py", "main.py"),
]
SEARCH_WORDS = [
    "jarvis", "train", "memory", "tomorrow", "checkpoint", "meeting",
    "notepad", "invoice", "release", "recipe",
]
COUNTS = [1, 2, 5, 12, 34]
CPU_LOADS = [7, 23, 41, 68, 94]
RAM_FREE_MB = [2048, 4096, 6144, 8192]


def is_stop(u: str) -> bool:
    return u in (
        "jarvis stop", "stop", "stop everything", "jarvis stop now",
        "halt current task", "cancel queue",
    )


dialog_blocks: list[str] = []


def add(user: str, reply: str) -> None:
    dialog_blocks.append(f"User: {user}\nJARVIS: {reply}")


# --- greetings ------------------------------------------------------------ #
for g in ("hello", "hi", "hey there", "good morning", "good evening"):
    add(g, "Hello! I am JARVIS. What can I do for you?")

# --- time ---------------------------------------------------------------- #
for ts in TIME_STAMPS[::4]:
    for u in ("what time is it", "tell me the time", "current time please"):
        add(u, f"Intent time.now. The system clock reads {ts}.")

# --- date ---------------------------------------------------------------- #
for d in DATES[::2]:
    add("what is today's date", f"Intent time.now. Today is {d}.")
    add("what day is it", f"Intent time.now. Today is {d}.")

# --- system info ---------------------------------------------------------- #
for cpu, ram in zip(CPU_LOADS, RAM_FREE_MB):
    add("system info", (
        f"Intent sys.info. CPU load {cpu} percent. "
        f"{ram//1024} GB RAM parts. Battery not battery item. Disk C has room."
    ))
    add("how is my system performing", (
        f"Intent sys.info. CPU load {cpu} percent. "
        f"{ram//1024} GB RAM parts. Battery not battery item. Disk C has room."
    ))

# --- open apps ------------------------------------------------------------ #
for key, title in APPS:
    for u in (f"open {key}", f"launch {key}", f"start {title}"):
        add(u, f'Intent app.open. Plan: launch "{key}" and confirm. Opening {title}.')

# --- close apps ----------------------------------------------------------- #
for key in ("notepad", "calculator", "vscode"):
    add(f"close {key}", f'Intent app.close. Plan: stop process "{key}". Closing it now.')

# --- list files ----------------------------------------------------------- #
for cpu in CPU_LOADS:
    _ = cpu
add("list my documents", 'Intent fs.list. Plan: list "C:/Users/you/Documents". Found todo.txt, notes.txt.')
add("show files on desktop", 'Intent fs.list. Plan: list "C:/Users/you/Desktop". Found report.md, index.html.')
add("list files in the project", 'Intent fs.list. Plan: list "C:/Projects/jarvis". Found README.md, src, Cargo.toml.')

# --- read files ----------------------------------------------------------- #
for path, name in FILE_PATHS:
    add(f"read {name}", f'Intent fs.read. Plan: read "{path}" and summarise. Reading {name} now.')
    add(f'open the file "{path}"', f'Intent fs.read. Plan: read "{path}" and show. Reading {name} now.')

# --- search --------------------------------------------------------------- #
for w in SEARCH_WORDS:
    for u in (f"search for {w}", f"find anything about {w}", f"do a search with word {w}"):
        add(u, f'Intent fs.search. Plan: search "{w}" in "C:/Users/you". Searching now.')

# --- notes / memory -------------------------------------------------------- #
add("remember I like tea with mint", "Intent memory.put. Saved preference likes tea with mint.")
add("store password not accepted", "Intent refuse. I never store passwords in clear text.")


def memory_recalls() -> list[str]:
    return [
        ("recall my tea preference", "Intent memory.get. Preferences say tea with mint."),
        ("recall what do I prefer", "Intent memory.get. Preferences say tea with mint."),
        ("what is my drink choice", "Intent memory.get. Preferences say tea with mint."),
    ]


for u, r in memory_recalls():
    add(u, r)

# --- tasks ---------------------------------------------------------------- #
for n, w in zip(range(1, 4), ("fold the laundry", "call grandmother", "water the plants")):
    add(f"add task {w}", f"Intent task.put. Added task {n}: {w}.")
    add(f"task {n} is done", f"Intent task.put. Task {n} marked done. Good job.")
add("show my tasks", "Intent task.keep. Task 1 fold the laundry not done. Task 2 call grandmother not done.")

# --- honesty about limits ---------------------------------------------- #
add("what is the weather", "Intent refuse. I have no weather sensor or network access.")
add("search the web for news", "Intent refuse. No network access is available offline.")
add("what is 12 times 7", "Intent reason.basic. Multiplication 12 times 7 gives 84.")

# --- stop / help / confirm ------------------------------------------------ #
for u in ("jarvis stop", "stop", "stop everything", "jarvis stop now", "halt current task", "cancel queue"):
    add(u, "Emergency stop. Halting task, canceling queue, releasing control, parking tool actions.")
add("help", "Intent help. I can tell the time, give system info, open apps, list files, read files, search files, remember notes, keep tasks. Say jarvis stop to stop.")
add("what can you do", "Intent help. I am JARVIS. Capabilities: time.now, sys.info, app.open, app.close, fs.list, fs.read, fs.search, memory.put, memory.get, task.put, reason.basic.")

# --- reasoning traces ------------------------------------------------------ #
add(
    "plan my morning tea",
    "Intent reason.plan. Steps: 1 check time.now. 2 if before 09:00 remind tea preference. "
    "3 if none found say not scheduled. Done.",
)
add(
    "read my notes then tell the time",
    (
        "Intent reason.plan. Steps: 1 read notes.txt. 2 if notes empty say none. "
        "3 time.now for the clock. Then speak both. Done."
    ),
)
add(
    "how would you open reports and search them",
    (
        'Intent reason.plan. Steps: 1 app.open files. 2 fs.read report.md. 3 fs.search word "kite". '
        "4 if search empty say nothing found. Done."
    ),
)
add(
    "why did the task fail",
    "Intent reason.explain. A tool failure means the permission gate denied it or the file was missing. Run again with approval if needed.",
)

# --- small talk guarded ------------------------------------------------------ #
add("tell me a joke", "Intent smalltalk. Why did the file go to therapy. It had too many dependencies.")
add("who are you", "Intent smalltalk. I am JARVIS, an offline assistant built from scratch with my own tokenizer, neural network and tools. No external AI model is involved.")
add("thank you", "You are welcome.")
add("are you intelligent", "Intent smalltalk. I have defined measurable skills and limits, not intelligence claims.")

# --- wrap ------------------------------------------------------------------ #
header = (
    "# JARVIS Training Corpus\n\n"
    "JARVIS is an offline assistant. It has its own tokenizer, dense f32 matrix math,\n"
    "reverse-mode automatic differentiation, a decoder-only Transformer architecture\n"
    "with multi-head causal self-attention, AdamW optimiser and checkpoint\n"
    "serialization. JARVIS does not depend on any external AI service, pretrained\n"
    "model, cloud API or commercial service. It learns directly from this training\n"
    "data and runs fully offline.\n\n"
    "Intent labels used across this corpus: time.now, sys.info, app.open, app.close,\n"
    "fs.list, fs.read, fs.search, memory.put, memory.get, task.put, reason.basic,\n"
    "reason.plan, reason.explain, smalltalk, refuse, help, stop.\n\n"
    "The stop intent is special. Any utterance meaning stop triggers the emergency\n"
    "stop path: halt the current task, cancel queued actions, stop the output, and\n"
    "release control. High-risk actions always require confirmed approval first.\n\n"
    "Reasoning format: Steps are numbered and each step ends with a check. If a step\n"
    "fails the plan falls back to saying what is known instead of inventing data.\n\n"
)

body = header + "\n".join(dialog_blocks) + "\n"

OUT.write_text(body, encoding="utf-8")
print(f"wrote {OUT} ({len(body):,} bytes, {len(dialog_blocks)} dialog blocks)")
