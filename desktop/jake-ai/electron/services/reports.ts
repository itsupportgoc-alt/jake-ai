import fs from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { BrowserWindow } from 'electron';
import type { Database } from './db';
import { isoNow } from './db';
import type { Assessments } from './assessments';
import type { Store } from './store';
import type { AuditLog } from './audit';
import type { Finding, Severity } from '../../shared/types';
import { redact } from './redact';

interface ReportFinding extends Finding {
  refs: string[];
}

export interface ReportBundle {
  generatedAt: string;
  assessment: Record<string, unknown>;
  findings: ReportFinding[];
  recon: unknown[];
  accountAudits: unknown[];
  rbac: unknown[];
  api: unknown[];
  approvals: unknown[];
  audit: unknown[];
  severityCounts: Record<Severity, number>;
  riskRating: string;
}

export class Reports {
  constructor(private db: Database, private assessments: Assessments, private store: Store, private audit: AuditLog) {}

  bundle(assessmentId: string): ReportBundle {
    const assessment = this.db.get<Record<string, unknown>>('SELECT * FROM assessments WHERE id = ?', [assessmentId]);
    if (!assessment) throw new Error('Assessment not found');
    const findings = this.db.all<Record<string, unknown>>('SELECT * FROM findings WHERE assessment_id = ? ORDER BY created_at', [assessmentId]).map((f) => ({
      ...(f as unknown as Finding),
      refs: safeJsonArray(String(f.references ?? '[]')),
    }));
    const counts: Record<Severity, number> = { critical: 0, high: 0, medium: 0, low: 0, info: 0 };
    for (const f of findings) counts[f.severity] = (counts[f.severity] ?? 0) + 1;
    const riskRating =
      counts.critical > 0 ? 'Critical' : counts.high > 0 ? 'High' : counts.medium > 0 ? 'Medium' : counts.low > 0 ? 'Low' : 'Informational';
    return {
      generatedAt: isoNow(),
      assessment,
      findings,
      recon: this.db.all('SELECT data, created_at FROM recon_results WHERE assessment_id = ?', [assessmentId]).map((r) => safeParse(String((r as { data: string }).data))),
      accountAudits: this.db.all('SELECT source, summary, data, created_at FROM account_audits WHERE assessment_id = ?', [assessmentId]).map((a) => ({
        source: (a as { source: string }).source,
        summary: (a as { summary: string }).summary,
        data: safeParse(String((a as { data: string }).data)),
        created_at: (a as { created_at: string }).created_at,
      })),
      rbac: this.db.all('SELECT profile, summary, matrix, created_at FROM rbac_results WHERE assessment_id = ?', [assessmentId]).map((r) => ({
        ...(r as Record<string, unknown>),
        matrix: safeParse(String((r as { matrix: string }).matrix)),
      })),
      api: this.db.all('SELECT profile, summary, data, created_at FROM api_results WHERE assessment_id = ?', [assessmentId]).map((r) => ({
        ...(r as Record<string, unknown>),
        data: safeParse(String((r as { data: string }).data)),
      })),
      approvals: this.db.all('SELECT level, title, decision, created_at FROM approvals WHERE assessment_id = ?', [assessmentId]),
      audit: this.db.all('SELECT actor, action, level, detail, created_at FROM audit_logs ORDER BY id DESC LIMIT 200'),
      severityCounts: counts,
      riskRating,
    };
  }

  async generate(assessmentId: string, format: 'html' | 'json' | 'pdf'): Promise<{ id: string; path: string }> {
    const bundle = this.bundle(assessmentId);
    const dir = this.store.reportsDir();
    const base = `jake-report-${String(bundle.assessment.target_label ?? 'target').replace(/[^A-Za-z0-9._-]/g, '_').slice(0, 40)}-${new Date().toISOString().slice(0, 10)}-${assessmentId.slice(0, 6)}`;
    const id = randomUUID();
    let file = path.join(dir, `${base}.${format === 'pdf' ? 'pdf' : format}`);
    if (format === 'json') {
      fs.writeFileSync(file, JSON.stringify(bundle, null, 2), 'utf8');
    } else {
      const html = renderHtml(bundle);
      if (format === 'html') fs.writeFileSync(file, html, 'utf8');
      else file = await this.renderPdf(html, file);
    }
    this.db.run('INSERT INTO reports (id, assessment_id, format, path, created_at) VALUES (?,?,?,?,?)', [id, assessmentId, format, file, isoNow()]);
    this.audit.record('user', 'report.generate', bundle.assessment.target_label as string, 1, `${format.toUpperCase()} report written to ${path.basename(file)}`);
    return { id, path: file };
  }

