/**
 * End-to-end smoke test against the real Electron app through its preload/IPC bridge.
 * Starts an isolated local target, runs reconnaissance + a safe assessment, verifies
 * findings and generates HTML/JSON/PDF reports. Uses a temporary data directory.
 *
 *   npm run build && node scripts/e2e-smoke.mjs
 */
import http from 'node:http';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { chromium } from 'playwright-core';

const root = path.resolve(import.meta.dirname, '..');
const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'jake-e2e-'));
const cdpPort = 9223;

const json = (res, status, body) => {
  res.writeHead(status, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify(body));
};

const USERS = [
  { id: 1, username: 'admin', role: 'Administrator', active: true, last_login: null, mfa_enabled: false, password_hash: '$2y$10$notarealhashxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx' },
  { id: 2, username: 'viewer', role: 'Viewer', active: true, last_login: '2026-11-15T10:00:00Z', mfa_enabled: false },
  { id: 3, username: 'viewer', role: 'Viewer', active: true, last_login: '2025-01-01T10:00:00Z', mfa_enabled: false },
  { id: 4, username: 'test', role: 'Viewer', active: false, last_login: '2026-11-10T10:00:00Z', mfa_enabled: false },
  { id: 5, username: 'manager', role: 'Manager', active: true, last_login: '2025-02-01T10:00:00Z', mfa_enabled: false },
];

