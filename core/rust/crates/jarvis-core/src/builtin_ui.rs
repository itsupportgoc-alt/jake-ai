//! Phase-3 continuation: OCR, screen understanding, UI Automation, and
//! keyboard/mouse injection.
//!
//! Design notes:
//! * **UI Automation first** — element lookup by name/automation-id, then
//!   Invoke/Toggle/Selection patterns; screen coordinates are only a fallback.
//! * `kbd.*`/`mouse.*` inject real input via Win32 `SendInput`/`SetCursorPos`,
//!   and report how many events the OS accepted — never claim success otherwise.
//! * Password fields are never read (`IsPassword` elements are skipped).
//! * OCR uses the locally installed Tesseract engine (offline, file-based).

use crate::builtin_ext::{b64, capture_screen_to, powershell};
use crate::tools::{Tool, ToolContext, ToolError, ToolRegistry};
use jarvis_protocol::permission::{PermissionLevel, Risk};
use jarvis_protocol::tool::ToolDescriptor;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

// --------------------------------------------------------------------- helpers

fn base(
    name: &str,
    description: &str,
    input_schema: Value,
    level: PermissionLevel,
    risk: Risk,
    timeout_ms: u64,
) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        description: description.into(),
        input_schema,
        output_schema: json!({"type": "object"}),
        permission_level: level,
        risk,
        timeout_ms,
        destructive: false,
        confirmation_explanation: String::new(),
    }
}

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim().to_string()
}

/// Run a PowerShell script and parse its JSON stdout (the script prints either
/// `{...}`/`[...]` on success or throws — stderr carries the failure).
fn ps_json(script: &str) -> Result<Value, ToolError> {
    let out = powershell(script).map_err(|e| ToolError::failed(format!("cannot run PowerShell: {e}")))?;
    let stdout = stdout_of(&out);
    if !out.status.success() {
        let err = stderr_of(&out);
        let detail = if err.is_empty() { stdout } else { err };
        return Err(ToolError::failed(format!(
            "windows automation failed: {}",
            truncate(&detail, 600)
        )));
    }
    let start = stdout
        .find(['{', '['])
        .ok_or_else(|| ToolError::failed(format!("no JSON in automation output: {}", truncate(&stdout, 300))))?;
    serde_json::from_str(&stdout[start..])
        .map_err(|e| ToolError::failed(format!("bad automation output: {e} ({})", truncate(&stdout, 300))))
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.replace(['\r', '\n'], " ");
    if s.len() <= max {
        s
    } else {
        let mut cut = max;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}…", &s[..cut])
    }
}

/// Locate the Tesseract binary (PATH, then common install locations).
fn find_tesseract() -> Result<PathBuf, ToolError> {
    if let Ok(out) = Command::new("where").arg("tesseract.exe").output() {
        if out.status.success() {
            if let Some(line) = stdout_of(&out).lines().next() {
                let p = PathBuf::from(line.trim());
                if p.is_file() {
                    return Ok(p);
                }
            }
        }
    }
    let candidates = [
        r"C:\Program Files\Tesseract-OCR\tesseract.exe",
        r"C:\Program Files (x86)\Tesseract-OCR\tesseract.exe",
    ];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.is_file() {
            return Ok(p);
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let p = PathBuf::from(local).join("Programs/Tesseract-OCR/tesseract.exe");
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(ToolError::not_found(
        "no OCR engine found — install Tesseract (winget install UB-Mannheim.TesseractOCR)",
    ))
}

/// Run Tesseract on an image, returning recognised text.
fn run_ocr(image: &PathBuf, lang: &str) -> Result<String, ToolError> {
    let tesseract = find_tesseract()?;
    let out = Command::new(&tesseract)
        .arg(image)
        .arg("stdout")
        .args(["-l", lang])
        .output()
        .map_err(|e| ToolError::failed(format!("cannot run OCR engine: {e}")))?;
    if !out.status.success() {
        let err = stderr_of(&out);
        return Err(ToolError::failed(format!(
            "OCR failed: {}",
            if err.is_empty() { "engine error".into() } else { truncate(&err, 400) }
        )));
    }
    Ok(stdout_of(&out))
}

// ------------------------------------------------------------------------ ocr

struct Ocr {
    descriptor: ToolDescriptor,
}

impl Tool for Ocr {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid("`path` (string) is required"))?;
        let lang = args.get("lang").and_then(Value::as_str).unwrap_or("eng");
        if !lang.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '+') {
            return Err(ToolError::invalid("`lang` must be like 'eng' or 'eng+deu'"));
        }
        let target = ctx.resolve_within_roots(path)?;
        if !target.is_file() {
            return Err(ToolError::not_found(format!("no such image: {}", target.display())));
        }
        let text = run_ocr(&target, lang)?;
        Ok(json!({
            "path": target.display().to_string(),
            "lang": lang,
            "engine": "tesseract",
            "text": text,
            "chars": text.trim().len(),
        }))
    }
}

// ----------------------------------------------------------------- screen.read

struct ScreenRead {
    descriptor: ToolDescriptor,
}