  private async renderPdf(html: string, outFile: string): Promise<string> {
    const win = new BrowserWindow({
      show: false,
      webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false, javascript: false },
    });
    const tmp = path.join(this.store.reportsDir(), `.render-${Date.now()}.html`);
    fs.writeFileSync(tmp, html, 'utf8');
    try {
      await win.loadFile(tmp);
      const pdf = await win.webContents.printToPDF({ printBackground: true, pageSize: 'A4', margins: { top: 0.5, bottom: 0.5, left: 0.5, right: 0.5 } });
      fs.writeFileSync(outFile, pdf);
    } finally {
      win.destroy();
      fs.rmSync(tmp, { force: true });
    }
    return outFile;
  }

  list(): unknown[] {
    return this.db.all('SELECT * FROM reports ORDER BY created_at DESC LIMIT 200');
  }
}

function safeParse(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

function safeJsonArray(raw: string): string[] {
  try {
    const v = JSON.parse(raw);
    return Array.isArray(v) ? v.map(String) : [];
  } catch {
    return [];
  }
}

function esc(v: unknown): string {
  return String(v ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

export function renderHtml(b: ReportBundle): string {
  const f = b.assessment;
  const sev = (s: Severity) => `<span class="sev ${s}">${s.toUpperCase()}</span>`;
  const total = b.findings.length;
  const riskLine = `Overall risk rating: <strong>${esc(b.riskRating)}</strong> (${b.severityCounts.critical} critical, ${b.severityCounts.high} high, ${b.severityCounts.medium} medium, ${b.severityCounts.low} low, ${b.severityCounts.info} informational).`;
  const findingsHtml = b.findings
    .map(
      (x, i) => `
    <section class="finding">
      <h3>${i + 1}. ${esc(x.name)} ${sev(x.severity)}</h3>
      <table class="meta">
        <tr><th>Affected endpoint</th><td>${esc(x.endpoint)}</td></tr>
        <tr><th>Category</th><td>${esc(x.category)}</td></tr>
        <tr><th>Confidence</th><td>${esc(x.confidence)}</td></tr>
        <tr><th>Evidence</th><td><pre>${esc(redact(x.evidence))}</pre></td></tr>
        <tr><th>Description</th><td>${esc(x.description)}</td></tr>
        <tr><th>Impact</th><td>${esc(x.impact)}</td></tr>
        <tr><th>Safe reproduction</th><td>${esc(x.reproduction)}</td></tr>
        <tr><th>Remediation</th><td>${esc(x.remediation)}</td></tr>
        <tr><th>References</th><td>${x.refs.map((r) => `<a href="${esc(r)}">${esc(r)}</a>`).join('<br/>')}</td></tr>
        <tr><th>Status</th><td>${esc(x.status)}</td></tr>
      </table>
    </section>`,
    )
    .join('\n');

  const recon = b.recon[0] as
    | { ips?: string[]; httpStatus?: number; technologies?: string[]; tls?: { protocol?: string; issuer?: string; daysRemaining?: number }; server?: string; endpoints?: string[]; subdomains?: string[]; robots?: { disallow?: string[] } }
    | undefined;

  return `<!doctype html>
<html lang="en"><head><meta charset="utf-8"/>
<title>JAKE AI Security Report — ${esc(f.target_label)}</title>
<style>
  :root { color-scheme: light; }
  body { font-family: "Segoe UI", Arial, sans-serif; color:#16202a; margin: 32px; line-height:1.5; font-size:12px; }
  h1 { font-size: 26px; margin:0 0 4px; color:#0b2533; }
  h2 { font-size: 17px; margin-top:28px; border-bottom:2px solid #0e7490; padding-bottom:4px; color:#0b2533; }
  h3 { font-size: 13px; margin: 18px 0 6px; }
  .sub { color:#4b5b6b; margin-bottom: 18px; }
  .summary { background:#f1f6f9; border-left:4px solid #0e7490; padding:12px 16px; margin:14px 0; }
  table.meta { width:100%; border-collapse:collapse; }
  table.meta th { width: 165px; text-align:left; vertical-align:top; color:#33475b; font-weight:600; background:#f7fafc; }
  table.meta th, table.meta td { border:1px solid #d7e0e8; padding:5px 8px; }
  table.grid { width:100%; border-collapse:collapse; margin-top:8px; }
  table.grid th, table.grid td { border:1px solid #d7e0e8; padding:5px 8px; text-align:left; }
  table.grid th { background:#eef4f8; }
  pre { white-space:pre-wrap; word-break:break-word; margin:0; font-family:Consolas, monospace; font-size:11px; }
  .sev { display:inline-block; padding:1px 7px; border-radius:3px; color:#fff; font-size:10px; letter-spacing:.4px; }
  .sev.critical { background:#9f1239; } .sev.high { background:#c2410c; } .sev.medium { background:#a16207; }
  .sev.low { background:#0e7490; } .sev.info { background:#475569; }
  .kpi { display:flex; gap:10px; flex-wrap:wrap; margin:12px 0; }
  .kpi div { border:1px solid #d7e0e8; border-radius:5px; padding:8px 14px; min-width:86px; }
  .kpi b { display:block; font-size:18px; }
  footer { margin-top:32px; border-top:1px solid #d7e0e8; padding-top:8px; color:#6b7c8d; font-size:10px; }
  .finding { page-break-inside: avoid; }
  a { color:#0e7490; }
</style></head><body>
<h1>JAKE AI — Security Assessment Report</h1>
<div class="sub">Target: <strong>${esc(f.target_label)}</strong> &nbsp;·&nbsp; Assessment type: ${esc(f.type)} &nbsp;·&nbsp; Generated ${esc(b.generatedAt)}</div>

<div class="kpi">
  <div><b>${b.findings.length}</b>Findings</div>
  <div><b>${b.severityCounts.critical}</b>Critical</div>
  <div><b>${b.severityCounts.high}</b>High</div>
  <div><b>${b.severityCounts.medium}</b>Medium</div>
  <div><b>${b.severityCounts.low}</b>Low</div>
  <div><b>${b.severityCounts.info}</b>Info</div>
</div>

<h2>1. Executive summary</h2>
<div class="summary">
  <p>JAKE AI performed an authorized ${esc(f.type)} assessment of <strong>${esc(f.target_label)}</strong>.
  ${total === 0 ? 'No findings were recorded in this assessment.' : `The assessment identified ${total} finding(s).`} ${riskLine}</p>
  <p>${esc(f.summary ?? 'See the methodology and findings sections for details.')}</p>
</div>

<h2>2. Target information</h2>
<table class="meta">
  <tr><th>Registered target</th><td>${esc(f.target_label)}</td></tr>
  <tr><th>Assessment ID</th><td>${esc(f.id)}</td></tr>
  <tr><th>Started</th><td>${esc(f.started_at)}</td></tr>
  <tr><th>Finished</th><td>${esc(f.finished_at ?? 'in progress')}</td></tr>
  <tr><th>Resolved addresses</th><td>${esc((recon?.ips ?? []).join(', ') || 'not recorded')}</td></tr>
  <tr><th>HTTP status</th><td>${esc(recon?.httpStatus ?? 'n/a')}</td></tr>
  <tr><th>Server / technologies</th><td>${esc(recon?.server ?? 'n/a')} · ${esc((recon?.technologies ?? []).join(', ') || 'not fingerprinted')}</td></tr>
  <tr><th>TLS</th><td>${recon?.tls ? `${esc(recon.tls.protocol)} · issuer ${esc(recon.tls.issuer)} · ${esc(recon.tls.daysRemaining)} days remaining` : 'not collected'}</td></tr>
</table>

<h2>3. Authorization scope</h2>
<p>Testing was limited to the registered target and its configured allowlist. Authorization was confirmed by the operator before execution.
The agent enforces scope on every request and redirect, applies rate limits, and blocks private networks unless explicitly permitted. No destructive or exploit payloads were used.</p>

<h2>4. Assessment methodology</h2>
<ul>
  <li>Passive reconnaissance: DNS, TLS, HTTP headers, technology fingerprinting, robots.txt, sitemap, certificate transparency.</li>
  <li>Safe active checks: security headers, cookie flags, transport, sensitive path exposure, admin interface discovery, CORS, CSRF form heuristics, reflective input probes.</li>
  <li>Account and RBAC review against an authorized admin API using dedicated test credentials (where configured).</li>
  <li>Read-only PHP source inspection and MySQL metadata/privilege review (where configured).</li>
  <li>All operations are permission-gated and recorded in the audit log.</li>
</ul>

<h2>5. Tools used</h2>
<p>JAKE built-in web assessment engine, PHP/MySQL static auditor, MySQL metadata auditor, plus any external tools recorded in the audit log. Inputs and approvals are listed in section 10.</p>

<h2>6. Vulnerability findings</h2>
${findingsHtml || '<p>No findings recorded.</p>'}

<h2>7. Account security findings</h2>
${b.accountAudits.length ? b.accountAudits.map((a) => `<p><strong>${esc((a as { source: string }).source)}</strong>: ${esc((a as { summary: string }).summary)}</p>`).join('') : '<p>No account audit was part of this assessment.</p>'}

<h2>8. Permission analysis</h2>
${b.rbac.length ? b.rbac.map((r) => `<p><strong>${esc((r as { profile: string }).profile)}</strong>: ${esc((r as { summary: string }).summary)}</p>`).join('') : '<p>No RBAC analysis was part of this assessment.</p>'}

<h2>9. Evidence and risk ratings</h2>
<p>Risk ratings follow OWASP-style severity: Critical, High, Medium, Low, Informational. Confidence reflects how directly the evidence demonstrates the issue. Evidence is redacted for secrets, tokens and password material.</p>

<h2>10. Human approvals</h2>
${b.approvals.length ? `<table class="grid"><tr><th>Level</th><th>Action</th><th>Decision</th><th>When</th></tr>${b.approvals.map((a) => `<tr><td>${esc((a as Record<string, unknown>).level)}</td><td>${esc((a as Record<string, unknown>).title)}</td><td>${esc((a as Record<string, unknown>).decision)}</td><td>${esc((a as Record<string, unknown>).created_at)}</td></tr>`).join('')}</table>` : '<p>No approval checkpoints were required.</p>'}

<h2>11. Recommended remediation</h2>
<ol>
${b.findings
  .filter((x) => ['critical', 'high', 'medium'].includes(x.severity))
  .map((x) => `<li><strong>${esc(x.name)}</strong> (${esc(x.severity)}): ${esc(x.remediation)}</li>`)
  .join('') || '<li>No high-priority remediation identified. Continue routine patching and monitoring.</li>'}
</ol>

<h2>12. Assessment limitations</h2>
<ul>
  <li>Black-box checks cannot prove the absence of vulnerabilities, especially authenticated business-logic flaws.</li>
  <li>No exploitation, fuzzing at scale, denial-of-service or destructive testing was performed.</li>
  <li>Rate limiting and WAF behaviour may differ in production.</li>
  <li>Account/RBAC results depend on the accuracy of the configured connection profile and test accounts.</li>
  <li>External tool coverage depends on which tools are installed.</li>
</ul>

<h2>13. Report metadata</h2>
<p>Generated by JAKE AI at ${esc(b.generatedAt)}. This report contains no plaintext passwords, API keys or session tokens.</p>

<footer>JAKE AI — authorized security assessment. Distribution should be limited to the asset owner and remediation team.</footer>
</body></html>`;
}
