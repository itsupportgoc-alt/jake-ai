import { randomUUID } from 'node:crypto';
import type { AppContext } from './context';
import type { ChatMessage, ToolSchema } from './ai';
import { isoNow } from './db';
import { redact, safeStringify } from './redact';
import type { Severity } from '../../shared/types';

const SYSTEM = `You are JAKE, an AI cybersecurity assistant inside the JAKE AI desktop application.

Your job: help the operator assess and secure systems they own or are explicitly authorized to administer. You perform authorized website reconnaissance, safe vulnerability assessment, account security auditing, RBAC verification, PHP/MySQL review and API security testing.

Hard rules:
1. Only act on targets registered under Target Management. If the operator names something not registered, ask them to register it with an authorization note first.
2. Use tools for every real action. Never claim a scan, login, report or change happened unless a tool result confirms it. If a tool fails, say so and explain the error.
3. Never ask for, echo or store passwords, API keys, session cookies or password hashes. Connection profiles hold credentials; refer to them by profile name.
4. All scanning is non-destructive. Do not propose or attempt DoS, brute force, exploitation, data exfiltration or writes to production accounts.
5. Be precise about confidence and limitations. Distinguish evidence from inference. Absence of findings is not proof of safety.
6. Explain what you plan to do, which tool you will use and what approval it needs, then call the tool. Approval prompts are handled by the application, not by you.
7. Keep answers concise and technical. Use severity labels (critical/high/medium/low/info) and include concrete remediation.
8. You may summarise tool results, but never invent findings, CVEs, versions or evidence.

The application provides approvals, scope enforcement, rate limits, redaction and audit logging. You cannot execute shell commands, run arbitrary tools, or access the filesystem outside approved operations.`;

interface ToolDef {
  schema: ToolSchema;
  level: number;
  run: (ctx: AppContext, args: Record<string, unknown>, emit: (e: Record<string, unknown>) => void) => Promise<unknown>;
}

function str(args: Record<string, unknown>, key: string, required = true): string {
  const v = args[key];
  if (typeof v === 'string' && v.trim()) return v.trim();
  if (required) throw new Error(`Missing required parameter "${key}".`);
  return '';
}

function optionalStr(args: Record<string, unknown>, key: string): string | undefined {
  const v = args[key];
  return typeof v === 'string' && v.trim() ? v.trim() : undefined;
}