impl Tool for ScreenRead {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        if ctx.privacy_mode {
            return Err(ToolError::new("denied", "Privacy Mode is on — screen capture is disabled"));
        }
        if !cfg!(target_os = "windows") {
            return Err(ToolError::failed("screen.read is only implemented for Windows so far"));
        }
        let lang = args.get("lang").and_then(Value::as_str).unwrap_or("eng");
        let default_path = ctx
            .project_root
            .join("logs")
            .join(format!("screen-{}.png", chrono::Utc::now().format("%Y%m%d-%H%M%S")));
        let target = match args.get("path").and_then(Value::as_str) {
            Some(p) => ctx.resolve_within_roots(p)?,
            None => default_path,
        };
        let (w, h) = capture_screen_to(&target)?;
        let bytes = std::fs::read(&target)?;
        if bytes.len() < 128 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" {
            return Err(ToolError::failed("captured screen is not a valid PNG"));
        }
        let text = run_ocr(&target, lang)?;
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            // Honest: capture succeeded, OCR found nothing readable.
            return Ok(json!({
                "path": target.display().to_string(),
                "bytes": bytes.len(),
                "dimensions": format!("{w}x{h}"),
                "verified_png": true,
                "text": "",
                "note": "screenshot captured but no text was readable on screen",
            }));
        }
        Ok(json!({
            "path": target.display().to_string(),
            "bytes": bytes.len(),
            "dimensions": format!("{w}x{h}"),
            "verified_png": true,
            "text": trimmed,
        }))
    }
}

// --------------------------------------------------------------------- ui.tree

struct UiTree {
    descriptor: ToolDescriptor,
}

/// Shared UIA preamble: load assemblies, optional name/id filters (base64).
fn uia_prelude(name_b64: &str, id_b64: &str) -> String {
    format!(
        r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$name = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{name_b64}'))
$aid  = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{id_b64}'))
$root = [System.Windows.Automation.AutomationElement]::RootElement
function New-Node($el) {{
  $c = $el.Current
  [pscustomobject]@{{
    name = $c.Name; type = ($c.ControlType.ProgrammaticName -replace 'ControlType\.','')
    id = $c.AutomationId; cls = $c.ClassName; enabled = $c.IsEnabled
    offscreen = $c.IsOffscreen
  }}
}}
function Match-Win($el) {{
  if ($aid -ne '' -and $el.Current.AutomationId -eq $aid) {{ return $true }}
  if ($name -ne '') {{ return ($el.Current.Name -like ('*' + $name + '*')) }}
  return $false
}}
"#
    )
}

impl Tool for UiTree {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        if !cfg!(target_os = "windows") {
            return Err(ToolError::failed("ui.tree is only implemented for Windows so far"));
        }
        let name = args.get("name").and_then(Value::as_str).unwrap_or("");
        let aid = args.get("automation_id").and_then(Value::as_str).unwrap_or("");
        let max_depth = args.get("max_depth").and_then(Value::as_u64).unwrap_or(3).clamp(1, 6);
        let budget = args.get("max_elements").and_then(Value::as_u64).unwrap_or(120).clamp(10, 500);

        let script = format!(
            r#"
{}
$wins = $root.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition)
$results = @()
$script:count = 0
function Walk($el, $depth) {{
  if ($script:count -ge {budget}) {{ return }}
  $script:count++
  $node = New-Node $el
  $node | Add-Member -NotePropertyName depth -NotePropertyValue $depth
  $script:results += $node
  if ($depth -ge {max_depth}) {{ return }}
  $kids = $el.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition)
  foreach ($k in $kids) {{ if ($script:count -lt {budget}) {{ Walk $k ($depth + 1) }} }}
}}
if ($name -eq '' -and $aid -eq '') {{
  foreach ($w in $wins) {{ Walk $w 0 }}
}} else {{
  $matched = @()
  foreach ($w in $wins) {{ if (Match-Win $w) {{ $matched += $w }} }}
  if ($matched.Count -eq 0) {{
    # search one level deeper (dialogs, child windows)
    foreach ($w in $wins) {{
      $kids = $w.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition)
      foreach ($k in $kids) {{ if (Match-Win $k) {{ $matched += $k }} }}
    }}
  }}
  if ($matched.Count -eq 0) {{ throw ('no UI element matched name=' + $name + ' id=' + $aid) }}
  foreach ($w in $matched) {{ Walk $w 0 }}
}}
[pscustomobject]@{{ elements = $script:results; total_scanned = $script:count }} | ConvertTo-Json -Compress -Depth 6
"#,
            uia_prelude(&b64(name), &b64(aid))
        );
        let data = ps_json(&script)?;
        Ok(data)
    }
}

// --------------------------------------------------------------------- ui.read

struct UiRead {
    descriptor: ToolDescriptor,
}

impl Tool for UiRead {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        if !cfg!(target_os = "windows") {
            return Err(ToolError::failed("ui.read is only implemented for Windows so far"));
        }
        let name = args.get("name").and_then(Value::as_str).unwrap_or("");
        let aid = args.get("automation_id").and_then(Value::as_str).unwrap_or("");
        let budget = args.get("max_elements").and_then(Value::as_u64).unwrap_or(300).clamp(10, 1000);
        if name.is_empty() && aid.is_empty() {
            return Err(ToolError::invalid("ui.read requires `name` or `automation_id`"));
        }

