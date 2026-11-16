import { EventEmitter } from 'node:events';
import { randomUUID } from 'node:crypto';
import type { Database } from './db';
import { isoNow } from './db';
import { logSafe } from './store';
import { redact } from './redact';

export class AuditLog {
  constructor(private db: Database) {}

  record(actor: string, action: string, target: string | null, level: number, detail?: unknown): void {
    const text = detail === undefined ? null : redact(detail).slice(0, 2000);
    this.db.run('INSERT INTO audit_logs (actor, action, target, level, detail, created_at) VALUES (?,?,?,?,?,?)', [
      actor,
      action,
      target,
      level,
      text,
      isoNow(),
    ]);
    logSafe('audit', `${actor} ${action} ${target ?? ''}`, text ?? undefined);
  }

  list(limit = 500): unknown[] {
    return this.db.all('SELECT * FROM audit_logs ORDER BY id DESC LIMIT ?', [Math.min(limit, 2000)]);
  }
}

interface PendingApproval {
  resolve: (v: boolean) => void;
  timer: NodeJS.Timeout;
}

/**
 * Human approval checkpoints (permission levels 1-4).
 * Every decision is recorded in the approvals table plus the audit log.
 */
export class ApprovalManager extends EventEmitter {
  private pending = new Map<string, PendingApproval>();

  constructor(private db: Database, private audit: AuditLog) {
    super();
  }

  async request(opts: {
    level: number;
    title: string;
    detail: string;
    assessmentId?: string | null;
    autoApproveAt: number;
    timeoutMs?: number;
  }): Promise<boolean> {
    if (opts.level <= 1) return true;
    if (opts.level <= opts.autoApproveAt) {
      this.record(opts, opts.assessmentId ?? null, 'auto-approved');
      return true;
    }
    const id = randomUUID();
    const timeoutMs = opts.timeoutMs ?? 120_000;
    // Persist the pending request up-front so the operator's decision updates
    // the real approval row (level, title, detail) instead of a placeholder.
    this.db.run('INSERT INTO approvals (id, assessment_id, level, title, detail, decision, created_at) VALUES (?,?,?,?,?,?,?)', [
      id,
      opts.assessmentId ?? null,
      opts.level,
      opts.title.slice(0, 300),
      redact(opts.detail).slice(0, 2000),
      'pending',
      isoNow(),
    ]);
    return new Promise<boolean>((resolve) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        // Close out the same pending row rather than creating a duplicate entry.
        this.db.run('UPDATE approvals SET decision = ? WHERE id = ?', ['timeout-denied', id]);
        this.audit.record('system', 'approval.timeout-denied', null, opts.level, opts.title);
        resolve(false);
      }, timeoutMs);
      this.pending.set(id, { resolve, timer });
      this.emit('request', {
        id,
        level: opts.level,
        title: opts.title,
        detail: redact(opts.detail).slice(0, 4000),
      });
    });
  }

  respond(id: string, approved: boolean): boolean {
    const p = this.pending.get(id);
    if (!p) return false;
    clearTimeout(p.timer);
    this.pending.delete(id);
    const row = this.db.get<{ level: number; title: string; detail: string }>(
      'SELECT level, title, detail FROM approvals WHERE id = ?',
      [id],
    );
    this.db.run('UPDATE approvals SET decision = ? WHERE id = ?', [approved ? 'approved' : 'denied', id]);
    this.audit.record('user', approved ? 'approval.approved' : 'approval.denied', null, row?.level ?? 1, row?.title ?? id);
    p.resolve(approved);
    return true;
  }

  /** Deny anything still waiting (emergency stop / window closed). */
  cancelAll(): void {
    for (const [, p] of this.pending) {
      clearTimeout(p.timer);
      p.resolve(false);
    }
    this.pending.clear();
    this.db.run("UPDATE approvals SET decision = 'cancelled' WHERE decision = 'pending'");
  }

  private record(opts: { level: number; title: string; detail: string }, assessmentId: string | null, decision: string): void {
    const id = randomUUID();
    this.db.run('INSERT INTO approvals (id, assessment_id, level, title, detail, decision, created_at) VALUES (?,?,?,?,?,?,?)', [
      id,
      assessmentId,
      opts.level,
      opts.title.slice(0, 300),
      redact(opts.detail).slice(0, 2000),
      decision,
      isoNow(),
    ]);
    this.audit.record('system', `approval.${decision}`, null, opts.level, opts.title);
  }
}
