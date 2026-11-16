import { useState } from 'react';
import { Search } from 'lucide-react';
import { call, fmtDate, useLoad, type AuditRow } from '../lib/api';
import { Badge, Card, Empty, ErrorBox, Spinner } from '../components/ui';

export default function AuditLogPage() {
  const logs = useLoad(() => call<AuditRow[]>('audit:list', { limit: 500 }), []);
  const [search, setSearch] = useState('');
  const rows = (logs.data ?? []).filter((r) => !search || `${r.action} ${r.target ?? ''} ${r.detail ?? ''} ${r.actor}`.toLowerCase().includes(search.toLowerCase()));

  return (
    <div className="space-y-4">
      <Card
        title={`Audit log (${rows.length})`}
        actions={
          <div className="relative w-72">
            <Search size={13} className="absolute left-2 top-2 text-slate-500" />
            <input className="input h-7 pl-7 text-[11px]" placeholder="Filter actions, targets, details…" value={search} onChange={(e) => setSearch(e.target.value)} />
          </div>
        }
      >
        <ErrorBox message={logs.error} />
        {logs.loading ? (
          <Spinner />
        ) : rows.length === 0 ? (
          <Empty title="No audit entries" hint="Target registrations, scans, approvals, tool runs, patches and settings changes are recorded here." />
        ) : (
          <div className="max-h-[calc(100vh-220px)] overflow-auto">
            <table className="table">
              <thead className="sticky top-0 bg-panel">
                <tr>
                  <th>Time</th>
                  <th>Actor</th>
                  <th>Action</th>
                  <th>Level</th>
                  <th>Target</th>
                  <th>Detail</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((r) => (
                  <tr key={r.id}>
                    <td className="whitespace-nowrap font-mono text-[10.5px] text-slate-500">{fmtDate(r.created_at)}</td>
                    <td className="text-[11px] text-slate-400">{r.actor}</td>
                    <td className="whitespace-nowrap text-[11.5px] text-slate-200">{r.action}</td>
                    <td>
                      <Badge tone={r.level >= 4 ? 'bad' : r.level >= 3 ? 'warn' : 'default'}>L{r.level}</Badge>
                    </td>
                    <td className="max-w-[220px] break-all font-mono text-[10.5px] text-slate-500">{r.target ?? '—'}</td>
                    <td className="max-w-xl break-words text-[10.5px] text-slate-500">{r.detail ?? '—'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        <p className="mt-3 text-[10.5px] text-slate-600">Log entries are stored locally in SQLite and redact secrets, tokens and credential material.</p>
      </Card>
    </div>
  );
}
