import { useState } from 'react';
import { CheckCircle2, FileText, Play, ShieldAlert } from 'lucide-react';
import { call, useLoad, type AssessmentRow, type ConnectionRow, type TargetRow } from '../lib/api';
import { Badge, Button, Card, Empty, ErrorBox, Field, Progress, Select } from '../components/ui';

type ModuleKey = 'recon' | 'vulnerability' | 'accounts' | 'rbac' | 'api' | 'php' | 'mysql';

const MODULES: Array<{ key: ModuleKey; label: string; level: number; description: string; needs: 'target' | 'connection' | 'directory' }> = [
  { key: 'recon', label: 'Passive reconnaissance', level: 1, description: 'DNS, TLS, headers, technologies, robots, sitemap, public endpoints, CT subdomains.', needs: 'target' },
  { key: 'vulnerability', label: 'Safe vulnerability assessment', level: 2, description: 'OWASP-aligned non-destructive checks: headers, exposure, CORS, CSRF, input probes.', needs: 'target' },
  { key: 'accounts', label: 'Account security audit', level: 3, description: 'Dormant/duplicate/privileged accounts, MFA, lockout and policy review via authorized API.', needs: 'connection' },
  { key: 'rbac', label: 'RBAC access-control test', level: 3, description: 'Compare unauthenticated, Viewer and Admin access on configured safe routes.', needs: 'connection' },
  { key: 'api', label: 'API security assessment', level: 3, description: 'Authentication, data exposure, CORS, caching, methods, rate limiting, docs exposure.', needs: 'connection' },
  { key: 'php', label: 'PHP source review', level: 2, description: 'Read-only static analysis of SQLi, XSS, hashing, uploads, CSRF, secrets.', needs: 'directory' },
  { key: 'mysql', label: 'MySQL / MariaDB audit', level: 3, description: 'Read-only privileges, hash algorithm, schema and server hardening review.', needs: 'connection' },
];

interface StepResult {
  module: ModuleKey;
  assessmentId?: string;
  status: 'pending' | 'running' | 'done' | 'failed' | 'skipped';
  message: string;
}