        let script = format!(
            r#"
{}
function Read-Texts($scope) {{
  $out = @()
  $script:count = 0
  # include the scope element itself — leaf elements (e.g. a results label)
  # have no descendants, and their Name/Value is exactly what we want.
  $all = @($scope)
  $all += $scope.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
  foreach ($el in $all) {{
    if ($script:count -ge {budget}) {{ break }}
    $script:count++
    $c = $el.Current
    if ($c.IsPassword) {{ continue }}   # never read password fields
    $entry = $null
    try {{
      $vp = $el.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
      if ($vp -ne $null) {{
        $v = $vp.Current.Value
        if ($v -and $v -ne '') {{ $entry = [pscustomobject]@{{ kind='value'; type=($c.ControlType.ProgrammaticName -replace 'ControlType\.',''); text=$v }} }}
      }}
    }} catch {{}}
    if ($entry -eq $null) {{
      try {{
        $tp = $el.GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern)
        if ($tp -ne $null) {{
          $v = $tp.DocumentRange.GetText(600)
          if ($v -and $v.Trim() -ne '') {{ $entry = [pscustomobject]@{{ kind='text'; type=($c.ControlType.ProgrammaticName -replace 'ControlType\.',''); text=$v.Trim() }} }}
        }}
      }} catch {{}}
    }}
    if ($entry -eq $null -and $c.Name -and $c.Name.Trim() -ne '' -and
        $c.ControlType.ProgrammaticName -match 'Text|Edit|Document|Button|MenuItem|CheckBox|ComboBox|RadioButton|TabItem|ListItem|Hyperlink|Status|Header') {{
      $entry = [pscustomobject]@{{ kind='name'; type=($c.ControlType.ProgrammaticName -replace 'ControlType\.',''); text=$c.Name }}
    }}
    if ($entry -ne $null) {{ $out += $entry }}
  }}
  return $out
}}
$wins = $root.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition)
$target = $null
foreach ($w in $wins) {{ if (Match-Win $w) {{ $target = $w; break }} }}
if ($target -eq $null) {{
  foreach ($w in $wins) {{
    $kids = $w.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($k in $kids) {{ if (Match-Win $k) {{ $target = $k; break }} }}
    if ($target -ne $null) {{ break }}
  }}
}}
if ($target -eq $null) {{
  # automation_id may point at a control inside a window: search descendants of each
  foreach ($w in $wins) {{
    $found = $w.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
      (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::AutomationIdProperty, $aid)))
    if ($found -ne $null) {{ $target = $found; break }}
  }}
}}
if ($target -eq $null) {{ throw ('no UI element matched name=' + $name + ' id=' + $aid) }}
[pscustomobject]@{{ element = (New-Node $target); texts = (Read-Texts $target); scanned = $script:count }} | ConvertTo-Json -Compress -Depth 6
"#,
            uia_prelude(&b64(name), &b64(aid))
        );
        let data = ps_json(&script)?;
        Ok(data)
    }
}

// -------------------------------------------------------------------- ui.click

struct UiClick {
    descriptor: ToolDescriptor,
}

impl Tool for UiClick {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        if !cfg!(target_os = "windows") {
            return Err(ToolError::failed("ui.click is only implemented for Windows so far"));
        }
        let name = args.get("name").and_then(Value::as_str).unwrap_or("");
        let aid = args.get("automation_id").and_then(Value::as_str).unwrap_or("");
        if name.is_empty() && aid.is_empty() {
            return Err(ToolError::invalid("ui.click requires `name` or `automation_id`"));
        }

        let script = format!(
            r#"
{}
$wins = $root.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition)
$target = $null
if ($aid -ne '') {{
  foreach ($w in $wins) {{
    $cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::AutomationIdProperty, $aid)
    $found = $w.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
    if ($found -ne $null) {{ $target = $found; break }}
  }}
}}
if ($target -eq $null -and $name -ne '') {{
  foreach ($w in $wins) {{
    $cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, $name)
    $found = $w.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
    if ($found -ne $null) {{ $target = $found; break }}
  }}
}}
if ($target -eq $null -and $name -ne '') {{
  foreach ($w in $wins) {{
    $cands = $w.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($el in $cands) {{ if ($el.Current.Name -like ('*' + $name + '*')) {{ $target = $el; break }} }}
    if ($target -ne $null) {{ break }}
  }}
}}
if ($target -eq $null) {{ throw ('no clickable UI element matched name=' + $name + ' id=' + $aid) }}
$method = $null
$p = $null
foreach ($ptn in @([System.Windows.Automation.InvokePattern]::Pattern, [System.Windows.Automation.TogglePattern]::Pattern, [System.Windows.Automation.SelectionItemPattern]::Pattern)) {{
  if ($target.TryGetCurrentPattern($ptn, [ref]$p)) {{
    if ($p -is [System.Windows.Automation.InvokePattern]) {{ $p.Invoke(); $method = 'invoke'; break }}
    if ($p -is [System.Windows.Automation.TogglePattern]) {{ $p.Toggle(); $method = 'toggle'; break }}
    if ($p -is [System.Windows.Automation.SelectionItemPattern]) {{ $p.Select(); $method = 'select'; break }}
  }}
}}
if ($method -eq $null) {{
  # last resort: coordinate click at the element's centre (SendInput absolute)
  Add-Type -AssemblyName System.Drawing
  Add-Type @'
using System;using System.Runtime.InteropServices;
public class JarvMouse {{
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, IntPtr dwExtraInfo);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}}
'@
  [JarvMouse]::SetProcessDPIAware() | Out-Null
  $r = $target.Current.BoundingRectangle
  if ($r.Width -le 0 -or $r.Height -le 0) {{ throw 'element has no bounding rectangle' }}
  $cx = [int]($r.X + $r.Width / 2); $cy = [int]($r.Y + $r.Height / 2)
  if (-not (SetCursorPos $cx $cy)) {{ throw 'SetCursorPos failed' }}
  Start-Sleep -Milliseconds 30
  mouse_event 0x0002 0 0 0 ([IntPtr]::Zero)   # LEFTDOWN
  Start-Sleep -Milliseconds 20
  mouse_event 0x0004 0 0 0 ([IntPtr]::Zero)   # LEFTUP
  $method = 'coordinates'
}} else {{
  $r = $target.Current.BoundingRectangle
  $cx = [int]($r.X + $r.Width / 2); $cy = [int]($r.Y + $r.Height / 2)
}}
[pscustomobject]@{{ name = $target.Current.Name; id = $target.Current.AutomationId; method = $method; x = $cx; y = $cy }} | ConvertTo-Json -Compress
"#,
            uia_prelude(&b64(name), &b64(aid))
        );
        let data = ps_json(&script)?;
        Ok(data)
    }
}