const TOOLS: Record<string, ToolDef> = {
  list_targets: {
    level: 1,
    schema: {
      type: 'function',
      function: {
        name: 'list_targets',
        description: 'List registered authorized targets with their IDs. Call this before scanning when you do not know the target ID.',
        parameters: { type: 'object', properties: {}, additionalProperties: false },
      },
    },
    run: async (ctx) => ctx.targets().map((t) => ({ id: t.id, label: t.label, url: t.url, kind: t.kind, rateLimit: t.rate_limit, allowPrivate: Boolean(t.allow_private) })),
  },
  list_connections: {
    level: 1,
    schema: {
      type: 'function',
      function: {
        name: 'list_connections',
        description: 'List stored authorized connection profiles (admin APIs, database servers) by ID. No credentials are returned.',
        parameters: { type: 'object', properties: {}, additionalProperties: false },
      },
    },
    run: async (ctx) => ctx.connections.list().map((c) => ({ id: c.id, name: c.name, kind: c.kind, baseUrl: c.base_url })),
  },
  start_recon: {
    level: 1,
    schema: {
      type: 'function',
      function: {
        name: 'start_recon',
        description: 'Passive reconnaissance of an authorized target: DNS, HTTP, TLS, headers, technologies, robots, sitemap, public endpoints and certificate-transparency subdomains.',
        parameters: { type: 'object', properties: { targetId: { type: 'string', description: 'Registered target ID' } }, required: ['targetId'], additionalProperties: false },
      },
    },
    run: async (ctx, args) => {
      const { assessmentId, data } = await ctx.runRecon(str(args, 'targetId'));
      return {
        assessmentId,
        httpStatus: data.httpStatus,
        finalUrl: data.finalUrl,
        ips: data.ips,
        technologies: data.technologies,
        tls: data.tls ? { protocol: data.tls.protocol, issuer: data.tls.issuer, authorized: data.tls.authorized, daysRemaining: data.tls.daysRemaining } : null,
        securityHeadersPresent: ['strict-transport-security', 'content-security-policy', 'x-frame-options', 'x-content-type-options'].filter((h) => data.headers[h]),
        endpoints: data.endpoints.slice(0, 40),
        subdomains: data.subdomains.slice(0, 40),
        error: data.error,
      };
    },
  },
  start_vulnerability_scan: {
    level: 2,
    schema: {
      type: 'function',
      function: {
        name: 'start_vulnerability_scan',
        description: 'Run the safe OWASP-aligned vulnerability assessment against an authorized target. Requires operator approval. Non-destructive: no exploits or state-changing requests.',
        parameters: { type: 'object', properties: { targetId: { type: 'string' } }, required: ['targetId'], additionalProperties: false },
      },
    },
    run: async (ctx, args) => {
      const { assessmentId, result } = await ctx.runVulnerability(args.targetId ? String(args.targetId) : '');
      return { assessmentId, summary: `Scan finished. ${result.findings} findings across ${result.requests} requests.`, notes: result.notes.slice(0, 6), topFindings: topFindings(ctx, assessmentId, 12) };
    },
  },
  audit_accounts: {
    level: 3,
    schema: {
      type: 'function',
      function: {
        name: 'audit_accounts',
        description: 'Read-only account security audit through an authorized admin API connection profile: dormant/duplicate/privileged accounts, MFA, policy and lockout settings.',
        parameters: { type: 'object', properties: { connectionId: { type: 'string' } }, required: ['connectionId'], additionalProperties: false },
      },
    },
    run: async (ctx, args) => {
      const { assessmentId, result } = await ctx.runAccounts(str(args, 'connectionId'));
      return {
        assessmentId,
        accounts: result.total,
        active: result.active,
        admins: result.admins,
        dormant: result.dormant,
        withoutMfa: result.withoutMfa,
        issues: result.issues.map((i) => ({ severity: i.severity, name: i.name, detail: i.detail, affected: i.accounts.length })),
        notes: result.notes,
      };
    },
  },
  audit_rbac: {
    level: 3,
    schema: {
      type: 'function',
      function: {
        name: 'audit_rbac',
        description: 'Test horizontal/vertical access control with dedicated admin and viewer test credentials on configured safe routes. Requires approval.',
        parameters: { type: 'object', properties: { connectionId: { type: 'string' } }, required: ['connectionId'], additionalProperties: false },
      },
    },
    run: async (ctx, args) => {
      const { assessmentId, result } = await ctx.runRbac(str(args, 'connectionId'));
      return { assessmentId, summary: result.summary, rows: result.rows.slice(0, 40), notes: result.notes };
    },
  },
  scan_api: {
    level: 3,
    schema: {
      type: 'function',
      function: {
        name: 'scan_api',
        description: 'Assess API routes for missing authentication, sensitive data exposure, CORS, caching, methods and rate limiting. Requires approval.',
        parameters: { type: 'object', properties: { connectionId: { type: 'string' } }, required: ['connectionId'], additionalProperties: false },
      },
    },
    run: async (ctx, args) => {
      const { assessmentId, result } = await ctx.runApi(str(args, 'connectionId'));
      return { assessmentId, summary: result.summary, rows: result.rows, notes: result.notes };
    },
  },
  audit_php: {
    level: 2,
    schema: {
      type: 'function',
      function: {
        name: 'audit_php',
        description: 'Read-only static security review of a local PHP project directory (SQL injection, XSS, hashing, uploads, CSRF, sessions, secrets).',
        parameters: { type: 'object', properties: { directory: { type: 'string', description: 'Absolute path to the PHP project' } }, required: ['directory'], additionalProperties: false },
      },
    },
    run: async (ctx, args) => {
      const { assessmentId, result } = await ctx.runPhp(str(args, 'directory'));
      const grouped = new Map<string, { severity: Severity; count: number; file: string; line: number }>();
      for (const i of result.issues) {
        const key = i.message;
        const cur = grouped.get(key);
        if (cur) cur.count++;
        else grouped.set(key, { severity: i.severity, count: 1, file: i.file, line: i.line });
      }
      return {
        assessmentId,
        filesScanned: result.filesScanned,
        issueCount: result.issues.length,
        groups: [...grouped.entries()].map(([name, v]) => ({ name, ...v })).slice(0, 20),
        truncated: result.truncated,
      };
    },
  },
  audit_mysql: {
    level: 3,
    schema: {
      type: 'function',
      function: {
        name: 'audit_mysql',
        description: 'Read-only MySQL/MariaDB security audit through a database connection profile: privileges, hash algorithm, schema, server hardening.',
        parameters: { type: 'object', properties: { connectionId: { type: 'string' } }, required: ['connectionId'], additionalProperties: false },
      },
    },
    run: async (ctx, args) => {
      const { assessmentId, result } = await ctx.runMysql(str(args, 'connectionId'));
      return { assessmentId, server: result.server, database: result.database, tables: result.tables, userTables: result.userTables, issues: result.issues, notes: result.notes };
    },
  },
  list_findings: {
    level: 1,
    schema: {
      type: 'function',
      function: {
        name: 'list_findings',
        description: 'Read recorded findings, optionally filtered by severity or assessment.',
        parameters: {
          type: 'object',
          properties: { severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low', 'info'] }, assessmentId: { type: 'string' }, limit: { type: 'number' } },
          additionalProperties: false,
        },
      },
    },
    run: async (ctx, args) => {
      const limit = Math.min(60, Math.max(1, Number(args.limit ?? 30)));
      const rows = ctx.findings({ severity: optionalStr(args, 'severity') as Severity | undefined }).slice(0, 300);
      const filtered = optionalStr(args, 'assessmentId') ? rows.filter((r) => (r as { assessment_id: string }).assessment_id === args.assessmentId) : rows;
      return filtered.slice(0, limit).map((r) => {
        const f = r as Record<string, unknown>;
        return { id: f.id, name: f.name, severity: f.severity, confidence: f.confidence, endpoint: f.endpoint, status: f.status, assessmentId: f.assessment_id };
      });
    },
  },
  get_assessment: {
    level: 1,
    schema: {
      type: 'function',
      function: {
        name: 'get_assessment',
        description: 'Read one assessment: status, summary, finding counts and finding names.',
        parameters: { type: 'object', properties: { assessmentId: { type: 'string' } }, required: ['assessmentId'], additionalProperties: false },
      },
    },
    run: async (ctx, args) => {
      const id = str(args, 'assessmentId');
      const assessment = ctx.assessments.get(id);
      if (!assessment) throw new Error('Assessment not found.');
      const findings = (ctx.assessments.findingsFor(id) as Array<Record<string, unknown>>).map((f) => ({
        name: f.name,
        severity: f.severity,
        endpoint: f.endpoint,
        remediation: String(f.remediation).slice(0, 240),
      }));
      return { ...assessment, findings };
    },
  },
  list_security_tools: {
    level: 1,
    schema: {
      type: 'function',
      function: {
        name: 'list_security_tools',
        description: 'Show installed security tools, versions and WSL status.',
        parameters: { type: 'object', properties: {}, additionalProperties: false },
      },
    },
    run: async (ctx) => ({ wsl: ctx.wslStatus(), tools: ctx.toolList() }),
  },
  run_external_tool: {
    level: 2,
    schema: {
      type: 'function',
      function: {
        name: 'run_external_tool',
        description: 'Run an installed external scanner (nmap/nikto/zap baseline) with fixed, validated arguments against an authorized target. Requires approval.',
        parameters: {
          type: 'object',
          properties: { tool: { type: 'string', enum: ['nmap', 'nikto', 'zap'] }, targetId: { type: 'string' }, topPorts: { type: 'number', description: 'nmap only, 10-1000' } },
          required: ['tool', 'targetId'],
          additionalProperties: false,
        },
      },
    },
    run: async (ctx, args) => {
      const tool = str(args, 'tool');
      if (!['nmap', 'nikto', 'zap'].includes(tool)) throw new Error('Unsupported external tool.');
      const { assessmentId, result } = await ctx.runTool(tool as 'nmap' | 'nikto' | 'zap', str(args, 'targetId'), { topPorts: Number(args.topPorts ?? 100) });
      return { assessmentId, status: result.status, exitCode: result.exitCode, command: result.command, output: result.output.slice(-6000) };
    },
  },
  generate_report: {
    level: 1,
    schema: {
      type: 'function',
      function: {
        name: 'generate_report',
        description: 'Generate an HTML, JSON or PDF report for an assessment.',
        parameters: {
          type: 'object',
          properties: { assessmentId: { type: 'string' }, format: { type: 'string', enum: ['html', 'json', 'pdf'] } },
          required: ['assessmentId', 'format'],
          additionalProperties: false,
        },
      },
    },
    run: async (ctx, args) => {
      const format = str(args, 'format');
      if (!['html', 'json', 'pdf'].includes(format)) throw new Error('Format must be html, json or pdf.');
      const res = await ctx.generateReport(str(args, 'assessmentId'), format as 'html' | 'json' | 'pdf');
      return { path: res.path, format };
    },
  },
  open_authorized_login: {
    level: 3,
    schema: {
      type: 'function',
      function: {
        name: 'open_authorized_login',
        description: 'Open a headed browser to an authorized login page so the operator can sign in manually. JAKE never receives the password.',
        parameters: { type: 'object', properties: { targetId: { type: 'string' }, url: { type: 'string' } }, required: ['targetId', 'url'], additionalProperties: false },
      },
    },
    run: async (ctx, args) => {
      const targetId = str(args, 'targetId');
      const url = str(args, 'url');
      const ok = await ctx.approval({ level: 3, title: 'Open authorized browser login', detail: `JAKE will open ${url} in a system browser for manual sign-in. Credentials are typed by you, not JAKE.`, assessmentId: null });
      if (!ok) throw new Error('Operator denied opening the browser session.');
      return ctx.openAccess(targetId, url);
    },
  },
  capture_authenticated_page: {
    level: 3,
    schema: {
      type: 'function',
      function: {
        name: 'capture_authenticated_page',
        description: 'Capture the current authorized browser page structure (forms, navigation, table headers) for inspection. Password values are stripped.',
        parameters: { type: 'object', properties: {}, additionalProperties: false },
      },
    },
    run: async (ctx) => ctx.captureAccessPage(),
  },
};

export function toolSchemas(): ToolSchema[] {
  return Object.values(TOOLS).map((t) => t.schema);
}

function topFindings(ctx: AppContext, assessmentId: string, limit: number) {
  return (ctx.assessments.findingsFor(assessmentId) as Array<Record<string, unknown>>)
    .sort((a, b) => rank(String(b.severity)) - rank(String(a.severity)))
    .slice(0, limit)
    .map((f) => ({ name: f.name, severity: f.severity, endpoint: f.endpoint, remediation: f.remediation }));
}

function rank(s: string): number {
  return { critical: 5, high: 4, medium: 3, low: 2, info: 1 }[s] ?? 0;
}

export class ChatAgent {
  constructor(private ctx: AppContext) {}

  ensureConversation(id?: string): string {
    if (id) {
      const row = this.ctx.db.get<{ id: string }>('SELECT id FROM conversations WHERE id = ?', [id]);
      if (!row) throw new Error('Conversation not found.');
      return id;
    }
    const newId = randomUUID();
    const now = isoNow();
    this.ctx.db.run('INSERT INTO conversations (id, title, created_at, updated_at) VALUES (?,?,?,?)', [newId, 'New conversation', now, now]);
    return newId;
  }

  conversations() {
    return this.ctx.db.all('SELECT * FROM conversations ORDER BY updated_at DESC LIMIT 50');
  }

  history(conversationId: string) {
    return this.ctx.db.all('SELECT id, role, content, created_at FROM messages WHERE conversation_id = ? ORDER BY id ASC', [conversationId]);
  }

  deleteConversation(id: string): void {
    this.ctx.db.run('DELETE FROM conversations WHERE id = ?', [id]);
  }

  private persist(conversationId: string, role: 'user' | 'assistant' | 'tool', content: string): void {
    const now = isoNow();
    this.ctx.db.run('INSERT INTO messages (conversation_id, role, content, created_at) VALUES (?,?,?,?)', [conversationId, role, content, now]);
    this.ctx.db.run('UPDATE conversations SET updated_at = ? WHERE id = ?', [now, conversationId]);
  }

  private systemPrompt(): string {
    const targets = this.ctx.targets().map((t) => `- ${t.label} [${t.id}] ${t.url} (rate ${t.rate_limit}/s${t.allow_private ? ', private network allowed' : ''})`);
    const connections = this.ctx.connections.list().map((c) => `- ${c.name} [${c.id}] kind=${c.kind} ${c.base_url ?? ''}`);
    const stats = this.ctx.assessments.stats();
    return `${SYSTEM}

Current context (${isoNow()}):
Authorized targets:
${targets.join('\n') || '- none registered yet; ask the operator to register one in Target Management'}
Connection profiles:
${connections.join('\n') || '- none configured'}
Recorded findings: ${stats.findings} (critical ${stats.bySeverity.critical}, high ${stats.bySeverity.high}, medium ${stats.bySeverity.medium}, low ${stats.bySeverity.low}, info ${stats.bySeverity.info}).

If the operator asks you to test something that is not registered, explain that authorization must be recorded first. Ask for a target ID when several targets exist and the request is ambiguous.`;
  }

  async run(opts: { conversationId: string; text: string; signal: AbortSignal; emit: (e: Record<string, unknown>) => void }): Promise<{ content: string; error?: string }> {
    const { conversationId, text, signal, emit } = opts;
    this.persist(conversationId, 'user', text);
    const row = this.ctx.db.get<{ title: string }>('SELECT title FROM conversations WHERE id = ?', [conversationId]);
    if (row?.title === 'New conversation') {
      this.ctx.db.run('UPDATE conversations SET title = ? WHERE id = ?', [text.slice(0, 70), conversationId]);
    }

    const history = (this.ctx.db.all<{ role: string; content: string }>('SELECT role, content FROM messages WHERE conversation_id = ? ORDER BY id ASC LIMIT 60', [conversationId]) ?? []).filter(
      (m) => m.role === 'user' || m.role === 'assistant',
    );
    const messages: ChatMessage[] = [{ role: 'system', content: this.systemPrompt() }, ...history.map((m) => ({ role: m.role as 'user' | 'assistant', content: redact(m.content) }))];

    let finalText = '';
    let error: string | undefined;
    try {
      for (let step = 0; step < 6; step++) {
        if (signal.aborted) throw new Error('Stopped by the operator.');
        let stepText = '';
        let calls: ChatMessage['tool_calls'] = undefined;
        for await (const ev of this.ctx.ai.stream({ messages, tools: toolSchemas(), signal })) {
          if (ev.delta) {
            stepText += ev.delta;
            finalText += ev.delta;
            emit({ type: 'chat:delta', conversationId, text: ev.delta });
          }
          if (ev.toolCalls && ev.toolCalls.length) calls = ev.toolCalls;
        }
        if (!calls || calls.length === 0) break;
        messages.push({ role: 'assistant', content: stepText || null, tool_calls: calls });
        // Do not persist narration per step; finalText (persisted below) already
        // contains it, and a second row would duplicate the reply in history.
        for (const call of calls) {
          const name = call.function.name;
          const def = TOOLS[name];
          emit({ type: 'chat:tool', conversationId, tool: name, status: 'started' });
          let resultContent: string;
          if (!def) {
            resultContent = JSON.stringify({ error: `Unknown tool: ${name}` });
          } else {
            try {
              const parsed = call.function.arguments ? (JSON.parse(call.function.arguments) as Record<string, unknown>) : {};
              const result = await def.run(this.ctx, parsed, (e) => emit({ ...e, conversationId }));
              resultContent = safeStringify(result).slice(0, 12_000);
            } catch (err) {
              resultContent = JSON.stringify({ error: (err as Error).message });
              this.ctx.audit.record('agent', 'tool.error', name, def.level, (err as Error).message);
            }
          }
          messages.push({ role: 'tool', tool_call_id: call.id, name, content: resultContent });
          this.persist(conversationId, 'tool', `[${name}] ${redact(resultContent).slice(0, 6000)}`);
          emit({ type: 'chat:tool', conversationId, tool: name, status: 'finished', summary: redact(resultContent).slice(0, 600) });
        }
        if (step === 5) {
          const note = '\n\nI reached the six-step execution limit for one request. Ask me to continue if more work is needed.';
          finalText += note;
          emit({ type: 'chat:delta', conversationId, text: note });
        }
      }
    } catch (err) {
      error = (err as Error).message;
      emit({ type: 'chat:error', conversationId, message: error });
    }
    if (!finalText.trim()) {
      finalText = error ? `I could not complete that request: ${error}` : 'I did not produce a response. Please rephrase or check the AI settings.';
      emit({ type: 'chat:delta', conversationId, text: finalText });
    }
    this.persist(conversationId, 'assistant', finalText);
    return { content: finalText, error };
  }
}
