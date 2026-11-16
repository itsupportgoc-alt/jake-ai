import { randomUUID } from 'node:crypto';
import type { Database } from './db';
import { isoNow } from './db';
import type { Assessment, AssessmentType, FindingInput, Severity } from '../../shared/types';
import { redact } from './redact';

export class Assessments {
  constructor(private db: Database) {}

  create(type: AssessmentType, targetLabel: string, targetId: string | null): Assessment {
    const id = randomUUID();
    const now = isoNow();
    this.db.run(
      'INSERT INTO assessments (id, target_id, target_label, type, status, progress, started_at) VALUES (?,?,?,?,?,?,?)',
      [id, targetId, targetLabel, type, 'running', 0, now],
    );
    return this.get(id)!;
  }

  get(id: string): Assessment | undefined {
    return this.db.get<Assessment>('SELECT * FROM assessments WHERE id = ?', [id]);
  }

  list(limit = 100): Assessment[] {
    return this.db.all<Assessment>('SELECT * FROM assessments ORDER BY started_at DESC LIMIT ?', [Math.min(limit, 500)]);
  }

  progress(id: string, progress: number): void {
    this.db.run('UPDATE assessments SET progress = ? WHERE id = ?', [Math.max(0, Math.min(100, Math.round(progress))), id]);
  }

  finish(id: string, status: 'complete' | 'failed' | 'stopped', summary?: string, error?: string): void {
    this.db.run('UPDATE assessments SET status = ?, progress = CASE WHEN ? = \'complete\' THEN 100 ELSE progress END, summary = ?, error = ?, finished_at = ? WHERE id = ?', [
      status,
      status,
      summary ? redact(summary).slice(0, 4000) : null,
      error ? redact(error).slice(0, 2000) : null,
      isoNow(),
      id,
    ]);
  }

  addFinding(assessmentId: string, target: string, f: FindingInput): string {
    const id = randomUUID();
    this.db.run(
      `INSERT INTO findings (id, assessment_id, target, name, category, severity, confidence, endpoint, evidence, description, impact, reproduction, remediation, "references", status, created_at)
       VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)`,
      [
        id,
        assessmentId,
        target,
        f.name.slice(0, 300),
        f.category,
        f.severity,
        f.confidence,
        f.endpoint.slice(0, 1000),
        redact(f.evidence).slice(0, 6000),
        f.description.slice(0, 4000),
        f.impact.slice(0, 2000),
        f.reproduction.slice(0, 3000),
        f.remediation.slice(0, 3000),
        JSON.stringify(f.references ?? []),
        'open',
        isoNow(),
      ],
    );
    return id;
  }

  findingsFor(assessmentId: string): unknown[] {
    return this.db.all('SELECT * FROM findings WHERE assessment_id = ? ORDER BY created_at', [assessmentId]);
  }

  listFindings(filters: { severity?: Severity; status?: string; search?: string; limit?: number } = {}): unknown[] {
    const where: string[] = [];
    const params: unknown[] = [];
    if (filters.severity) {
      where.push('severity = ?');
      params.push(filters.severity);
    }
    if (filters.status) {
      where.push('status = ?');
      params.push(filters.status);
    }
    if (filters.search) {
      where.push('(name LIKE ? OR endpoint LIKE ? OR target LIKE ? OR category LIKE ?)');
      const q = `%${filters.search}%`;
      params.push(q, q, q, q);
    }
    const sql = `SELECT * FROM findings ${where.length ? 'WHERE ' + where.join(' AND ') : ''} ORDER BY
      CASE severity WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 WHEN 'low' THEN 3 ELSE 4 END,
      created_at DESC LIMIT ?`;
    params.push(Math.min(filters.limit ?? 500, 2000));
    return this.db.all(sql, params);
  }

  stats() {
    const counts = this.db.all<{ severity: Severity; n: number }>('SELECT severity, COUNT(*) AS n FROM findings GROUP BY severity');
    const bySeverity = { critical: 0, high: 0, medium: 0, low: 0, info: 0 } as Record<Severity, number>;
    for (const c of counts) bySeverity[c.severity] = Number(c.n);
    return {
      assessments: Number(this.db.get<{ n: number }>('SELECT COUNT(*) AS n FROM assessments')?.n ?? 0),
      findings: Number(this.db.get<{ n: number }>('SELECT COUNT(*) AS n FROM findings')?.n ?? 0),
      bySeverity,
      accountIssues: Number(
        this.db.get<{ n: number }>(`SELECT COUNT(*) AS n FROM findings WHERE category IN ('accounts','authentication')`)?.n ?? 0,
      ),
      permissionIssues: Number(this.db.get<{ n: number }>(`SELECT COUNT(*) AS n FROM findings WHERE category IN ('rbac','access-control','api')`)?.n ?? 0),
      recentAssessments: this.list(8),
    };
  }
}