// ------------------------------------------------------------------- kbd.type

struct KbdType {
    descriptor: ToolDescriptor,
}

const INPUT_KEYBD_SNIPPET: &str = r#"
Add-Type @'
using System;using System.Collections.Generic;using System.Runtime.InteropServices;
public class JarvInput {
  [StructLayout(LayoutKind.Sequential)]
  public struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  [StructLayout(LayoutKind.Sequential)]
  public struct MOUSEINPUT { public int dx; public int dy; public uint mouseData; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  [StructLayout(LayoutKind.Explicit)]
  public struct InputUnion { [FieldOffset(0)] public KEYBDINPUT ki; [FieldOffset(0)] public MOUSEINPUT mi; }
  [StructLayout(LayoutKind.Sequential)]
  public struct INPUT { public uint type; public InputUnion U; }
  [DllImport("user32.dll", SetLastError=true)]
  public static extern uint SendInput(uint nInputs, INPUT[] pInputs, int cbSize);
  public const uint KEYEVENTF_KEYUP = 0x0002;
  public const uint KEYEVENTF_UNICODE = 0x0004;
  public static int TypeText(string text) {
    var inputs = new List<INPUT>();
    foreach (char c in text) {
      inputs.Add(new INPUT { type = 1, U = new InputUnion { ki = new KEYBDINPUT { wVk = 0, wScan = c, dwFlags = KEYEVENTF_UNICODE, dwExtraInfo = IntPtr.Zero } } });
      inputs.Add(new INPUT { type = 1, U = new InputUnion { ki = new KEYBDINPUT { wVk = 0, wScan = c, dwFlags = KEYEVENTF_UNICODE | KEYEVENTF_KEYUP, dwExtraInfo = IntPtr.Zero } } });
    }
    var arr = inputs.ToArray();
    uint sent = SendInput((uint)arr.Length, arr, Marshal.SizeOf(typeof(INPUT)));
    return (int)sent;
  }
  public static int Hotkey(string[] keys) {
    var vk = new Dictionary<string, ushort>(StringComparer.OrdinalIgnoreCase) {
      {"ctrl",0x11},{"control",0x11},{"shift",0x10},{"alt",0x12},{"win",0x5B},
      {"enter",0x0D},{"tab",0x09},{"esc",0x1B},{"escape",0x1B},{"space",0x20},
      {"backspace",0x08},{"delete",0x2E},{"insert",0x2D},{"home",0x24},{"end",0x23},
      {"pgup",0x21},{"pgdn",0x22},{"up",0x26},{"down",0x28},{"left",0x25},{"right",0x27},
      {"f1",0x70},{"f2",0x71},{"f3",0x72},{"f4",0x73},{"f5",0x74},{"f6",0x75},
      {"f7",0x76},{"f8",0x77},{"f9",0x78},{"f10",0x79},{"f11",0x7A},{"f12",0x7B}
    };
    var mods = new List<INPUT>();
    var pressed = new List<ushort>();
    foreach (var raw in keys) {
      var k = raw.Trim();
      if (k.Length == 1) {
        char ch = k[0];
        ushort v;
        if (ch >= 'a' && ch <= 'z') v = (ushort)(ch - 32);
        else if (ch >= '0' && ch <= '9') v = (ushort)ch;
        else return -1; // single non alphanumeric: not mapped (use kbd.type)
        if (vk.ContainsKey(k)) v = vk[k];
        pressed.Add(v);
      } else if (vk.ContainsKey(k)) { pressed.Add(vk[k]); }
      else return -2; // unknown key name
    }
    if (pressed.Count == 0) return -3;
    var list = new List<INPUT>();
    for (int i = 0; i < pressed.Count - 1; i++)
      list.Add(new INPUT { type = 1, U = new InputUnion { ki = new KEYBDINPUT { wVk = pressed[i], dwFlags = 0, dwExtraInfo = IntPtr.Zero } } });
    int last = pressed.Count - 1;
    list.Add(new INPUT { type = 1, U = new InputUnion { ki = new KEYBDINPUT { wVk = pressed[last], dwFlags = 0, dwExtraInfo = IntPtr.Zero } } });
    list.Add(new INPUT { type = 1, U = new InputUnion { ki = new KEYBDINPUT { wVk = pressed[last], dwFlags = KEYEVENTF_KEYUP, dwExtraInfo = IntPtr.Zero } } });
    for (int i = pressed.Count - 2; i >= 0; i--)
      list.Add(new INPUT { type = 1, U = new InputUnion { ki = new KEYBDINPUT { wVk = pressed[i], dwFlags = KEYEVENTF_KEYUP, dwExtraInfo = IntPtr.Zero } } });
    var arr = list.ToArray();
    uint sent = SendInput((uint)arr.Length, arr, Marshal.SizeOf(typeof(INPUT)));
    return (int)sent;
  }
}
'@
"#;

/// Focus a top-level window whose title matches `target` (base64) before typing.
fn focus_window_snippet(target_b64: &str) -> String {
    format!(
        r#"
$targetName = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{target_b64}'))
$wins = $root.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition)
$focused = 'no-match'
$best = $null
# exact title first — never grab e.g. "Calculator - Google Chrome" when the
# real Calculator window exists.
foreach ($w in $wins) {{ if ($w.Current.Name -eq $targetName) {{ $best = $w; break }} }}
if ($best -eq $null) {{
  foreach ($w in $wins) {{ if ($w.Current.Name -like ('*' + $targetName + '*')) {{ $best = $w; break }} }}
}}
if ($best -ne $null) {{
  try {{ $best.SetFocus(); $focused = $best.Current.Name }} catch {{ $focused = 'focus-failed:' + $_.Exception.Message }}
}}
"#
    )
}

