import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import type { ToolInfo, WslStatus, Target } from '../../shared/types';
import type { Database } from './db';
import { isoNow } from './db';
import type { AuditLog } from './audit';
import { redact } from './redact';

/** Tools are invoked only through fixed argument builders. No free-form shell. */
export type ToolId = 'nmap' | 'nikto' | 'zap' | 'playwright' | 'builtin-web' | 'builtin-php' | 'builtin-mysql';

export interface RunToolResult {
  tool: string;
  command: string;
  status: 'completed' | 'failed' | 'timeout';
  exitCode: number | null;
  output: string;
  startedAt: string;
  finishedAt: string;
}

const MAX_OUTPUT = 400_000;
let cachedWsl: WslStatus | null = null;

function commandExists(cmd: string): boolean {
  const res = spawnSync(process.platform === 'win32' ? 'where' : 'which', [cmd], { encoding: 'utf8', timeout: 5000 });
  return res.status === 0 && res.stdout.trim().length > 0;
}

function versionOf(cmd: string, args: string[]): string | null {
  try {
    const res = spawnSync(cmd, args, { encoding: 'utf8', timeout: 8000, windowsHide: true });
    const out = `${res.stdout ?? ''} ${res.stderr ?? ''}`.trim();
    if (!out) return null;
    const m = /(\d+\.\d+(?:\.\d+)?)/.exec(out);
    return m ? m[1] : out.split(/\r?\n/)[0].slice(0, 80);
  } catch {
    return null;
  }
}

export function detectWsl(force = false): WslStatus {
  if (cachedWsl && !force) return cachedWsl;
  if (process.platform !== 'win32') {
    cachedWsl = { installed: false, distributions: [], ubuntu: false, notes: 'Not running on Windows; WSL detection is not applicable.' };
    return cachedWsl;
  }
  let distributions: string[] = [];
  let notes = '';
  try {
    const res = spawnSync('wsl.exe', ['-l', '-q'], { timeout: 8000, windowsHide: true });
    const raw = res.stdout ?? Buffer.alloc(0);
    const text = Buffer.from(raw).toString('utf16le').replace(/\u0000/g, '').replace(/\r/g, '');
    if (text.includes('no installed distributions') || text.includes('Windows Subsystem for Linux has no installed')) {
      notes = 'WSL is available but no Linux distribution is installed. Install Ubuntu 24.04 with: wsl --install -d Ubuntu-24.04';
    } else {
      distributions = text
        .split('\n')
        .map((s) => s.trim())
        .filter(Boolean);
      if (distributions.length) notes = `Detected: ${distributions.join(', ')}`;
      else notes = 'WSL returned no distributions.';
    }
  } catch (err) {
    notes = `WSL not available: ${(err as Error).message}`;
  }
  const ubuntu = distributions.some((d) => /ubuntu/i.test(d));
  cachedWsl = { installed: distributions.length > 0, distributions, ubuntu, notes };
  return cachedWsl;
}

function wslDistro(): string | null {
  const wsl = detectWsl();
  return wsl.distributions.find((d) => /ubuntu/i.test(d)) ?? wsl.distributions[0] ?? null;
}

function commonZapPaths(): string[] {
  const candidates: string[] = [];
  if (process.env.ZAP_HOME) candidates.push(process.env.ZAP_HOME);
  if (process.env.ZAPROXY_JAR) candidates.push(path.dirname(process.env.ZAPROXY_JAR));
  if (process.platform === 'win32') {
    candidates.push(
      'C:\\Program Files\\ZAP\\Zed Attack Proxy',
      'C:\\Program Files\\OWASP\\Zed Attack Proxy',
      'C:\\Program Files (x86)\\OWASP\\Zed Attack Proxy',
      path.join(process.env.LOCALAPPDATA ?? '', 'Programs', 'ZAP'),
    );
  }
  return candidates.filter((p) => p && fs.existsSync(p));
}

function findEdgeOrChrome(): string | null {
  const candidates =
    process.platform === 'win32'
      ? [
          'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
          'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
          'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
          'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
        ]
      : ['/usr/bin/google-chrome', '/usr/bin/chromium', '/usr/bin/chromium-browser'];
  return candidates.find((p) => fs.existsSync(p)) ?? null;
}

