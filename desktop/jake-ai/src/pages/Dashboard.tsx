import { Activity, ArrowRight, ShieldAlert, Users, KeyRound, ClipboardList } from 'lucide-react';
import { call, fmtDate, statusStyle, useLoad, type AssessmentRow, type AuditRow } from '../lib/api';
import { Card, Empty, ErrorBox, Severity, Spinner } from '../components/ui';
import type { PageKey } from '../App';

interface Stats {
  assessments: number;
  findings: number;
  bySeverity: Record<string, number>;
  accountIssues: number;
  permissionIssues: number;
  recentAssessments: AssessmentRow[];
  recentActivity: AuditRow[];
}

export default function Dashboard({ onNavigate }: { onNavigate: (p: PageKey) => void }) {
  const { data, error, loading } = useLoad(() => call<Stats>('dashboard:stats'), []);
  if (loading) return <Spinner label="Loading dashboard" />;
  if (error) return <ErrorBox message={error} />;
  if (!data) return null;
  const total = Math.max(1, Object.values(data.bySeverity).reduce((a, b) => a + b, 0));

  const kpis = [
    { label: 'Assessments', value: data.assessments, icon: ClipboardList, tone: 'text-cyan-300', page: 'reports' as PageKey },
    { label: 'Findings', value: data.findings, icon: ShieldAlert, tone: 'text-slate-200', page: 'findings' as PageKey },
    { label: 'Critical + High', value: data.bySeverity.critical + data.bySeverity.high, icon: ShieldAlert, tone: 'text-rose-300', page: 'findings' as PageKey },
    { label: 'Account issues', value: data.accountIssues, icon: Users, tone: 'text-amber-300', page: 'accounts' as PageKey },
    { label: 'Permission issues', value: data.permissionIssues, icon: KeyRound, tone: 'text-orange-300', page: 'rbac' as PageKey },
  ];

  return (
    <div className="space-y-4">
      <div className="rounded-lg border border-edge bg-gradient-to-r from-panel to-[#0e1a21] p-5">
        <h2 className="text-lg font-semibold text-slate-100">Hello, I'm JAKE.</h2>
        <p className="mt-1 max-w-3xl text-[13px] leading-relaxed text-slate-400">
          Your AI cybersecurity assistant. I can help assess website security, inspect application accounts, identify vulnerabilities, and improve your system's protection.
          Every assessment must start from an authorized target, and every privileged action is approval-gated and audited.
        </p>
        <div className="mt-3 flex flex-wrap gap-2">
          <button className="btn btn-primary" onClick={() => onNavigate('assessment')}>
            Start a new assessment <ArrowRight size={14} />
          </button>
          <button className="btn" onClick={() => onNavigate('chat')}>
            Ask JAKE
          </button>
          <button className="btn" onClick={() => onNavigate('targets')}>
            Manage targets
          </button>
        </div>
      </div>

      <div className="grid grid-cols-2 gap-3 lg:grid-cols-5">
        {kpis.map((k) => (
          <button key={k.label} className="card-pad text-left transition-colors hover:border-slate-600" onClick={() => onNavigate(k.page)}>
            <div className="flex items-center justify-between">
              <span className="text-[11px] uppercase tracking-wider text-muted">{k.label}</span>
              <k.icon size={15} className={k.tone} />
            </div>
            <div className={`mt-1 text-3xl font-semibold ${k.tone}`}>{k.value}</div>
          </button>
        ))}
      </div>

      <div className="grid grid-cols-1 gap-4 xl:grid-cols-3">
        <Card title="Findings by severity" className="xl:col-span-1">
          <div className="space-y-2.5">
            {(['critical', 'high', 'medium', 'low', 'info'] as const).map((s) => {
              const n = data.bySeverity[s] ?? 0;
              return (
                <div key={s} className="flex items-center gap-2">
                  <Severity value={s} />
                  <div className="h-2 flex-1 overflow-hidden rounded bg-slate-800">
                    <div
                      className={`h-full rounded ${s === 'critical' ? 'bg-rose-700' : s === 'high' ? 'bg-orange-700' : s === 'medium' ? 'bg-amber-600' : s === 'low' ? 'bg-cyan-700' : 'bg-slate-600'}`}
                      style={{ width: `${(n / total) * 100}%` }}
                    />
                  </div>
                  <span className="w-8 text-right text-xs text-slate-400">{n}</span>
                </div>
              );
            })}
          </div>
          <div className="mt-4 border-t border-edge pt-3 text-[11px] text-slate-500">
            Counts come directly from stored findings. Nothing is estimated or simulated.
          </div>
        </Card>

        <Card title="Recent assessments" className="xl:col-span-2" actions={<button className="text-[11px] text-cyan-400 hover:underline" onClick={() => onNavigate('reports')}>View all</button>}>
          {data.recentAssessments.length === 0 ? (
            <Empty title="No assessments yet" hint="Register an authorized target, then run reconnaissance or a safe vulnerability assessment." />
          ) : (
            <table className="table">
              <thead>
                <tr>
                  <th>Target</th>
                  <th>Type</th>
                  <th>Status</th>
                  <th>Started</th>
                  <th>Summary</th>
                </tr>
              </thead>
              <tbody>
                {data.recentAssessments.map((a) => (
                  <tr key={a.id}>
                    <td className="font-medium text-slate-200">{a.target_label}</td>
                    <td className="capitalize text-slate-400">{a.type}</td>
                    <td className={`font-medium capitalize ${statusStyle(a.status)}`}>{a.status}</td>
                    <td className="text-slate-500">{fmtDate(a.started_at)}</td>
                    <td className="max-w-md truncate text-slate-400" title={a.summary ?? ''}>{a.summary ?? a.error ?? '—'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </Card>
      </div>

      <Card title="Recent activity">
        {data.recentActivity.length === 0 ? (
          <Empty title="No activity recorded" hint="Audit entries appear as soon as you register targets or run operations." />
        ) : (
          <div className="space-y-1.5">
            {data.recentActivity.map((a) => (
              <div key={a.id} className="flex items-start gap-3 rounded px-2 py-1.5 text-[12px] hover:bg-slate-800/30">
                <Activity size={13} className="mt-0.5 text-slate-600" />
                <span className="w-40 shrink-0 font-mono text-[11px] text-slate-500">{fmtDate(a.created_at)}</span>
                <span className="w-44 shrink-0 text-slate-300">{a.action}</span>
                <span className="min-w-0 flex-1 truncate text-slate-500" title={a.detail ?? ''}>{a.target ?? ''} {a.detail ?? ''}</span>
                <span className="chip">L{a.level}</span>
              </div>
            ))}
          </div>
        )}
      </Card>
    </div>
  );
}
