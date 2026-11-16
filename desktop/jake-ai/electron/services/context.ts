import { EventEmitter } from 'node:events';
import path from 'node:path';
import fs from 'node:fs';
import { randomUUID } from 'node:crypto';
import { Database, isoNow } from './db';
import { Store } from './store';
import { AuditLog, ApprovalManager } from './audit';
import { Assessments } from './assessments';
import { Connections } from './connections';
import { AiClient } from './ai';
import { Reports } from './reports';
import { SystemAccess } from './access';
import { runRecon, type ReconData } from './recon';
import { runVulnerabilityScan } from './vuln';
import { runAccountAudit } from './accounts';
import { runRbacAudit } from './rbac';
import { runApiScan } from './apiScan';
import { runPhpAudit } from './php';
import { runMysqlAudit } from './mysql';
import { listTools, runSecurityTool, detectWsl, type ToolId } from './tools';
import { parseTarget, assertUrlInScope, hostMatches, targetHosts } from './scope';
import type { AssessmentType, Severity, Target } from '../../shared/types';

export class ApprovalDenied extends Error {
  constructor(message = 'Operation denied by the operator.') {
    super(message);
    this.name = 'ApprovalDenied';
  }
}

/**
 * Application service container. Every privileged operation is orchestrated
 * here so it is permission-gated and audited in one place.
 */
export class AppContext {
  readonly emitter = new EventEmitter();
  readonly db: Database;
  readonly store: Store;
  readonly audit: AuditLog;
  readonly approvals: ApprovalManager;
  readonly assessments: Assessments;
  readonly connections: Connections;
  readonly ai: AiClient;
  readonly reports: Reports;
  readonly access: SystemAccess;
  private controllers = new Set<AbortController>();

  constructor(db: Database, baseDir: string) {
    this.db = db;
    this.store = new Store(db, baseDir);
    this.audit = new AuditLog(db);
    this.approvals = new ApprovalManager(db, this.audit);
    this.assessments = new Assessments(db);
    this.connections = new Connections(db, this.store);
    this.ai = new AiClient(this.store);
    this.reports = new Reports(db, this.assessments, this.store, this.audit);
    this.access = new SystemAccess(db, path.join(baseDir, 'browser-profile'), (s) => this.emit({ type: 'access', message: s }));
    this.approvals.on('request', (req) => this.emit({ type: 'approval', payload: req }));
    this.audit.record('system', 'app.start', null, 1, `JAKE AI started; data directory ${baseDir}`);
  }

  emit(event: Record<string, unknown>): void {
    this.emitter.emit('event', event);
  }

  stopAll(): void {
    for (const c of this.controllers) c.abort();
    this.controllers.clear();
    this.approvals.cancelAll();
    void this.access.close();
    this.audit.record('user', 'emergency.stop', null, 5, 'Emergency stop: all running operations cancelled.');
    this.emit({ type: 'stopped', message: 'Emergency stop engaged. All operations cancelled.' });
  }

  private controller(): AbortController {
    const c = new AbortController();
    this.controllers.add(c);
    return c;
  }

  private release(c: AbortController): void {
    this.controllers.delete(c);
  }

  async approval(opts: { level: number; title: string; detail: string; assessmentId: string | null }): Promise<boolean> {
    return this.approvals.request({ ...opts, autoApproveAt: this.store.getSettings().autoApproveLevel, timeoutMs: 180_000 });
  }

  targets(): Target[] {
    return this.db.all<Record<string, unknown>>('SELECT * FROM targets ORDER BY created_at DESC').map(parseTarget);
  }

  target(id: string): Target {
    const row = this.db.get<Record<string, unknown>>('SELECT * FROM targets WHERE id = ?', [id]);
    if (!row) throw new Error('Target not found. Register it in Target Management first.');
    return parseTarget(row);
  }