export function listTools(): ToolInfo[] {
  const wsl = detectWsl(true);
  const nmapWin = commandExists('nmap');
  const niktoWin = commandExists('nikto');
  const zapDirs = commonZapPaths();

  let nmapWsl: string | null = null;
  let niktoWsl: string | null = null;
  if (wsl.installed && wsl.ubuntu) {
    const distro = wslDistro()!;
    const has = (bin: string) =>
      spawnSync('wsl.exe', ['-d', distro, '--', 'bash', '-lc', `command -v ${bin} >/dev/null 2>&1 && ${bin} --version 2>&1 | head -n 1`], {
        timeout: 12000,
        windowsHide: true,
        encoding: 'utf8',
      });
    const n = has('nmap');
    if (n.status === 0) nmapWsl = (n.stdout || '').trim().slice(0, 80) || 'installed';
    const k = has('nikto');
    if (k.status === 0) niktoWsl = (k.stdout || '').trim().slice(0, 80) || 'installed';
  }

  const edge = findEdgeOrChrome();
  const wslNote = wsl.installed ? `WSL: ${wsl.distributions.join(', ')}` : wsl.notes;

  const tools: ToolInfo[] = [
    {
      id: 'nmap',
      name: 'Nmap',
      kind: 'binary',
      installed: nmapWin || Boolean(nmapWsl),
      version: nmapWin ? versionOf('nmap', ['--version']) : nmapWsl,
      location: nmapWin ? 'Windows PATH' : nmapWsl ? 'WSL (Ubuntu)' : null,
      via: nmapWin ? 'windows' : nmapWsl ? 'wsl' : 'none',
      notes: nmapWin || nmapWsl ? 'Top-100 TCP ports, version light, -Pn. No NSE scripts.' : 'Install on Windows (winget install Insecure.Nmap) or in WSL (sudo apt install nmap).',
    },
    {
      id: 'nikto',
      name: 'Nikto',
      kind: 'binary',
      installed: niktoWin || Boolean(niktoWsl),
      version: niktoWin ? versionOf('nikto', ['-Version']) : niktoWsl,
      location: niktoWin ? 'Windows PATH' : niktoWsl ? 'WSL (Ubuntu)' : null,
      via: niktoWin ? 'windows' : niktoWsl ? 'wsl' : 'none',
      notes: niktoWin || niktoWsl ? 'Web server checks with a 120s cap.' : 'Install in WSL: sudo apt install nikto (Perl-based).',
    },
    {
      id: 'zap',
      name: 'OWASP ZAP',
      kind: 'binary',
      installed: zapDirs.length > 0,
      version: zapDirs.length ? 'ZAP installation detected' : null,
      location: zapDirs[0] ?? null,
      via: zapDirs.length ? 'windows' : 'none',
      notes: zapDirs.length
        ? 'Baseline scan runs zap-baseline.py (safe, passive rules) if Python is available.'
        : 'Install ZAP Desktop, then set ZAP_HOME. Baseline scanning requires zap-baseline.py.',
    },
    {
      id: 'playwright',
      name: 'Playwright (system browser)',
      kind: 'browser',
      installed: Boolean(edge),
      version: edge ? path.basename(edge) : null,
      location: edge,
      via: edge ? 'windows' : 'none',
      notes: edge ? 'Authorized login sessions use the system Edge/Chrome via playwright-core; no browser download required.' : 'Install Microsoft Edge or Google Chrome.',
    },
    {
      id: 'builtin-web',
      name: 'JAKE web assessment engine',
      kind: 'builtin',
      installed: true,
      version: '1.0',
      location: 'built-in',
      via: 'builtin',
      notes: 'Passive recon, OWASP-aligned safe checks, header/TLS/CORS/CSRF analysis.',
    },
    {
      id: 'builtin-php',
      name: 'JAKE PHP/MySQL static auditor',
      kind: 'builtin',
      installed: true,
      version: '1.0',
      location: 'built-in',
      via: 'builtin',
      notes: 'PHP source inspection and live read-only MySQL metadata review.',
    },
    {
      id: 'builtin-mysql',
      name: 'JAKE MySQL auditor',
      kind: 'builtin',
      installed: true,
      version: '1.0',
      location: 'built-in',
      via: 'builtin',
      notes: 'Read-only session, metadata and privilege review.',
    },
  ];
  tools.push({
    id: 'wsl',
    name: 'WSL / Ubuntu',
    kind: 'builtin',
    installed: wsl.ubuntu,
    version: wsl.distributions.join(', ') || null,
    location: wsl.installed ? 'wsl.exe' : null,
    via: wsl.installed ? 'wsl' : 'none',
    notes: wslNote,
  });
  return tools;
}

export interface ToolRunOptions {
  topPorts?: number;
}