const server = http.createServer((req, res) => {
  if (req.url === '/') {
    res.writeHead(200, { 'Content-Type': 'text/html', Server: 'Apache/2.4.1' });
    res.end('<html><head><title>Isolated test target</title></head><body><h1>Test</h1><form method="post" action="/save"><input name="name"><button>Save</button></form></body></html>');
    return;
  }
  if (req.url === '/openapi.json') return json(res, 200, { openapi: '3.0.0', paths: { '/api/health': {} } });
  if (req.url === '/api/health') return json(res, 200, { status: 'ok' });
  if (req.url && req.url.startsWith('/api/')) {
    const auth = req.headers.authorization ?? '';
    if (!auth.startsWith('Bearer ')) return json(res, 401, { error: 'authentication required' });
    if (req.url === '/api/admin/users') {
      if (auth !== 'Bearer admin-token') return json(res, 403, { error: 'forbidden' });
      return json(res, 200, { data: { users: USERS } });
    }
    if (req.url === '/api/admin/security-policy') {
      return json(res, 200, { min_length: 6, max_attempts: 0, session_timeout: 240, mfa_required: false });
    }
    if (req.url === '/api/admin/login-events') {
      return json(res, 200, Array.from({ length: 6 }, () => ({ username: 'admin', success: false, created_at: new Date().toISOString() })));
    }
    if (req.url === '/api/admin/reports') {
      // Intentionally accepts the low-privilege token: RBAC must detect this.
      return json(res, 200, [{ report: 'Q4', revenue: 1000 }]);
    }
  }
  res.writeHead(404, { 'Content-Type': 'text/plain' });
  res.end('not found');
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const port = server.address().port;

const packaged = process.env.JAKE_E2E_BIN;
const electronBin = packaged ?? (process.platform === 'win32' ? path.join(root, 'node_modules', 'electron', 'dist', 'electron.exe') : path.join(root, 'node_modules', 'electron', 'dist', 'electron'));
const args = packaged ? [`--remote-debugging-port=${cdpPort}`] : ['.', `--remote-debugging-port=${cdpPort}`];
const child = spawn(electronBin, args, {
  cwd: root,
  env: { ...process.env, JAKE_DATA_DIR: dataDir },
  stdio: ['ignore', 'pipe', 'pipe'],
});
let logs = '';
child.stdout.on('data', (d) => (logs += d.toString()));
child.stderr.on('data', (d) => (logs += d.toString()));

async function connect() {
  for (let i = 0; i < 40; i++) {
    try {
      const browser = await chromium.connectOverCDP(`http://127.0.0.1:${cdpPort}`);
      const pages = browser.contexts().flatMap((c) => c.pages());
      if (pages.length) return { browser, page: pages[0] };
      await browser.close();
    } catch {
      /* not ready yet */
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`Could not connect to Electron CDP.\n${logs}`);
}

let browser;
let page;
let failed = false;
try {
  ({ browser, page } = await connect());
  await page.waitForTimeout(1200);

  // Render check for the rebuilt provider settings screen.
  await page.getByText('Settings', { exact: true }).first().click();
  await page.getByText('AI provider', { exact: true }).waitFor({ timeout: 8000 });
  const providerOptions = await page.locator('select').first().locator('option').allTextContents();
  if (!providerOptions.some((o) => o.includes('Ollama'))) throw new Error('Ollama is missing from the provider selector.');
  if (!providerOptions.some((o) => o.includes('Qwen'))) throw new Error('Qwen is missing from the provider selector.');
  await page.getByText('Free provider quick start').waitFor({ timeout: 4000 });
  await page.getByText('AI Assistant', { exact: true }).first().click();
  await page.waitForTimeout(300);

  const invoke = (channel, payload = {}) => page.evaluate(([c, p]) => window.jake.invoke(c, p), [channel, payload]);
  const unwrap = (res, label) => {
    if (!res?.ok) throw new Error(`${label}: ${res?.error}`);
    return res.data;
  };

  unwrap(await invoke('settings:save', { autoApproveLevel: 3, defaultRateLimit: 20 }), 'settings');

  const initial = unwrap(await invoke('settings:get'), 'initial settings');
  if (initial.aiProvider !== 'ollama') throw new Error(`Expected Ollama as the default provider, got ${initial.aiProvider}.`);
  if (initial.requiresKey) throw new Error('Ollama must not require an API key.');
  if (initial.keyConfigured) throw new Error('A fresh profile should not have an API key configured.');

  // Free local AI: if Ollama is reachable, run a real streamed completion that
  // must not call tools and must not produce an error.
  let aiLive = { skipped: 'Ollama is not running on this machine' };
  let localModels = null;
  try {
    localModels = unwrap(await invoke('ai:models'), 'ai:models');
  } catch {
    localModels = null;
  }
  if (localModels) {
    if (localModels.length === 0) throw new Error('Ollama is running but has no models installed; pull one before running e2e.');
    const model = localModels.includes('qwen2.5:7b') ? 'qwen2.5:7b' : localModels[0];
    // A missing configured model must self-repair to an installed one.
    unwrap(
      await invoke('settings:save', { aiProvider: 'ollama', aiBaseUrl: 'http://127.0.0.1:11434/v1', aiModel: 'e2e-model-not-installed:1b' }),
      'missing local model settings',
    );
    const repairedTest = unwrap(await invoke('settings:test-ai'), 'local auto-select test');
    if (!repairedTest.ok) throw new Error(`Local model auto-select failed: ${repairedTest.message}`);
    if (!repairedTest.message.includes('Selected installed model')) throw new Error('Connection test did not report the automatic model selection.');
    const repairedSettings = unwrap(await invoke('settings:get'), 'settings after repair');
    if (!localModels.includes(repairedSettings.aiModel)) throw new Error(`Auto-selected model ${repairedSettings.aiModel} is not installed.`);
    unwrap(await invoke('settings:save', { aiProvider: 'ollama', aiBaseUrl: 'http://127.0.0.1:11434/v1', aiModel: model }), 'local AI settings');
    const convo = unwrap(await invoke('chat:new'), 'chat:new');
    const reply = unwrap(await invoke('chat:send', { conversationId: convo.id, text: 'Reply with exactly READY. Do not call any tools.' }), 'chat:send');
    if (reply.error) throw new Error(`Local AI error: ${reply.error}`);
    if (!reply.content || !reply.content.trim()) throw new Error('Local AI returned an empty response.');
    aiLive = { provider: 'ollama', model, reply: reply.content.trim().slice(0, 160) };
  }

  // Provider-scoped OS-encrypted secret round-trip using a cloud provider (dummy key).
  unwrap(
    await invoke('settings:save', { aiProvider: 'openrouter', aiBaseUrl: 'https://openrouter.ai/api/v1', aiModel: 'qwen/qwen-2.5-72b-instruct:free' }),
    'cloud provider settings',
  );
  const cloud = unwrap(await invoke('settings:get'), 'cloud settings');
  if (!cloud.requiresKey) throw new Error('OpenRouter should report requiresKey=true.');
  unwrap(await invoke('settings:save', { apiKey: 'sk-or-e2e-not-a-real-key' }), 'save key');
  const withKey = unwrap(await invoke('settings:get'), 'get settings');
  if (!withKey.keyConfigured) throw new Error('API key was not stored with OS secure storage.');
  const failedTest = unwrap(await invoke('settings:test-ai'), 'test-ai');
  if (failedTest.ok) throw new Error('A fake cloud API key must not pass the connection test.');
  if (failedTest.provider !== 'openrouter') throw new Error(`Connection test reported the wrong provider: ${failedTest.provider}`);
  if (!failedTest.message) throw new Error('Connection test did not explain the failure.');
  unwrap(await invoke('settings:save', { clearApiKey: true }), 'clear key');
  const withoutKey = unwrap(await invoke('settings:get'), 'get settings');
  if (withoutKey.keyConfigured) throw new Error('API key was not cleared.');
  unwrap(await invoke('settings:save', { aiProvider: 'ollama', aiBaseUrl: 'http://127.0.0.1:11434/v1' }), 'restore ollama');
  const target = unwrap(
    await invoke('targets:create', {
      label: 'E2E isolated target',
      url: `http://127.0.0.1:${port}`,
      kind: 'localhost',
      authorizationNote: 'Isolated local test server started by the automated smoke test.',
      rateLimit: 20,
      allowPrivate: true,
    }),
    'targets:create',
  );

  const recon = unwrap(await invoke('recon:run', { targetId: target.id }), 'recon');
  const vuln = unwrap(await invoke('vuln:run', { targetId: target.id }), 'vuln');

  const findings = unwrap(await invoke('findings:list', {}), 'findings');
  const scopedFindings = findings.filter((f) => f.assessment_id === vuln.assessmentId || f.assessment_id === recon.assessmentId);
  if (scopedFindings.length === 0) throw new Error('Expected at least one finding from the intentionally weak test server.');

  const reports = {};
  for (const format of ['html', 'json', 'pdf']) {
    const res = unwrap(await invoke('reports:generate', { assessmentId: vuln.assessmentId, format }), `report ${format}`);
    if (!fs.existsSync(res.path)) throw new Error(`${format} report missing at ${res.path}`);
    reports[format] = { path: res.path, bytes: fs.statSync(res.path).size };
  }
  if (reports.pdf.bytes < 1000) throw new Error('PDF report is suspiciously small.');

  // --- Authorized admin API: account audit, RBAC boundaries, API assessment ---
  const apiTarget = unwrap(
    await invoke('targets:create', {
      label: 'E2E admin API',
      url: `http://127.0.0.1:${port}`,
      kind: 'api',
      authorizationNote: 'Isolated mock admin API started by the automated smoke test.',
      rateLimit: 20,
      allowPrivate: true,
    }),
    'api target',
  );
  const endpoints = [
    { name: 'Public health', method: 'GET', path: '/api/health', viewerAllowed: true, requiresAuth: false },
    { name: 'Admin users', method: 'GET', path: '/api/admin/users', viewerAllowed: false, requiresAuth: true },
    { name: 'Reports', method: 'GET', path: '/api/admin/reports', viewerAllowed: false, requiresAuth: true },
  ];
  const connection = unwrap(
    await invoke('connections:save', {
      name: 'E2E admin API',
      kind: 'rest',
      targetId: apiTarget.id,
      baseUrl: `http://127.0.0.1:${port}`,
      config: {
        auth: { type: 'bearer', value: 'admin-token' },
        viewerAuth: { type: 'bearer', value: 'viewer-token' },
        usersEndpoint: '/api/admin/users',
        policyPath: '/api/admin/security-policy',
        loginEventsPath: '/api/admin/login-events',
        endpoints,
      },
    }),
    'connection',
  );
  if (!connection.has_secret) throw new Error('Connection credentials were not stored securely.');

  const accountAudit = unwrap(await invoke('accounts:run', { connectionId: connection.id }), 'account audit');
  if (accountAudit.result.total !== 5) throw new Error(`Expected 5 accounts, got ${accountAudit.result.total}`);
  if (accountAudit.result.issues.length < 5) throw new Error('Account audit did not detect the planted account issues.');
  if (!accountAudit.result.issues.some((i) => i.name.includes('Password material'))) throw new Error('Password material exposure was not detected.');

  const rbac = unwrap(await invoke('rbac:run', { connectionId: connection.id }), 'rbac');
  const reportsRow = rbac.result.rows.find((r) => r.name === 'Reports');
  const healthRow = rbac.result.rows.find((r) => r.name === 'Public health');
  if (reportsRow?.verdict !== 'viewer-escalation') throw new Error(`RBAC failed to detect viewer escalation (got ${reportsRow?.verdict}).`);
  if (healthRow?.verdict !== 'ok') throw new Error(`Public endpoint was misreported (got ${healthRow?.verdict}).`);

  const api = unwrap(await invoke('api:run', { connectionId: connection.id }), 'api');
  const apiFindings = unwrap(await invoke('findings:list', {}), 'findings').filter((f) => f.assessment_id === api.assessmentId);
  if (!apiFindings.some((f) => f.name.includes('sensitive fields'))) throw new Error('API sensitive-field exposure was not detected.');
  if (!apiFindings.some((f) => f.name.includes('documentation'))) throw new Error('Exposed API documentation was not detected.');

  const stats = unwrap(await invoke('dashboard:stats'), 'stats');
  const audit = unwrap(await invoke('audit:list', { limit: 50 }), 'audit');
  const tools = unwrap(await invoke('tools:list'), 'tools');

  unwrap(await invoke('targets:delete', { id: target.id }), 'targets:delete');
  unwrap(await invoke('targets:delete', { id: apiTarget.id }), 'targets:delete api');

  console.log(
    JSON.stringify(
      {
        ok: true,
        recon: { assessmentId: recon.assessmentId, httpStatus: recon.data.httpStatus, technologies: recon.data.technologies, endpoints: recon.data.endpoints.length },
        vuln: { assessmentId: vuln.assessmentId, findings: vuln.result.findings, requests: vuln.result.requests },
        storedFindings: scopedFindings.length,
        accounts: { total: accountAudit.result.total, issues: accountAudit.result.issues.length, dormant: accountAudit.result.dormant, withoutMfa: accountAudit.result.withoutMfa },
        rbac: { summary: rbac.result.summary, verdicts: rbac.result.rows.map((r) => `${r.name}:${r.verdict}`) },
        api: { summary: api.result.summary, findings: apiFindings.map((f) => f.name) },
        reports,
        stats: { assessments: stats.assessments, findings: stats.findings },
        auditEntries: audit.length,
        toolsDetected: tools.filter((t) => t.installed).map((t) => t.id),
        aiLive,
        aiAutoSelected: aiLive.model ?? null,
      },
      null,
      2,
    ),
  );
} catch (err) {
  failed = true;
  console.error(`E2E FAILED: ${err.message}`);
  console.error(logs.slice(-3000));
} finally {
  await browser?.close().catch(() => undefined);
  child.kill();
  await new Promise((r) => server.close(r));
  fs.rmSync(dataDir, { recursive: true, force: true });
}
process.exit(failed ? 1 : 0);
