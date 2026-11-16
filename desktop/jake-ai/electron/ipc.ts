import { ipcMain, shell, app, type BrowserWindow } from 'electron';
import path from 'node:path';
import fs from 'node:fs';
import mysql from 'mysql2/promise';
import type { AppContext } from './services/context';
import type { ChatAgent } from './services/agent';
import { CHANNELS, type Channel } from '../shared/channels';
import type { ConnectionConfig } from './services/connections';
import type { Severity } from '../shared/types';
import { restRequest } from './services/rest';
import { redact } from './services/redact';

type Handler = (payload: Record<string, unknown>) => Promise<unknown> | unknown;

export function registerIpc(ctx: AppContext, agent: ChatAgent, getWindow: () => BrowserWindow | null, isAllowedSender: (url: string) => boolean): void {
  let chatController: AbortController | null = null;

  const handlers: Record<Channel, Handler> = {
    health: () => ({ ok: true, version: app.getVersion(), dataDir: app.getPath('userData') }),
    'settings:get': () => ctx.store.getSettings(),
    'settings:save': (p) =>
      ctx.store.saveSettings({
        aiProvider: p.aiProvider as string | undefined,
        aiModel: p.aiModel as string | undefined,
        temperature: p.temperature as number | undefined,
        maxTokens: p.maxTokens as number | undefined,
        aiBaseUrl: p.aiBaseUrl as string | undefined,
        defaultRateLimit: p.defaultRateLimit as number | undefined,
        requestTimeoutMs: p.requestTimeoutMs as number | undefined,
        autoApproveLevel: p.autoApproveLevel as number | undefined,
        apiKey: typeof p.apiKey === 'string' ? p.apiKey : undefined,
        clearApiKey: Boolean(p.clearApiKey),
      }),
    'settings:test-ai': async () => {
      try {
        return await ctx.ai.testConnection();
      } catch (err) {
        const s = ctx.store.getSettings();
        return { ok: false, message: (err as Error).message, latencyMs: 0, model: s.aiModel, provider: s.aiProvider };
      }
    },
    'ai:models': () => ctx.ai.listModels(),
    'dashboard:stats': () => ctx.dashboard(),
    'targets:list': () => ctx.targets(),
    'targets:create': (p) =>
      ctx.createTarget({
        label: String(p.label ?? ''),
        url: String(p.url ?? ''),
        kind: (p.kind as 'web' | 'api' | 'php' | 'mysql' | 'localhost') ?? 'web',
        authorizationNote: String(p.authorizationNote ?? ''),
        scopePatterns: Array.isArray(p.scopePatterns) ? (p.scopePatterns as string[]) : [],
        rateLimit: Number(p.rateLimit ?? 2),
        allowPrivate: Boolean(p.allowPrivate),
      }),
    'targets:delete': (p) => {
      ctx.deleteTarget(String(p.id));
      return { ok: true };
    },
    'assessments:list': () => ctx.assessments.list(),
    'assessments:get': (p) => {
      const id = String(p.id);
      const assessment = ctx.assessments.get(id);
      if (!assessment) throw new Error('Assessment not found.');
      const recon = ctx.db.all('SELECT data, created_at FROM recon_results WHERE assessment_id = ?', [id]).map((r) => ({
        ...JSON.parse(String((r as { data: string }).data)),
        collectedAt: (r as { created_at: string }).created_at,
      }));
      return {
        assessment,
        findings: ctx.assessments.findingsFor(id),
        recon,
        accountAudits: ctx.db.all('SELECT * FROM account_audits WHERE assessment_id = ?', [id]),
        rbac: ctx.db.all('SELECT * FROM rbac_results WHERE assessment_id = ?', [id]),
        api: ctx.db.all('SELECT * FROM api_results WHERE assessment_id = ?', [id]),
        reports: ctx.db.all('SELECT * FROM reports WHERE assessment_id = ?', [id]),
        approvals: ctx.db.all('SELECT * FROM approvals WHERE assessment_id = ?', [id]),
      };
    },
    'recon:run': (p) => ctx.runRecon(String(p.targetId)),
    'vuln:run': (p) => ctx.runVulnerability(String(p.targetId)),
    'accounts:run': (p) => ctx.runAccounts(String(p.connectionId)),
    'rbac:run': (p) => ctx.runRbac(String(p.connectionId)),
    'api:run': (p) => ctx.runApi(String(p.connectionId)),
    'php:scan': (p) => ctx.runPhp(String(p.directory)),
    'php:file': (p) => ctx.readProjectFile(String(p.file)),
    'php:apply': (p) => ctx.applyProjectPatch({ file: String(p.file), oldText: String(p.oldText ?? ''), newText: String(p.newText ?? ''), assessmentId: p.assessmentId ? String(p.assessmentId) : undefined }),
    'mysql:audit': (p) => ctx.runMysql(String(p.connectionId)),
    'connections:list': () => ctx.connections.list(),
    'connections:get': (p) => {
      const c = ctx.connections.get(String(p.id));
      return { ...c, config: ctx.connections.publicConfig(c.config), authValueConfigured: ctx.connections.hasCredentials(c.id) };
    },
    'connections:save': (p) => {
      const input = {
        id: p.id ? String(p.id) : undefined,
        name: String(p.name ?? ''),
        kind: (p.kind as 'rest' | 'mysql' | 'database') ?? 'rest',
        targetId: String(p.targetId ?? ''),
        baseUrl: p.baseUrl ? String(p.baseUrl) : undefined,
        username: p.username ? String(p.username) : undefined,
        config: (p.config ?? {}) as ConnectionConfig,
      };
      if (!input.name.trim()) throw new Error('Give the connection profile a name.');
      if (!input.targetId) throw new Error('Select the authorized target this profile belongs to.');
      if (input.id) {
        try {
          const prev = ctx.connections.get(input.id).config;
          if (input.config.auth && !input.config.auth.value && !input.config.auth.username) input.config.auth = prev.auth ?? input.config.auth;
          if (input.config.viewerAuth && !input.config.viewerAuth.value && !input.config.viewerAuth.username) input.config.viewerAuth = prev.viewerAuth ?? input.config.viewerAuth;
          if (input.config.mysql && !input.config.mysql.password) input.config.mysql.password = prev.mysql?.password;
        } catch {
          /* profile could not be decrypted; accept new values */
        }
      }
      const saved = ctx.connections.save(input);
      ctx.audit.record('user', 'connection.save', saved.name, 3, `Connection profile saved (${saved.kind}); credentials stored encrypted`);
      // Never return decrypted credentials to the renderer.
      return { ...saved, config: ctx.connections.publicConfig(saved.config), has_secret: ctx.connections.hasCredentials(saved.id) ? 1 : 0 };
    },
    'connections:delete': (p) => {
      ctx.connections.remove(String(p.id));
      ctx.audit.record('user', 'connection.delete', String(p.id), 3, 'Connection profile removed');
      return { ok: true };
    },
    'connections:test': async (p) => {
      const profile = ctx.connections.get(String(p.id));
      if (profile.kind === 'mysql') {
        const cfg = profile.config.mysql;
        if (!cfg) throw new Error('No MySQL settings in this profile.');
        const conn = await mysql.createConnection({ host: cfg.host, port: cfg.port, user: cfg.user, password: cfg.password ?? '', database: cfg.database, connectTimeout: 8000 });
        try {
          const [rows] = await conn.query<mysql.RowDataPacket[]>('SELECT VERSION() AS version, CURRENT_USER() AS user');
          return { ok: true, message: `Connected to MySQL ${rows[0].version} as ${rows[0].user}`, latencyMs: 0 };
        } finally {
          await conn.end().catch(() => undefined);
        }
      }
      const { restContext } = await import('./services/rest');
      const rc = restContext(profile);
      const started = Date.now();
      const res = await restRequest(rc, profile.config.usersEndpoint || '/', { auth: 'admin' });
      return {
        ok: res.status < 400,
        message: `HTTP ${res.status} from ${res.url} (${res.bodyBytes} bytes) in ${Date.now() - started} ms`,
        preview: redact(res.body.slice(0, 500)),
      };
    },
    'findings:list': (p) =>
      ctx.findings({
        severity: p.severity ? (String(p.severity) as Severity) : undefined,
        status: p.status ? String(p.status) : undefined,
        search: p.search ? String(p.search) : undefined,
      }),
    'findings:update': (p) => {
      ctx.updateFinding(String(p.id), String(p.status));
      return { ok: true };
    },
    'reports:list': () => ctx.reports.list(),
    'reports:generate': (p) => ctx.generateReport(String(p.assessmentId), (String(p.format) as 'html' | 'json' | 'pdf') ?? 'html'),
    'reports:open': async (p) => {
      const file = path.resolve(String(p.path));
      const dir = path.resolve(ctx.store.reportsDir());
      if (!file.startsWith(dir + path.sep) || !fs.existsSync(file)) throw new Error('Report file is not inside the JAKE reports directory.');
      const result = await shell.openPath(file);
      if (result) throw new Error(result);
      return { ok: true };
    },
    'tools:list': () => ctx.toolList(),
    'tools:run': (p) => ctx.runTool(String(p.tool) as 'nmap' | 'nikto' | 'zap', String(p.targetId), { topPorts: Number(p.topPorts ?? 100) }),
    'tools:history': () => ctx.db.all('SELECT * FROM tool_runs ORDER BY started_at DESC LIMIT 100'),
    'wsl:status': () => ctx.wslStatus(),
    'access:list': () => ctx.db.all('SELECT id, url, title, summary, html_path AS htmlPath, captured_at FROM access_captures ORDER BY captured_at DESC LIMIT 50'),
    'access:open': (p) => ctx.openAccess(String(p.targetId), String(p.url)),
    'access:capture': () => ctx.captureAccessPage(),
    'access:close': () => ctx.closeAccess(),
    'audit:list': (p) => ctx.audit.list(Number(p.limit ?? 500)),
    'approval:respond': (p) => ({ handled: ctx.approvals.respond(String(p.id), Boolean(p.approved)) }),
    'emergency:stop': () => {
      chatController?.abort();
      ctx.stopAll();
      return { ok: true };
    },
    'chat:new': () => {
      const id = agent.ensureConversation();
      return { id };
    },
    'chat:list': () => agent.conversations(),
    'chat:history': (p) => agent.history(String(p.conversationId)),
    'chat:delete': (p) => {
      agent.deleteConversation(String(p.conversationId));
      return { ok: true };
    },
    'chat:send': async (p) => {
      if (chatController) throw new Error('JAKE is already answering. Stop the current response first.');
      const conversationId = agent.ensureConversation(p.conversationId ? String(p.conversationId) : undefined);
      const text = String(p.text ?? '').trim();
      if (!text) throw new Error('Message is empty.');
      if (text.length > 8000) throw new Error('Message is too long (limit 8000 characters).');
      const controller = new AbortController();
      chatController = controller;
      try {
        const result = await agent.run({
          conversationId,
          text,
          signal: controller.signal,
          emit: (e) => {
            const win = getWindow();
            if (win && !win.isDestroyed()) win.webContents.send('jake:event', e);
          },
        });
        return { conversationId, ...result };
      } finally {
        chatController = null;
      }
    },
    'chat:stop': () => {
      chatController?.abort();
      chatController = null;
      return { ok: true };
    },
  };

  ipcMain.handle('jake:invoke', async (event, channel: unknown, payload: unknown) => {
    try {
      if (typeof channel !== 'string' || !(CHANNELS as readonly string[]).includes(channel)) throw new Error('Unknown IPC channel.');
      if (!isAllowedSender(event.senderFrame?.url ?? '')) throw new Error('Blocked IPC call from an untrusted frame.');
      const handler = handlers[channel as Channel];
      const data = await handler((payload && typeof payload === 'object' ? payload : {}) as Record<string, unknown>);
      return { ok: true, data };
    } catch (err) {
      const message = (err as Error).message ?? 'Unknown error';
      ctx.audit.record('system', 'ipc.error', typeof channel === 'string' ? channel : null, 1, message);
      return { ok: false, error: message };
    }
  });
}