export async function runSecurityTool(opts: {
  db: Database;
  audit: AuditLog;
  tool: ToolId;
  target: Target;
  host: string;
  url: string;
  options?: ToolRunOptions;
  signal?: AbortSignal;
  timeoutMs?: number;
}): Promise<RunToolResult> {
  const { db, audit, tool, target, host, url, signal } = opts;
  const info = listTools().find((t) => t.id === tool);
  if (!info) throw new Error(`Unknown tool: ${tool}`);
  if (['builtin-web', 'builtin-php', 'builtin-mysql'].includes(tool)) {
    throw new Error(`${info.name} runs through its dedicated module, not the external tool runner.`);
  }
  if (!info.installed) throw new Error(`${info.name} is not installed. ${info.notes}`);

  const safeHost = assertHost(host);
  const safeUrl = assertHttpUrl(url);
  const topPorts = Math.round(Math.min(1000, Math.max(10, opts.options?.topPorts ?? 100)));

  let command: string;
  let args: string[] = [];
  let display: string;

  if (tool === 'nmap') {
    if (info.via === 'wsl') {
      const distro = wslDistro();
      command = 'wsl.exe';
      args = ['-d', distro!, '--', 'nmap', '-Pn', '-T3', '--top-ports', String(topPorts), '-sV', '--version-light', safeHost];
    } else {
      command = 'nmap';
      args = ['-Pn', '-T3', '--top-ports', String(topPorts), '-sV', '--version-light', safeHost];
    }
    display = `nmap -Pn -T3 --top-ports ${topPorts} -sV --version-light ${safeHost}`;
  } else if (tool === 'nikto') {
    if (!scopeAllows(target, host)) throw new Error('Host is outside the authorized scope.');
    if (info.via === 'wsl') {
      const distro = wslDistro();
      command = 'wsl.exe';
      args = ['-d', distro!, '--', 'nikto', '-host', safeUrl, '-maxtime', '120s', '-nointeractive', '-ask', 'no'];
    } else {
      command = 'nikto';
      args = ['-host', safeUrl, '-maxtime', '120s', '-nointeractive', '-ask', 'no'];
    }
    display = `nikto -host ${safeUrl} -maxtime 120s -nointeractive`;
  } else if (tool === 'zap') {
    const dir = info.location ? path.resolve(info.location) : '';
    const script = path.join(dir, 'zap-baseline.py');
    if (!fs.existsSync(script)) throw new Error(`ZAP baseline script not found at ${script}. Open ZAP once so its scripts are installed, or set ZAP_HOME.`);
    if (!commandExists('python')) throw new Error('Python is required to run the ZAP baseline script.');
    command = 'python';
    args = [script, '-t', safeUrl, '-m', '2', '-I'];
    display = `python zap-baseline.py -t ${safeUrl} -m 2 -I`;
  } else if (tool === 'playwright') {
    throw new Error('Playwright sessions are started from Authorized System Access, not the tool runner.');
  } else {
    throw new Error(`Tool ${tool} cannot be executed directly.`);
  }

  const startedAt = isoNow();
  audit.record('agent', 'tool.start', safeUrl, 2, `${info.name}: ${display}`);
  const result = await spawnCollect(command, args, opts.timeoutMs ?? 180_000, signal);
  const output = redact(result.output).slice(0, MAX_OUTPUT);
  const finishedAt = isoNow();
  db.run('INSERT INTO tool_runs (id, tool, target, args, status, exit_code, output, started_at, finished_at) VALUES (?,?,?,?,?,?,?,?,?)', [
    randomUUID(),
    tool,
    safeUrl,
    display,
    result.status,
    result.exitCode,
    output,
    startedAt,
    finishedAt,
  ]);
  audit.record('agent', 'tool.finish', safeUrl, 2, `${info.name} ${result.status} exit=${result.exitCode}`);
  return { tool, command: display, status: result.status, exitCode: result.exitCode, output, startedAt, finishedAt };
}

function assertHost(host: string): string {
  if (!/^[A-Za-z0-9.\-]{1,253}$/.test(host) || host.startsWith('-')) throw new Error('Invalid hostname for tool execution.');
  return host;
}

function assertHttpUrl(url: string): string {
  const u = new URL(url);
  if (!['http:', 'https:'].includes(u.protocol)) throw new Error('Only HTTP(S) targets can be tested.');
  if (!/^[A-Za-z0-9.\-:/\[\]?=&%_~+]+$/.test(url)) throw new Error('Target URL contains characters that are not allowed for tool execution.');
  return url;
}

function scopeAllows(target: Target, host: string): boolean {
  const allowed = [new URL(target.url).hostname, ...JSON.parse(target.scope_patterns || '[]')].map((h: string) => h.toLowerCase());
  return allowed.some((p: string) => p === host.toLowerCase() || (p.startsWith('*.') && host.toLowerCase().endsWith(p.slice(1))));
}

function spawnCollect(cmd: string, args: string[], timeoutMs: number, signal?: AbortSignal): Promise<{ status: RunToolResult['status']; exitCode: number | null; output: string }> {
  return new Promise((resolve) => {
    let out = '';
    let settled = false;
    const child = spawn(cmd, args, { windowsHide: true, shell: false });
    const finish = (status: RunToolResult['status'], code: number | null) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve({ status, exitCode: code, output: out });
    };
    const timer = setTimeout(() => {
      out += `\n[JAKE] Timed out after ${timeoutMs} ms; process terminated.`;
      try {
        child.kill();
      } catch {
        /* ignore */
      }
      finish('timeout', null);
    }, timeoutMs);
    const onAbort = () => {
      out += '\n[JAKE] Cancelled by emergency stop.';
      try {
        child.kill();
      } catch {
        /* ignore */
      }
      finish('failed', null);
    };
    signal?.addEventListener('abort', onAbort, { once: true });
    child.stdout?.on('data', (d: Buffer) => {
      if (out.length < MAX_OUTPUT) out += d.toString('utf8');
    });
    child.stderr?.on('data', (d: Buffer) => {
      if (out.length < MAX_OUTPUT) out += d.toString('utf8');
    });
    child.on('error', (err) => {
      out += `\n[JAKE] ${err.message}`;
      finish('failed', null);
    });
    child.on('close', (code) => finish(code === 0 ? 'completed' : 'failed', code));
  });
}