export default function NewAssessment({ onNavigate }: { onNavigate: (p: 'reports' | 'findings' | 'targets') => void }) {
  const targets = useLoad(() => call<TargetRow[]>('targets:list'), []);
  const connections = useLoad(() => call<ConnectionRow[]>('connections:list'), []);
  const [targetId, setTargetId] = useState('');
  const [selected, setSelected] = useState<ModuleKey[]>(['recon', 'vulnerability']);
  const [connectionFor, setConnectionFor] = useState<Record<string, string>>({});
  const [phpDir, setPhpDir] = useState('');
  const [steps, setSteps] = useState<StepResult[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastReport, setLastReport] = useState<{ path: string; format: string } | null>(null);
  const [assessments, setAssessments] = useState<AssessmentRow[]>([]);

  const toggle = (key: ModuleKey) => setSelected((s) => (s.includes(key) ? s.filter((x) => x !== key) : [...s, key]));

  const restProfiles = (connections.data ?? []).filter((c) => c.kind !== 'mysql');
  const mysqlProfiles = (connections.data ?? []).filter((c) => c.kind === 'mysql');

  const run = async () => {
    if (!targetId) {
      setError('Select an authorized target first.');
      return;
    }
    setBusy(true);
    setError(null);
    setLastReport(null);
    setAssessments([]);
    setSteps(selected.map((m) => ({ module: m, status: 'pending', message: 'Waiting' })));
    const done: AssessmentRow[] = [];
    for (const mod of MODULES.filter((m) => selected.includes(m.key))) {
      const update = (patch: Partial<StepResult>) =>
        setSteps((s) => s.map((x) => (x.module === mod.key ? { ...x, ...patch } : x)));
      update({ status: 'running', message: 'Running…' });
      try {
        let res: Record<string, unknown>;
        if (mod.needs === 'target') {
          res = await call(mod.key === 'recon' ? 'recon:run' : 'vuln:run', { targetId });
        } else if (mod.needs === 'directory') {
          if (!phpDir.trim()) throw new Error('Enter the PHP project directory.');
          res = await call('php:scan', { directory: phpDir.trim() });
        } else {
          const cid = connectionFor[mod.key];
          if (!cid) throw new Error('Select a connection profile for this module.');
          const channel =
            mod.key === 'mysql' ? 'mysql:audit' : mod.key === 'accounts' ? 'accounts:run' : mod.key === 'rbac' ? 'rbac:run' : 'api:run';
          res = await call(channel, { connectionId: cid });
        }
        const assessmentId = res.assessmentId ? String(res.assessmentId) : (res as { result?: { assessmentId?: string } }).result?.assessmentId;
        update({ status: 'done', message: 'Completed', assessmentId });
        if (assessmentId) {
          const a = await call<{ assessment: AssessmentRow }>('assessments:get', { id: assessmentId });
          done.push(a.assessment);
        }
      } catch (e) {
        const message = (e as Error).message;
        update({ status: message.toLowerCase().includes('denied') ? 'skipped' : 'failed', message });
      }
      setAssessments([...done]);
    }
    setBusy(false);
  };

  const generate = async (assessmentId: string, format: 'html' | 'pdf' | 'json') => {
    try {
      const res = await call<{ path: string }>('reports:generate', { assessmentId, format });
      setLastReport({ path: res.path, format });
    } catch (e) {
      setError((e as Error).message);
    }
  };

  return (
    <div className="grid grid-cols-1 gap-4 xl:grid-cols-3">
      <div className="space-y-4 xl:col-span-2">
        <Card title="Assessment plan">
          <div className="space-y-3">
            <ErrorBox message={error} />
            <Field label="Authorized target">
              <Select value={targetId} onChange={setTargetId}>
                <option value="">Select a registered target…</option>
                {(targets.data ?? []).map((t) => (
                  <option key={t.id} value={t.id}>
                    {t.label} — {t.url}
                  </option>
                ))}
              </Select>
            </Field>
            {(targets.data ?? []).length === 0 && <Empty title="No authorized targets" hint="Register a target in Target Management before starting an assessment." />}

            <div className="space-y-1.5">
              {MODULES.map((m) => (
                <div key={m.key} className={`rounded-md border px-3 py-2 ${selected.includes(m.key) ? 'border-cyan-900 bg-cyan-950/10' : 'border-edge'}`}>
                  <label className="flex cursor-pointer items-start gap-2">
                    <input type="checkbox" className="mt-1" checked={selected.includes(m.key)} onChange={() => toggle(m.key)} />
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2">
                        <span className="text-[12.5px] font-medium text-slate-200">{m.label}</span>
                        <Badge tone={m.level >= 3 ? 'warn' : 'default'}>Level {m.level}</Badge>
                      </div>
                      <p className="text-[11px] text-slate-500">{m.description}</p>
                    </div>
                  </label>
                  {selected.includes(m.key) && m.needs === 'connection' && (
                    <div className="mt-2 pl-6">
                      <Select value={connectionFor[m.key] ?? ''} onChange={(v) => setConnectionFor({ ...connectionFor, [m.key]: v })}>
                        <option value="">Select connection profile…</option>
                        {(m.key === 'mysql' ? mysqlProfiles : restProfiles).map((c) => (
                          <option key={c.id} value={c.id}>
                            {c.name} {c.base_url ? `— ${c.base_url}` : ''}
                          </option>
                        ))}
                      </Select>
                      {(m.key === 'mysql' ? mysqlProfiles : restProfiles).length === 0 && (
                        <p className="mt-1 text-[10.5px] text-amber-500">No profile yet. Create one on the {m.key === 'mysql' ? 'PHP / MySQL' : m.key === 'accounts' ? 'Account Security' : m.key === 'rbac' ? 'RBAC' : 'API Security'} page.</p>
                      )}
                    </div>
                  )}
                  {selected.includes(m.key) && m.needs === 'directory' && (
                    <div className="mt-2 pl-6">
                      <input className="input font-mono text-[11px]" value={phpDir} onChange={(e) => setPhpDir(e.target.value)} placeholder="C:\xampp\htdocs\myapp" />
                    </div>
                  )}
                </div>
              ))}
            </div>

            <div className="flex items-center justify-between pt-1">
              <p className="text-[11px] text-slate-500">Level 2+ modules pause for explicit approval. Level 3 uses saved test credentials.</p>
              <Button variant="primary" busy={busy} disabled={!targetId || selected.length === 0} onClick={() => void run()}>
                <Play size={14} /> Run {selected.length} module{selected.length === 1 ? '' : 's'}
              </Button>
            </div>
          </div>
        </Card>

        {steps.length > 0 && (
          <Card title="Execution">
            <div className="space-y-2">
              {steps.map((s) => (
                <div key={s.module} className="flex items-start gap-2 rounded border border-edge px-3 py-2">
                  {s.status === 'done' ? <CheckCircle2 size={14} className="mt-0.5 text-emerald-400" /> : s.status === 'running' ? <ShieldAlert size={14} className="mt-0.5 animate-pulse text-cyan-400" /> : <span className="mt-1 h-3 w-3 rounded-full border border-slate-600" />}
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="text-[12.5px] capitalize text-slate-200">{s.module}</span>
                      <span className={`text-[10px] uppercase ${s.status === 'done' ? 'text-emerald-400' : s.status === 'failed' ? 'text-rose-400' : s.status === 'skipped' ? 'text-amber-400' : 'text-slate-500'}`}>{s.status}</span>
                    </div>
                    <p className="break-words text-[11px] text-slate-500">{s.message}</p>
                  </div>
                  {s.assessmentId && (
                    <div className="flex gap-1">
                      {(['html', 'pdf', 'json'] as const).map((f) => (
                        <Button key={f} className="btn-sm" onClick={() => void generate(s.assessmentId!, f)}>
                          <FileText size={11} /> {f}
                        </Button>
                      ))}
                    </div>
                  )}
                </div>
              ))}
            </div>
          </Card>
        )}
      </div>

      <div className="space-y-4">
        <Card title="Completed in this run">
          {assessments.length === 0 ? (
            <Empty title="Nothing yet" hint="Assessment results appear here with report shortcuts." />
          ) : (
            <div className="space-y-2">
              {assessments.map((a) => (
                <div key={a.id} className="rounded border border-edge px-2.5 py-2">
                  <div className="flex items-center justify-between">
                    <span className="text-[12px] capitalize text-slate-200">{a.type}</span>
                    <span className={a.status === 'complete' ? 'text-[10px] text-emerald-400' : 'text-[10px] text-rose-400'}>{a.status}</span>
                  </div>
                  <p className="text-[10.5px] text-slate-500">{a.summary ?? a.error ?? '—'}</p>
                </div>
              ))}
            </div>
          )}
        </Card>
        {lastReport && (
          <Card title="Report generated">
            <p className="break-all text-[11px] text-slate-400">{lastReport.path}</p>
            <div className="mt-2 flex gap-2">
              <Button className="btn-sm" onClick={() => void call('reports:open', { path: lastReport.path }).catch((e) => setError((e as Error).message))}>Open</Button>
              <Button className="btn-sm" onClick={() => onNavigate('reports')}>All reports</Button>
            </div>
          </Card>
        )}
        <Card title="How this runs">
          <ol className="list-decimal space-y-1.5 pl-4 text-[11.5px] leading-relaxed text-slate-400">
            <li>Each module creates a recorded assessment.</li>
            <li>Approvals are requested before active or authenticated testing.</li>
            <li>Requests stay inside the target allowlist and rate limit.</li>
            <li>Findings and evidence are stored in SQLite.</li>
            <li>Reports can be exported as HTML, PDF or JSON.</li>
          </ol>
        </Card>
      </div>
    </div>
  );
}