  createTarget(input: { label: string; url: string; kind: Target['kind']; authorizationNote: string; scopePatterns?: string[]; rateLimit?: number; allowPrivate?: boolean }): Target {
    const url = new URL(input.url);
    if (!['http:', 'https:'].includes(url.protocol)) throw new Error('Target URL must start with http:// or https://');
    if (!input.label.trim()) throw new Error('Give the target a name.');
    if (!input.authorizationNote || input.authorizationNote.trim().length < 10) {
      throw new Error('Record the authorization basis (at least 10 characters): who owns the system and why you may test it.');
    }
    const patterns = (input.scopePatterns ?? []).map((p) => p.trim().toLowerCase()).filter(Boolean);
    for (const p of patterns) {
      if (!/^(\*\.)?[a-z0-9]([a-z0-9.-]*[a-z0-9])?$/.test(p)) throw new Error(`Invalid scope pattern: ${p}`);
    }
    const id = randomUUID();
    this.db.run(
      'INSERT INTO targets (id, label, url, kind, authorization_note, scope_patterns, rate_limit, allow_private, created_at) VALUES (?,?,?,?,?,?,?,?,?)',
      [id, input.label.trim().slice(0, 120), url.toString(), input.kind, input.authorizationNote.trim().slice(0, 2000), JSON.stringify(patterns), Math.min(20, Math.max(0.2, input.rateLimit ?? 2)), input.allowPrivate ? 1 : 0, isoNow()],
    );
    this.audit.record('user', 'target.create', url.toString(), 1, `Authorized target registered: ${input.label}`);
    return this.target(id);
  }

  deleteTarget(id: string): void {
    const t = this.target(id);
    this.db.run('DELETE FROM targets WHERE id = ?', [id]);
    this.audit.record('user', 'target.delete', t.url, 1, `Target removed: ${t.label}`);
  }

  async runRecon(targetId: string): Promise<{ assessmentId: string; data: ReconData }> {
    const target = this.target(targetId);
    const assessment = this.assessments.create('recon', target.label, target.id);
    const c = this.controller();
    try {
      this.emit({ type: 'progress', assessmentId: assessment.id, progress: 2, message: 'Starting passive reconnaissance' });
      const data = await runRecon({
        db: this.db,
        assessments: this.assessments,
        target,
        assessmentId: assessment.id,
        signal: c.signal,
        onProgress: (p, msg) => {
          this.assessments.progress(assessment.id, p);
          this.emit({ type: 'progress', assessmentId: assessment.id, progress: p, message: msg });
        },
      });
      const summary = `${data.technologies.length} technologies, ${data.endpoints.length} endpoints, ${data.subdomains.length} public subdomains, HTTP ${data.httpStatus ?? 'n/a'}${data.error ? `; error: ${data.error}` : ''}`;
      this.assessments.finish(assessment.id, data.error && data.httpStatus === null ? 'failed' : 'complete', summary, data.error ?? undefined);
      this.audit.record('agent', 'recon.complete', target.url, 1, summary);
      return { assessmentId: assessment.id, data };
    } catch (err) {
      this.assessments.finish(assessment.id, c.signal.aborted ? 'stopped' : 'failed', undefined, (err as Error).message);
      throw err;
    } finally {
      this.release(c);
    }
  }