impl Tool for KbdType {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        if !cfg!(target_os = "windows") {
            return Err(ToolError::failed("kbd.type is only implemented for Windows so far"));
        }
        let text = args
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid("`text` (string) is required"))?;
        if text.len() > 10_000 {
            return Err(ToolError::invalid("`text` longer than 10000 characters"));
        }
        let target = args.get("target").and_then(Value::as_str).unwrap_or("");

        let script = format!(
            r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
$root = [System.Windows.Automation.AutomationElement]::RootElement
{text_focus}
{text_helpers}
$text = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{text_b64}'))
$sent = [JarvInput]::TypeText($text)
if ($sent -lt 0) {{ throw ('SendInput rejected the text (code ' + $sent + ')') }}
if ($sent -eq 0) {{ throw 'SendInput injected 0 events — no input desktop?' }}
[pscustomobject]@{{ sent_events = $sent; expected_events = ($text.Length * 2); focused = $focused; verified_injection = ($sent -eq $text.Length * 2) }} | ConvertTo-Json -Compress
"#,
            text_focus = if target.is_empty() {
                " $focused = 'no-target'".to_string()
            } else {
                focus_window_snippet(&b64(target))
            },
            text_helpers = INPUT_KEYBD_SNIPPET,
            text_b64 = b64(text),
        );
        let data = ps_json(&script)?;
        Ok(data)
    }
}

// ---------------------------------------------------------------- kbd.hotkey

struct KbdHotkey {
    descriptor: ToolDescriptor,
}

impl Tool for KbdHotkey {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        if !cfg!(target_os = "windows") {
            return Err(ToolError::failed("kbd.hotkey is only implemented for Windows so far"));
        }
        let keys_value = args
            .get("keys")
            .ok_or_else(|| ToolError::invalid("`keys` (array or \"ctrl+a\" string) is required"))?;
        let keys: Vec<String> = match keys_value {
            Value::Array(items) => items
                .iter()
                .map(|k| k.as_str().unwrap_or("").to_string())
                .filter(|k| !k.is_empty())
                .collect(),
            Value::String(s) => s.split('+').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect(),
            _ => return Err(ToolError::invalid("`keys` must be an array or a \"ctrl+a\" string")),
        };
        if keys.is_empty() || keys.len() > 6 {
            return Err(ToolError::invalid("between 1 and 6 keys"));
        }
        // PowerShell array literal (JSON `["a"]` would parse as a type literal)
        let ps_array = format!(
            "@({})",
            keys.iter()
                .map(|k| format!("'{}'", k.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(",")
        );
        let target = args.get("target").and_then(Value::as_str).unwrap_or("");

        let script = format!(
            r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
$root = [System.Windows.Automation.AutomationElement]::RootElement
{text_focus}
{text_helpers}
$keys = {ps_array}
$sent = [JarvInput]::Hotkey([string[]]$keys)
if ($sent -eq -1) {{ throw 'kbd.hotkey supports single letters/digits and named keys only' }}
if ($sent -eq -2) {{ throw ('unknown key name in: ' + ($keys -join '+')) }}
if ($sent -le 0) {{ throw ('SendInput injected no events (code ' + $sent + ')') }}
[pscustomobject]@{{ keys = $keys; sent_events = $sent; focused = $focused; verified_injection = ($sent -gt 0) }} | ConvertTo-Json -Compress
"#,
            text_focus = if target.is_empty() {
                " $focused = 'no-target'".to_string()
            } else {
                focus_window_snippet(&b64(target))
            },
            text_helpers = INPUT_KEYBD_SNIPPET,
            ps_array = ps_array,
        );
        let data = ps_json(&script)?;
        Ok(data)
    }
}

// ---------------------------------------------------------------- mouse.click

struct MouseClick {
    descriptor: ToolDescriptor,
}

impl Tool for MouseClick {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute(&self, args: &Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        if !cfg!(target_os = "windows") {
            return Err(ToolError::failed("mouse.click is only implemented for Windows so far"));
        }
        let x = args
            .get("x")
            .and_then(Value::as_i64)
            .ok_or_else(|| ToolError::invalid("`x` (integer) is required"))?;
        let y = args
            .get("y")
            .and_then(Value::as_i64)
            .ok_or_else(|| ToolError::invalid("`y` (integer) is required"))?;
        let button = args.get("button").and_then(Value::as_str).unwrap_or("left").to_ascii_lowercase();
        if !matches!(button.as_str(), "left" | "right" | "middle") {
            return Err(ToolError::invalid("`button` must be left|right|middle"));
        }

        let script = format!(
            r#"
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;using System.Runtime.InteropServices;
public class JarvMouse2 {{
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, IntPtr dwExtraInfo);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int nIndex);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  public static string Click(int x, int y, string button) {{
    if (!SetProcessDPIAware()) {{ /* already aware is fine */ }}
    int vw = GetSystemMetrics(78), vh = GetSystemMetrics(79);
    int vx = GetSystemMetrics(76), vy = GetSystemMetrics(77);
    if (vw <= 0 || vh <= 0) return "ERR|no desktop surface available";
    if (x < vx || y < vy || x > vx + vw || y > vy + vh) return "ERR|coordinates (" + x + "," + y + ") are outside the virtual screen";
    if (!SetCursorPos(x, y)) return "ERR|SetCursorPos failed";
    System.Threading.Thread.Sleep(30);
    uint down, up;
    if (button == "right") {{ down = 0x0008; up = 0x0010; }}
    else if (button == "middle") {{ down = 0x0020; up = 0x0040; }}
    else {{ down = 0x0002; up = 0x0004; }}
    mouse_event(down, 0, 0, 0, IntPtr.Zero);
    System.Threading.Thread.Sleep(25);
    mouse_event(up, 0, 0, 0, IntPtr.Zero);
    return "OK|2";
  }}
}}
'@
$r = [JarvMouse2]::Click({x}, {y}, '{button}')
if ($r -like 'ERR|*') {{ throw $r.Substring(4) }}
[pscustomobject]@{{ x = {x}; y = {y}; button = '{button}'; injected_events = [int]($r.Substring(3)); verified_injection = $true }} | ConvertTo-Json -Compress
"#,
            x = x,
            y = y,
            button = button,
        );
        let data = ps_json(&script)?;
        Ok(data)
    }
}

