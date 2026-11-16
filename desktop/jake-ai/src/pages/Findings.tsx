import { useState } from 'react';
import { Download, Search } from 'lucide-react';
import { call, useLoad, type FindingRow } from '../lib/api';
import { Button, Card, Empty, ErrorBox, Select, Spinner } from '../components/ui';
import { FindingsView } from '../components/FindingsView';

export default function FindingsPage() {
  const [severity, setSeverity] = useState('');
  const [status, setStatus] = useState('');
  const [search, setSearch] = useState('');
  const [query, setQuery] = useState('');
  const findings = useLoad(() => call<FindingRow[]>('findings:list', { severity: severity || undefined, status: status || undefined, search: query || undefined }), [severity, status, query]);

  const update = async (id: string, s: string) => {
    await call('findings:update', { id, status: s });
    findings.reload();
  };

  const exportJson = async () => {
    const blob = new Blob([JSON.stringify(findings.data ?? [], null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `jake-findings-${new Date().toISOString().slice(0, 10)}.json`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const counts = (findings.data ?? []).reduce<Record<string, number>>((acc, f) => ({ ...acc, [f.severity]: (acc[f.severity] ?? 0) + 1 }), {});

  return (
    <div className="space-y-4">
      <Card title="Findings">
        <div className="mb-3 flex flex-wrap items-end gap-2">
          <div className="w-40">
            <label className="label">Severity</label>
            <Select value={severity} onChange={setSeverity}>
              <option value="">All severities</option>
              {['critical', 'high', 'medium', 'low', 'info'].map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </Select>
          </div>
          <div className="w-40">
            <label className="label">Status</label>
            <Select value={status} onChange={setStatus}>
              <option value="">All statuses</option>
              {['open', 'confirmed', 'fixed', 'accepted', 'false_positive'].map((s) => (
                <option key={s} value={s}>
                  {s.replace('_', ' ')}
                </option>
              ))}
            </Select>
          </div>
          <div className="min-w-[220px] flex-1">
            <label className="label">Search</label>
            <div className="relative">
              <Search size={13} className="absolute left-2 top-2.5 text-slate-500" />
              <input
                className="input pl-7"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && setQuery(search)}
                placeholder="Name, endpoint, target, category…"
              />
            </div>
          </div>
          <Button onClick={() => setQuery(search)}>Filter</Button>
          <Button onClick={() => void exportJson()}>
            <Download size={13} /> Export JSON
          </Button>
        </div>
        <div className="mb-2 flex flex-wrap gap-3 text-[11px] text-slate-500">
          <span>{(findings.data ?? []).length} shown</span>
          {(['critical', 'high', 'medium', 'low', 'info'] as const).map((s) => (
            <span key={s}>
              {s}: <span className="text-slate-300">{counts[s] ?? 0}</span>
            </span>
          ))}
        </div>
        <ErrorBox message={findings.error} />
        {findings.loading ? <Spinner /> : (findings.data ?? []).length === 0 ? <Empty title="No findings match" hint="Run an assessment or change the filters." /> : <FindingsView findings={findings.data ?? []} onUpdate={(id, s) => void update(id, s)} />}
      </Card>
    </div>
  );
}
