# Permission Model

The gate lives in Rust. The brain can *ask*; only the gate can *allow*.

## 1. Permission levels

| Level | Name | Examples | Default requirement |
|---|---|---|---|
| 0 | INFORMATION | system stats, time, `tool.list`, reading already-open text | automatic |
| 1 | LOW RISK | open/switch app, search files, read permitted files, screenshot of own UI | automatic (logged) |
| 2 | MODERATE | create/modify files, send a message, keyboard/mouse input, run build/tests | confirmation at autonomy ≤3, automatic at autonomy ≥4 |
| 3 | HIGH RISK | delete files, install software, change system settings, arbitrary shell | **always** explicit confirmation (unless a user-authored trusted rule matches) |
| 4 | CRITICAL | financial transactions, account/password changes, security config, remote-access config | **always** explicit confirmation + typed reason shown; never auto-approved by autonomy level |

## 2. Tool descriptor

Every tool declares:

```json
{
  "name": "fs.delete",
  "description": "Permanently delete files or directories",
  "input_schema": { "type": "object", "properties": { "paths": {"type":"array","items":{"type":"string"}} }, "required": ["paths"] },
  "output_schema": { "type": "object" },
  "permission_level": 3,
  "risk": "high",
  "timeout_ms": 30000,
  "destructive": true
}
```

Unknown tools, tools with a schema that does not validate, or arguments that fail
validation are rejected **before** execution.

## 3. Autonomy levels

| Level | Behaviour |
|---|---|
| 1 | answer only — no tool calls except level 0 |
| 2 | suggest actions, ask before each |
| 3 | execute level 0–1 automatically, confirm 2+ *(default)* |
| 4 | execute 0–2 automatically, confirm 3+ |
| 5 | trusted automation: 0–3 automatic via user-authored rules; level 4 still confirms |

Autonomy can only be raised explicitly by the user (UI/voice confirmation). It is
displayed in the UI at all times.

## 4. Confirmation flow

```text
tool needs confirmation
  → gate emits PermissionRequest {tool, args summary, level, risk, explanation}
  → UI/voice: "I am about to permanently delete 47 files. Continue? [Cancel] [Confirm]"
  → Confirm → execute once (grant is not persistent unless saved as a rule)
  → Cancel  → tool returns {ok:false, error:"user_declined"}; the brain must report it honestly
  → timeout → treated as Cancel
```

Trusted rules: a user may store `{tool, argument pattern, scope, expiry}` to skip
repeated confirmations. Rules are visible, editable, revocable, and never auto-created
by the model.

## 5. Emergency stop

Global and unconditional: cancels the current task token, flushes the queue, stops
speech, releases keyboard/mouse capture, and blocks further tool execution until the
user re-enables. Triggered by voice ("stop"), UI button, or hotkey. It works even if
the brain process is hung, because it is enforced in the Rust runtime.

## 6. Platform-specific notes

- **Windows**: input injection and process control run in the user's session; UAC
  elevation is never requested silently.
- **Android**: each capability maps to an Android permission the user enables manually
  in system settings; accessibility is disclosed with an explanation screen and can be
  turned off at any time.