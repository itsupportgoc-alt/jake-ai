import { useState } from 'react';
import { Crosshair, Play, Radar, Trash2 } from 'lucide-react';
import { call, fmtDate, useLoad, type TargetRow } from '../lib/api';
import { Badge, Button, Card, Empty, ErrorBox, Field, Select } from '../components/ui';
import type { PageKey } from '../App';

export default function Targets({ onNavigate }: { onNavigate: (p: PageKey) => void }) {
  const targets = useLoad(() => call<TargetRow[]>('targets:list'), []);
  const [form, setForm] = useState({ label: '', url: '', kind: 'web', authorizationNote: '', scopePatterns: '', rateLimit: '2', allowPrivate: false });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [running, setRunning] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      await call('targets:create', {
        label: form.label,
        url: form.url,
        kind: form.kind,
        authorizationNote: form.authorizationNote,
        scopePatterns: form.scopePatterns.split(/[\n,]+/).map((s) => s.trim()).filter(Boolean),
        rateLimit: Number(form.rateLimit),
        allowPrivate: form.allowPrivate,
      });
      setForm({ label: '', url: '', kind: 'web', authorizationNote: '', scopePatterns: '', rateLimit: '2', allowPrivate: false });
      targets.reload();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const remove = async (t: TargetRow) => {
    if (!window.confirm(`Remove "${t.label}" and its authorization record? Existing assessment history is kept.`)) return;
    try {
      await call('targets:delete', { id: t.id });
      targets.reload();
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const run = async (t: TargetRow, kind: 'recon' | 'vuln') => {
    setRunning(`${t.id}:${kind}`);
    setResult(null);
    try {
      const res = await call<Record<string, unknown>>(kind === 'recon' ? 'recon:run' : 'vuln:run', { targetId: t.id });
      setResult(`${kind === 'recon' ? 'Reconnaissance' : 'Vulnerability assessment'} finished for ${t.label}.\n${JSON.stringify(res, null, 2).slice(0, 4000)}`);
    } catch (e) {
      setResult(`${kind === 'recon' ? 'Reconnaissance' : 'Assessment'} failed: ${(e as Error).message}`);
    } finally {
      setRunning(null);
    }
  };

  return (
    <div className="grid grid-cols-1 gap-4 xl:grid-cols-3">
      <Card title="Register an authorized target" className="xl:col-span-1">
        <div className="space-y-3">
          <p className="rounded border border-amber-900/50 bg-amber-950/20 px-2.5 py-2 text-[11px] leading-relaxed text-amber-200/80">
            Only register systems you own or have written permission to test. The authorization note is stored with the target and included in reports.
          </p>
          <ErrorBox message={error} />
          <Field label="Target name">
            <input className="input" value={form.label} onChange={(e) => setForm({ ...form, label: e.target.value })} placeholder="Staging storefront" />
          </Field>
          <Field label="URL" hint="Include the scheme, e.g. https://staging.example.com">
            <input className="input" value={form.url} onChange={(e) => setForm({ ...form, url: e.target.value })} placeholder="https://staging.example.com" />
          </Field>
          <Field label="Target kind">
            <Select value={form.kind} onChange={(v) => setForm({ ...form, kind: v })}>
              <option value="web">Website</option>
              <option value="api">API</option>
              <option value="php">PHP application</option>
              <option value="mysql">Database-backed app</option>
              <option value="localhost">Local / development</option>
            </Select>
          </Field>
          <Field label="Authorization basis" hint="Who owns the system, and why you may test it.">
            <textarea className="input h-20" value={form.authorizationNote} onChange={(e) => setForm({ ...form, authorizationNote: e.target.value })} placeholder="I own this staging environment and am authorized to assess it." />
          </Field>
          <Field label="Additional scope hosts" hint="Optional. One per line. Wildcards like *.example.com are allowed.">
            <textarea className="input h-16 font-mono text-[11px]" value={form.scopePatterns} onChange={(e) => setForm({ ...form, scopePatterns: e.target.value })} placeholder="api.example.com" />
          </Field>
          <div className="grid grid-cols-2 gap-3">
            <Field label="Rate limit (req/s)">
              <input className="input" type="number" min={0.2} max={20} step={0.2} value={form.rateLimit} onChange={(e) => setForm({ ...form, rateLimit: e.target.value })} />
            </Field>
            <label className="flex items-end gap-2 pb-1.5 text-[12px] text-slate-300">
              <input type="checkbox" checked={form.allowPrivate} onChange={(e) => setForm({ ...form, allowPrivate: e.target.checked })} />
              Allow private/local network
            </label>
          </div>
          <Button variant="primary" busy={busy} onClick={() => void create()} className="w-full">
            <Crosshair size={14} /> Register target
          </Button>
        </div>
      </Card>

      <div className="space-y-4 xl:col-span-2">
        <Card title={`Authorized targets (${targets.data?.length ?? 0})`}>
          <ErrorBox message={targets.error} />
          {(targets.data ?? []).length === 0 ? (
            <Empty title="No targets registered" hint="Register a system on the left before running reconnaissance or assessments." />
          ) : (
            <div className="overflow-x-auto">
              <table className="table">
                <thead>
                  <tr>
                    <th>Target</th>
                    <th>Scope</th>
                    <th>Limits</th>
                    <th>Registered</th>
                    <th className="text-right">Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {(targets.data ?? []).map((t) => {
                    let extra: string[] = [];
                    try {
                      extra = JSON.parse(t.scope_patterns || '[]');
                    } catch {
                      extra = [];
                    }
                    return (
                      <tr key={t.id}>
                        <td>
                          <div className="font-medium text-slate-200">{t.label}</div>
                          <div className="text-[11px] text-slate-500">{t.url}</div>
                          <div className="mt-1 max-w-md text-[10.5px] italic text-slate-600" title={t.authorization_note}>
                            “{t.authorization_note.slice(0, 140)}{t.authorization_note.length > 140 ? '…' : ''}”
                          </div>
                        </td>
                        <td>
                          <div className="flex flex-wrap gap-1">
                            <Badge>{t.kind}</Badge>
                            {extra.map((x) => (
                              <Badge key={x}>{x}</Badge>
                            ))}
                            {t.allow_private ? <Badge tone="warn">private allowed</Badge> : null}
                          </div>
                        </td>
                        <td className="text-[11px] text-slate-400">{t.rate_limit}/s</td>
                        <td className="text-[11px] text-slate-500">{fmtDate(t.created_at)}</td>
                        <td>
                          <div className="flex justify-end gap-1">
                            <Button className="btn-sm" busy={running === `${t.id}:recon`} onClick={() => void run(t, 'recon')}>
                              <Radar size={12} /> Recon
                            </Button>
                            <Button className="btn-sm" busy={running === `${t.id}:vuln`} onClick={() => void run(t, 'vuln')}>
                              <Play size={12} /> Scan
                            </Button>
                            <Button className="btn-sm btn-danger" onClick={() => void remove(t)}>
                              <Trash2 size={12} />
                            </Button>
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
          <div className="mt-3 flex gap-2">
            <Button className="btn-sm" onClick={() => onNavigate('recon')}>Open reconnaissance</Button>
            <Button className="btn-sm" onClick={() => onNavigate('assessment')}>New assessment</Button>
          </div>
        </Card>

        {result && (
          <Card title="Last run">
            <pre className="code max-h-80 whitespace-pre-wrap">{result}</pre>
          </Card>
        )}
      </div>
    </div>
  );
}