  private async guarded<T>(
    opts: { type: AssessmentType; title: string; detail: string; level: number; targetLabel: string; targetId: string | null },
    fn: (ctx: { assessmentId: string; signal: AbortSignal; onProgress: (p: number, m: string) => void }) => Promise<{ summary: string; result: T }>,
  ): Promise<{ assessmentId: string; result: T }> {
    const assessment = this.assessments.create(opts.type, opts.targetLabel, opts.targetId);
    const approved = await this.approval({ level: opts.level, title: opts.title, detail: opts.detail, assessmentId: assessment.id });
    if (!approved) {
      this.assessments.finish(assessment.id, 'failed', 'Not executed: operator denied approval.', 'Approval denied');
      throw new ApprovalDenied();
    }
    const c = this.controller();
    try {
      const onProgress = (p: number, m: string) => {
        this.assessments.progress(assessment.id, p);
        this.emit({ type: 'progress', assessmentId: assessment.id, progress: p, message: m });
      };
      onProgress(2, 'Starting');
      const { summary, result } = await fn({ assessmentId: assessment.id, signal: c.signal, onProgress });
      this.assessments.finish(assessment.id, 'complete', summary);
      this.audit.record('agent', `${opts.type}.complete`, opts.targetLabel, opts.level, summary);
      return { assessmentId: assessment.id, result };
    } catch (err) {
      const aborted = c.signal.aborted;
      this.assessments.finish(assessment.id, aborted ? 'stopped' : 'failed', aborted ? 'Stopped by emergency stop.' : undefined, (err as Error).message);
      this.audit.record('agent', `${opts.type}.${aborted ? 'stopped' : 'failed'}`, opts.targetLabel, opts.level, (err as Error).message);
      throw err;
    } finally {
      this.release(c);
    }
  }

  async runVulnerability(targetId: string, opts: { level?: number } = {}) {
    const target = this.target(targetId);
    return this.guarded<Awaited<ReturnType<typeof runVulnerabilityScan>>>(
      {
        type: 'vulnerability',
        title: 'Active web vulnerability assessment',
        detail: `JAKE will send safe, non-destructive HTTP requests to ${target.url} (headers, sensitive paths, admin interfaces, CORS, forms, inert input probes). No exploits or state-changing requests. Rate limit: ${target.rate_limit}/s.`,
        level: opts.level ?? 2,
        targetLabel: target.label,
        targetId: target.id,
      },
      async ({ assessmentId, signal, onProgress }) => {
        const result = await runVulnerabilityScan({
          db: this.db,
          assessments: this.assessments,
          target,
          assessmentId,
          signal,
          onProgress,
        });
        return { summary: `${result.findings} findings across ${result.requests} requests. ${result.notes.slice(0, 4).join(' ')}`, result };
      },
    );
  }

  async runAccounts(connectionId: string, level = 3) {
    const profile = this.connections.get(connectionId);
    return this.guarded({
      type: 'accounts',
      title: 'Account security audit',
      detail: `Read-only review of account records through "${profile.name}" (${profile.base_url ?? 'configured target'}). Password material is never stored or sent to the AI.`,
      level,
      targetLabel: profile.name,
      targetId: profile.target?.id ?? null,
    }, async ({ assessmentId, signal, onProgress }) => {
      const result = await runAccountAudit({
        db: this.db,
        assessments: this.assessments,
        connections: this.connections,
        connectionId,
        assessmentId,
        signal,
        onProgress,
      });
      return { summary: `${result.total} accounts, ${result.issues.length} issue groups, ${result.dormant} dormant, ${result.admins} admin.`, result };
    });
  }

  async runRbac(connectionId: string, level = 3) {
    const profile = this.connections.get(connectionId);
    return this.guarded({
      type: 'rbac',
      title: 'RBAC access-control test',
      detail: `Compare unauthenticated, Viewer and Admin responses on configured safe (GET/HEAD/OPTIONS) routes for "${profile.name}". No write requests are sent.`,
      level,
      targetLabel: profile.name,
      targetId: profile.target?.id ?? null,
    }, async ({ assessmentId, signal, onProgress }) => {
      const result = await runRbacAudit({
        db: this.db,
        assessments: this.assessments,
        connections: this.connections,
        connectionId,
        assessmentId,
        signal,
        onProgress,
      });
      return { summary: result.summary, result };
    });
  }