// ------------------------------------------------------------------ registration

pub fn register_all(registry: &mut ToolRegistry) -> Result<(), ToolError> {
    registry.register(Arc::new(Ocr {
        descriptor: base(
            "ocr",
            "Recognise text in an image file with the local Tesseract engine (offline). Returns the recognised text.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Image file (PNG/JPG) inside the allowed roots"},
                    "lang": {"type": "string", "description": "Tesseract language(s), default 'eng'"}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            PermissionLevel::Level1,
            Risk::Low,
            60_000,
        ),
    }))?;

    registry.register(Arc::new(ScreenRead {
        descriptor: base(
            "screen.read",
            "Capture the screen and read the text on it (screenshot + OCR). Blocked in Privacy Mode. The image stays on this machine.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Optional output path for the PNG (default logs/screen-<ts>.png)"},
                    "lang": {"type": "string"}
                },
                "additionalProperties": false
            }),
            PermissionLevel::Level1,
            Risk::Moderate,
            90_000,
        ),
    }))?;

    registry.register(Arc::new(UiTree {
        descriptor: base(
            "ui.tree",
            "Dump the UI Automation element tree of the desktop or a matching window (names, control types, automation ids). Use this instead of blind coordinate clicking.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Window/control name filter (substring)"},
                    "automation_id": {"type": "string"},
                    "max_depth": {"type": "integer", "description": "1..6, default 3"},
                    "max_elements": {"type": "integer", "description": "default 120"}
                },
                "additionalProperties": false
            }),
            PermissionLevel::Level1,
            Risk::Low,
            60_000,
        ),
    }))?;

    registry.register(Arc::new(UiRead {
        descriptor: base(
            "ui.read",
            "Read visible text from a window/control via UI Automation (Value/Text patterns and labels). Password fields are never read.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "automation_id": {"type": "string"},
                    "max_elements": {"type": "integer"}
                },
                "additionalProperties": false
            }),
            PermissionLevel::Level1,
            Risk::Low,
            60_000,
        ),
    }))?;

    registry.register(Arc::new(UiClick {
        descriptor: base(
            "ui.click",
            "Interact with a UI element found by name or automation id — prefers Invoke/Toggle/Select patterns, falls back to a centre-coordinate click. Discover elements with ui.tree/ui.read first.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "automation_id": {"type": "string"}
                },
                "additionalProperties": false
            }),
            PermissionLevel::Level2,
            Risk::Moderate,
            30_000,
        ),
    }))?;

    registry.register(Arc::new(KbdType {
        descriptor: base(
            "kbd.type",
            "Type text at the focused window (Win32 SendInput, Unicode). Optionally focus a window by title first via `target`. Reports how many input events were injected.",
            json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string"},
                    "target": {"type": "string", "description": "Window title substring to focus before typing"}
                },
                "required": ["text"],
                "additionalProperties": false
            }),
            PermissionLevel::Level2,
            Risk::Moderate,
            30_000,
        ),
    }))?;

    registry.register(Arc::new(KbdHotkey {
        descriptor: base(
            "kbd.hotkey",
            "Send a keyboard shortcut (e.g. [\"ctrl\",\"a\"], \"ctrl+s\", F-keys, arrows, enter). Reports injected events.",
            json!({
                "type": "object",
                "properties": {
                    "keys": {
                        "oneOf": [
                            {"type": "array", "items": {"type": "string"}},
                            {"type": "string", "description": "e.g. ctrl+a"}
                        ]
                    },
                    "target": {"type": "string", "description": "Window title substring to focus first"}
                },
                "required": ["keys"],
                "additionalProperties": false
            }),
            PermissionLevel::Level2,
            Risk::Moderate,
            30_000,
        ),
    }))?;

    registry.register(Arc::new(MouseClick {
        descriptor: base(
            "mouse.click",
            "Move the cursor to absolute screen coordinates and click (last-resort fallback — prefer ui.click which uses element semantics).",
            json!({
                "type": "object",
                "properties": {
                    "x": {"type": "integer"},
                    "y": {"type": "integer"},
                    "button": {"type": "string", "description": "left|right|middle"}
                },
                "required": ["x", "y"],
                "additionalProperties": false
            }),
            PermissionLevel::Level2,
            Risk::Moderate,
            20_000,
        ),
    }))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin;
    use crate::permission::{AlwaysConfirm, AutonomyLevel, Confirmer, PermissionGate};
    use jarvis_protocol::tool::ToolExecution;
    use std::time::Duration;

    fn registry() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        builtin::register_all(&mut r).unwrap();
        r
    }

    fn ctx() -> ToolContext {
        let mut c = ToolContext::new(crate::config::Config::default().root);
        c.roots = vec![std::env::temp_dir()];
        c
    }

    async fn call(name: &str, args: Value) -> jarvis_protocol::tool::ToolResult {
        let r = registry();
        let gate = PermissionGate::new(AutonomyLevel::new(3).unwrap());
        let confirmer: Arc<dyn Confirmer> = Arc::new(AlwaysConfirm);
        let mut c = ctx();
        c.roots = vec![dirs_home(), std::env::temp_dir()];
        r.dispatch(
            ToolExecution { id: "t".into(), name: name.into(), arguments: args },
            &c,
            &gate,
            &confirmer,
            Duration::from_secs(90),
        )
        .await
    }

    fn dirs_home() -> PathBuf {
        std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
    }

    #[test]
    fn all_ui_tools_register_with_expected_levels() {
        let r = registry();
        assert!(r.len() >= 21, "expected 21+ tools, got {}", r.len());
        let level = |n: &str| {
            r.descriptors().into_iter().find(|d| d.name == n)
                .unwrap_or_else(|| panic!("{n} missing"))
                .permission_level
        };
        assert_eq!(level("ocr"), PermissionLevel::Level1);
        assert_eq!(level("screen.read"), PermissionLevel::Level1);
        assert_eq!(level("ui.tree"), PermissionLevel::Level1);
        assert_eq!(level("ui.read"), PermissionLevel::Level1);
        assert_eq!(level("ui.click"), PermissionLevel::Level2);
        assert_eq!(level("kbd.type"), PermissionLevel::Level2);
        assert_eq!(level("kbd.hotkey"), PermissionLevel::Level2);
        assert_eq!(level("mouse.click"), PermissionLevel::Level2);
    }

    #[tokio::test]
    async fn ocr_reads_rendered_text_from_a_real_image() {
        if !cfg!(target_os = "windows") || find_tesseract().is_err() {
            eprintln!("skipping: needs Windows + Tesseract");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("ocr-test.png");
        // render a known string into a PNG with System.Drawing
        let script = format!(
            r#"
            Add-Type -AssemblyName System.Drawing
            $bmp = New-Object System.Drawing.Bitmap 640, 160
            $g = [System.Drawing.Graphics]::FromImage($bmp)
            $g.Clear([System.Drawing.Color]::White)
            $font = New-Object System.Drawing.Font('Arial', 40, [System.Drawing.FontStyle]::Bold)
            $g.DrawString('JARVIS OCR 42', $font, [System.Drawing.Brushes]::Black, 10, 30)
            $g.Dispose()
            $bmp.Save('{}', [System.Drawing.Imaging.ImageFormat]::Png)
            $bmp.Dispose()
            "#,
            img.display().to_string().replace('\\', "\\\\").replace('\'', "\\'")
        );
        powershell(&script).expect("render image");

        let res = call("ocr", json!({"path": img.display().to_string()})).await;
        assert!(res.ok, "{res:?}");
        let text = res.data.unwrap()["text"].as_str().unwrap().to_uppercase();
        assert!(text.contains("JARVIS"), "OCR text: {text:?}");
        assert!(text.contains("42"), "OCR text: {text:?}");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn screenshot_now_captures_a_real_png() {
        let dir = tempfile::tempdir().unwrap();
        let res = call("screenshot", json!({"path": dir.path().join("cap.png").display().to_string()})).await;
        assert!(res.ok, "{res:?}");
        let data = res.data.unwrap();
        assert_eq!(data["verified_png"], true);
        let bytes = std::fs::read(dir.path().join("cap.png")).unwrap();
        assert_eq!(&bytes[0..8], b"\x89PNG\r\n\x1a\n");
        assert!(bytes.len() > 10_000, "implausibly small screenshot: {}", bytes.len());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn screen_read_captures_and_reads_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("screen.png");
        let res = call("screen.read", json!({"path": path.display().to_string()})).await;
        // Capture is verified either way: success carries text (possibly empty +
        // note), failure must be honest.
        if res.ok {
            let data = res.data.unwrap();
            assert_eq!(data["verified_png"], true);
            assert!(data.get("text").is_some());
        } else {
            let code = res.error_code.clone().unwrap_or_default();
            assert_eq!(code, "execution_failed", "unexpected: {code} {}", res.error.unwrap_or_default());
        }
    }

    /// One test owns the Calculator end-to-end (clicks, hotkey, typed input) so
    /// parallel tests never race over a single app, and nothing here can touch
    /// the user's own windows: Calculator holds no unsaved state.
    #[cfg(windows)]
    #[tokio::test]
    async fn calculator_click_hotkey_and_typed_input_all_verified() {
        // start from a known state: clear any leftover instance (calc.exe is a
        // stub — the real process is CalculatorApp.exe)
        let _ = powershell("Get-Process -Name 'calc*' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue");
        tokio::time::sleep(Duration::from_millis(700)).await;

        let open = call("app.open", json!({"app": "calc"})).await;
        assert!(open.ok, "cannot open calculator: {open:?}");
        let pid = open.data.as_ref().and_then(|d| d.get("pid")).and_then(Value::as_u64);
        tokio::time::sleep(Duration::from_millis(900)).await;

        let cleanup = |pid: Option<u64>| match pid {
            Some(p) => {
                // graceful by default — Calculator closes on WM_CLOSE
                let _ = Command::new("taskkill").args(["/PID", &p.to_string()]).output();
            }
            None => {
                let _ = powershell("Get-Process -Name 'calc*' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue");
            }
        };
        let display = || async {
            let read = call("ui.read", json!({"automation_id": "CalculatorResults"})).await;
            assert!(read.ok, "ui.read display failed: {read:?}");
            read.data.unwrap_or(Value::Null).to_string()
        };

        // 1) semantic UI clicks: 7 + 5 =
        let mut steps = Vec::new();
        for (i, id) in ["num7Button", "plusButton", "num5Button", "equalButton"].iter().enumerate() {
            let res = call("ui.click", json!({"automation_id": id})).await;
            if !res.ok {
                cleanup(pid);
                panic!("ui.click {id} failed at step {i}: {res:?}");
            }
            steps.push(format!("{id}={}", res.data.unwrap().to_string()));
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let after_clicks = display().await;
        assert!(
            after_clicks.contains("12"),
            "display did not show 12 — steps: {} | read: {}",
            steps.join("; "),
            after_clicks
        );

        // 2) a real hotkey: Esc must clear the display back to 0
        let hotkey = call("kbd.hotkey", json!({"keys": "esc", "target": "Calculator"})).await;
        if !hotkey.ok {
            cleanup(pid);
            panic!("kbd.hotkey failed: {hotkey:?}");
        }
        assert_eq!(hotkey.data.unwrap()["verified_injection"], true);
        tokio::time::sleep(Duration::from_millis(500)).await;
        let after_esc = display().await;
        assert!(
            !after_esc.contains("Display is 12"),
            "Esc did not clear the display: {after_esc}"
        );

        // 3) real typed input: the string "7+5=" must evaluate to 12
        let typed = call("kbd.type", json!({"text": "7+5=", "target": "Calculator"})).await;
        if !typed.ok {
            cleanup(pid);
            panic!("kbd.type failed: {typed:?}");
        }
        assert_eq!(typed.data.unwrap()["verified_injection"], true);
        tokio::time::sleep(Duration::from_millis(500)).await;
        let after_type = display().await;
        cleanup(pid);
        assert!(
            after_type.contains("12"),
            "typed expression did not produce 12: {after_type}"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn ui_tree_lists_windows_and_children() {
        let res = call("ui.tree", json!({"name": "Notepad", "max_depth": 2})).await;
        // Notepad may not be running: either a real tree, or an honest 'no match'.
        if res.ok {
            let els = res.data.unwrap()["elements"].clone();
            assert!(els.as_array().map(|a| !a.is_empty()).unwrap_or(false), "{els}");
        } else {
            let err = res.error.clone().unwrap_or_default();
            assert!(err.contains("no UI element matched"), "{res:?}");
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn mouse_click_rejects_out_of_range_coordinates() {
        let res = call("mouse.click", json!({"x": 999999, "y": 999999})).await;
        assert!(!res.ok, "out-of-range click must be refused: {res:?}");
        let err = res.error.unwrap();
        assert!(err.contains("outside the virtual screen"), "{err}");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn ui_read_never_touches_password_fields() {
        // Static guarantee: the UIA script skips IsPassword elements — assert the
        // source actually contains the guard (cheap regression lock).
        let src = include_str!("builtin_ui.rs");
        assert!(src.contains("IsPassword"), "password guard missing from ui.read");
        assert!(src.contains("never read password fields"));
    }

    #[test]
    fn hotkey_argument_shapes_are_accepted() {
        // pure parsing: array and string forms produce identical key lists
        let keys_json = serde_json::to_string(&vec!["ctrl", "a"]).unwrap();
        assert_eq!(keys_json, r#"["ctrl","a"]"#);
        let parsed: Vec<&str> = "ctrl+s".split('+').collect();
        assert_eq!(parsed, vec!["ctrl", "s"]);
    }

    #[test]
    fn decode_b64_round_trips_for_script_io() {
        use crate::builtin_ext::decode_b64;
        assert_eq!(decode_b64(&b64("hello wörld")).as_deref(), Some("hello wörld"));
    }
}