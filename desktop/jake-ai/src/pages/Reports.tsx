import { useState } from 'react';
import { FileJson, FileText, FolderOpen } from 'lucide-react';
import { call, fmtDate, useLoad, type AssessmentRow, type ReportRow } from '../lib/api';
import { Button, Card, Empty, ErrorBox, Spinner } from '../components/ui';

export default function ReportsPage() {
  const assessments = useLoad(() => call<AssessmentRow[]>('assessments:list'), []);
  const reports = useLoad(() => call<ReportRow[]>('reports:list'), []);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [last, setLast] = useState<{ path: string; format: string } | null>(null);

  const generate = async (assessmentId: string, format: 'html' | 'pdf' | 'json') => {
    setBusy(`${assessmentId}:${format}`);
    setError(null);
    try {
      const res = await call<{ path: string }>('reports:generate', { assessmentId, format });
      setLast({ path: res.path, format });
      reports.reload();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  const open = async (path: string) => {
    try {
      await call('reports:open', { path });
    } catch (e) {
      setError((e as Error).message);
    }
  };

  return (
    <div className="space-y-4">
      <ErrorBox message={error} />
      <Card title="Assessment reports">
        {(assessments.data ?? []).length === 0 ? (
          <Empty title="No assessments to report on" hint="Run reconnaissance, a vulnerability scan, or an account audit first." />
        ) : (
          <div className="overflow-x-auto">
            <table className="table">
              <thead>
                <tr>
                  <th>Target</th>
                  <th>Type</th>
                  <th>Status</th>
                  <th>Started</th>
                  <th>Summary</th>
                  <th className="text-right">Export</th>
                </tr>
              </thead>
              <tbody>
                {(assessments.data ?? []).map((a) => (
                  <tr key={a.id}>
                    <td className="font-medium text-slate-200">{a.target_label}</td>
                    <td className="capitalize text-slate-400">{a.type}</td>
                    <td className="capitalize text-slate-400">{a.status}</td>
                    <td className="text-[11px] text-slate-500">{fmtDate(a.started_at)}</td>
                    <td className="max-w-xs truncate text-[11px] text-slate-500" title={a.summary ?? ''}>{a.summary ?? a.error ?? '—'}</td>
                    <td>
                      <div className="flex justify-end gap-1">
                        <Button className="btn-sm" busy={busy === `${a.id}:html`} onClick={() => void generate(a.id, 'html')}>
                          <FileText size={11} /> HTML
                        </Button>
                        <Button className="btn-sm" busy={busy === `${a.id}:pdf`} onClick={() => void generate(a.id, 'pdf')}>
                          PDF
                        </Button>
                        <Button className="btn-sm" busy={busy === `${a.id}:json`} onClick={() => void generate(a.id, 'json')}>
                          <FileJson size={11} /> JSON
                        </Button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        {last && (
          <div className="mt-3 flex items-center justify-between rounded border border-emerald-900/60 bg-emerald-950/20 px-3 py-2 text-[11px]">
            <span className="break-all text-emerald-200">{last.format.toUpperCase()} written: {last.path}</span>
            <Button className="btn-sm" onClick={() => void open(last.path)}>
              <FolderOpen size={12} /> Open
            </Button>
          </div>
        )}
      </Card>

      <Card title={`Generated reports (${reports.data?.length ?? 0})`}>
        {reports.loading ? (
          <Spinner />
        ) : (reports.data ?? []).length === 0 ? (
          <Empty title="No reports yet" hint="Export a completed assessment above. Reports contain no plaintext passwords, keys or session tokens." />
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Format</th>
                <th>File</th>
                <th>Created</th>
                <th className="text-right">Action</th>
              </tr>
            </thead>
            <tbody>
              {(reports.data ?? []).map((r) => (
                <tr key={r.id}>
                  <td className="uppercase text-slate-300">{r.format}</td>
                  <td className="max-w-2xl break-all font-mono text-[10.5px] text-slate-400">{r.path}</td>
                  <td className="text-[11px] text-slate-500">{fmtDate(r.created_at)}</td>
                  <td className="text-right">
                    <Button className="btn-sm" onClick={() => void open(r.path)}>
                      <FolderOpen size={12} /> Open
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <p className="mt-3 text-[10.5px] text-slate-600">
          Reports include the executive summary, target and scope, methodology, findings with evidence, account/RBAC analysis, approvals, remediation and limitations. Secrets and password material are redacted.
        </p>
      </Card>
    </div>
  );
}