  async runApi(connectionId: string, level = 3) {
    const profile = this.connections.get(connectionId);
    return this.guarded({
      type: 'api',
      title: 'API security assessment',
      detail: `Send authenticated and unauthenticated requests to configured API routes for "${profile.name}", including a 15-request rate-limit sample. Response values are redacted.`,
      level,
      targetLabel: profile.name,
      targetId: profile.target?.id ?? null,
    }, async ({ assessmentId, signal, onProgress }) => {
      const result = await runApiScan({
        db: this.db,
        assessments: this.assessments,
        connections: this.connections,
        connectionId,
        assessmentId,
        signal,
        onProgress,
      });
      return { summary: result.summary, result };
    });
  }

  async runPhp(directory: string, level = 2) {
    const resolved = path.resolve(directory);
    if (!fs.existsSync(resolved) || !fs.statSync(resolved).isDirectory()) throw new Error(`Directory not found: ${resolved}`);
    this.store.setSetting('php.lastRoot', resolved);
    return this.guarded({
      type: 'php',
      title: 'PHP source-code security review',
      detail: `Read-only static analysis of PHP files under ${resolved}. No files are modified.`,
      level,
      targetLabel: resolved,
      targetId: null,
    }, async ({ assessmentId, onProgress }) => {
      const result = await runPhpAudit({ db: this.db, assessments: this.assessments, directory: resolved, assessmentId, targetLabel: resolved, onProgress });
      return { summary: `${result.filesScanned} PHP files scanned, ${result.issues.length} issues.`, result };
    });
  }

  async runMysql(connectionId: string, level = 3) {
    const profile = this.connections.get(connectionId);
    return this.guarded({
      type: 'mysql',
      title: 'MySQL / MariaDB security audit',
      detail: `Read-only session against ${profile.name}. Inspects privileges, schema metadata and account hashing configuration. No rows are exported and no writes are performed.`,
      level,
      targetLabel: profile.name,
      targetId: profile.target?.id ?? null,
    }, async ({ assessmentId, signal, onProgress }) => {
      const result = await runMysqlAudit({
        db: this.db,
        assessments: this.assessments,
        connections: this.connections,
        connectionId,
        assessmentId,
        signal,
        onProgress,
      });
      return { summary: `${result.tables} tables, ${result.issues.length} issues on ${result.server}.`, result };
    });
  }

  async runTool(tool: ToolId, targetId: string, options: { topPorts?: number } = {}) {
    const target = this.target(targetId);
    const host = new URL(target.url).hostname;
    const tools = listTools();
    const info = tools.find((t) => t.id === tool);
    if (!info) throw new Error(`Unknown tool: ${tool}`);
    if (!info.installed) throw new Error(`${info.name} is not installed. ${info.notes}`);
    const assessment = this.assessments.create('recon', target.label, target.id);
    const approved = await this.approval({
      level: 2,
      title: `${info.name} execution`,
      detail: `Run ${info.name} (${info.via}) against ${host} within the registered scope. Output is captured and redacted.`,
      assessmentId: assessment.id,
    });
    if (!approved) {
      this.assessments.finish(assessment.id, 'failed', 'Tool run denied by operator.', 'Approval denied');
      throw new ApprovalDenied();
    }
    const c = this.controller();
    try {
      const result = await runSecurityTool({
        db: this.db,
        audit: this.audit,
        tool,
        target,
        host,
        url: target.url,
        options,
        signal: c.signal,
      });
      this.assessments.finish(assessment.id, result.status === 'completed' ? 'complete' : 'failed', `${info.name} ${result.status} (exit ${result.exitCode ?? 'n/a'})`, result.status === 'completed' ? undefined : result.output.slice(-800));
      return { assessmentId: assessment.id, result };
    } finally {
      this.release(c);
    }
  }

  wslStatus() {
    return detectWsl(true);
  }

  toolList() {
    return listTools();
  }

  generateReport(assessmentId: string, format: 'html' | 'json' | 'pdf') {
    return this.reports.generate(assessmentId, format);
  }

  dashboard() {
    const stats = this.assessments.stats();
    return { ...stats, recentActivity: this.audit.list(10) };
  }

