"""JARVIS Python brain: AI orchestration layer.

This package must never touch the OS directly: everything privileged is a tool
request handled by the Rust runtime behind the permission gate.
"""

__version__ = "0.1.0"