  findings(filters: { severity?: Severity; status?: string; search?: string }) {
    return this.assessments.listFindings(filters);
  }

  updateFinding(id: string, status: string): void {
    const allowed = ['open', 'confirmed', 'fixed', 'accepted', 'false_positive'];
    if (!allowed.includes(status)) throw new Error('Invalid finding status.');
    this.db.run('UPDATE findings SET status = ? WHERE id = ?', [status, id]);
    this.audit.record('user', 'finding.status', id, 1, `Finding marked ${status}`);
  }

  /** Verify that a path lies inside the PHP project the user last approved. */
  safeProjectPath(file: string): string {
    const root = this.store.getSetting('php.lastRoot');
    if (!root) throw new Error('Scan a PHP project first so JAKE knows the approved project directory.');
    const resolved = path.isAbsolute(file) ? path.resolve(file) : path.resolve(root, file);
    const rel = path.relative(root, resolved);
    if (rel.startsWith('..') || path.isAbsolute(rel)) throw new Error('File is outside the approved project directory.');
    if (!/\.(php|inc|phtml|ini|env|htaccess)$/i.test(resolved)) throw new Error('Only PHP/configuration files can be opened here.');
    return resolved;
  }

  readProjectFile(file: string): { path: string; content: string } {
    const resolved = this.safeProjectPath(file);
    const stat = fs.statSync(resolved);
    if (stat.size > 2_000_000) throw new Error('File is too large to open (limit 2 MB).');
    return { path: resolved, content: fs.readFileSync(resolved, 'utf8') };
  }

  async applyProjectPatch(input: { file: string; oldText: string; newText: string; assessmentId?: string }): Promise<{ backup: string; path: string }> {
    const resolved = this.safeProjectPath(input.file);
    if (!input.oldText || input.oldText === input.newText) throw new Error('The patch is empty or unchanged.');
    const original = fs.readFileSync(resolved, 'utf8');
    const occurrences = original.split(input.oldText).length - 1;
    if (occurrences !== 1) throw new Error(`The exact code to replace must appear once; found ${occurrences} occurrence(s). Edit the patch manually.`);
    const approved = await this.approval({
      level: 4,
      title: 'Modify PHP source file',
      detail: `File: ${resolved}\n\nReplace:\n${input.oldText.slice(0, 800)}\n\nWith:\n${input.newText.slice(0, 800)}\n\nA timestamped backup will be created first.`,
      assessmentId: input.assessmentId ?? null,
    });
    if (!approved) throw new ApprovalDenied('Patch not applied: approval denied.');
    const backup = `${resolved}.jake-backup-${new Date().toISOString().replace(/[:.]/g, '-')}`;
    fs.copyFileSync(resolved, backup);
    fs.writeFileSync(resolved, original.replace(input.oldText, input.newText), 'utf8');
    this.audit.record('user', 'php.patch.applied', resolved, 4, `Backup: ${path.basename(backup)}`);
    return { backup, path: resolved };
  }

  async captureAccess(targetId: string) {
    const captures = this.db.all('SELECT id, url, title, summary, html_path AS htmlPath, captured_at FROM access_captures ORDER BY captured_at DESC LIMIT 30');
    return captures;
  }

  async openAccess(targetId: string, url: string) {
    const target = this.target(targetId);
    await assertUrlInScope(target, url);
    const res = await this.access.openLogin(target, url);
    this.audit.record('user', 'access.login.open', url, 3, `Authorized browser session opened for ${target.label}`);
    return res;
  }

  async captureAccessPage() {
    const cap = await this.access.capture();
    this.audit.record('user', 'access.capture', cap.url, 3, 'Authenticated page structure captured');
    return cap;
  }

  async closeAccess() {
    await this.access.close();
    return { ok: true };
  }

  validateExtraHost(target: Target, host: string): boolean {
    return targetHosts(target).some((p) => hostMatches(host, p));
  }
}
