import { useState } from 'react';
import { AlertTriangle, KeyRound, Play } from 'lucide-react';
import { call } from '../lib/api';
import { Badge, Button, Card, Empty, ErrorBox, Spinner } from '../components/ui';
import { ConnectionsPanel } from '../components/Connections';

interface Row {
  name: string;
  method: string;
  path: string;
  viewerAllowed: boolean;
  unauthenticatedStatus: number | null;
  viewerStatus: number | null;
  adminStatus: number | null;
  verdict: 'ok' | 'exposed' | 'viewer-escalation' | 'auth-misconfigured' | 'inconclusive';
  detail: string;
}

interface Result {
  assessmentId: string;
  result: { rows: Row[]; summary: string; notes: string[] };
}

const VERDICT: Record<Row['verdict'], { label: string; cls: string }> = {
  ok: { label: 'OK', cls: 'border-emerald-800 bg-emerald-950/40 text-emerald-300' },
  exposed: { label: 'Exposed', cls: 'border-rose-800 bg-rose-950/40 text-rose-300' },
  'viewer-escalation': { label: 'Privilege escalation', cls: 'border-rose-800 bg-rose-950/40 text-rose-300' },
  'auth-misconfigured': { label: 'Check credentials', cls: 'border-amber-800 bg-amber-950/40 text-amber-300' },
  inconclusive: { label: 'Inconclusive', cls: 'border-slate-700 bg-slate-800/50 text-slate-400' },
};

export default function Rbac() {
  const [connectionId, setConnectionId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<Result | null>(null);

  const run = async () => {
    if (!connectionId) return;
    setBusy(true);
    setError(null);
    try {
      setResult(await call<Result>('rbac:run', { connectionId }));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      <Card title="Connection profile" actions={<span className="text-[10px] text-slate-500">Admin + Viewer test credentials</span>}>
        <ConnectionsPanel kind="rest" selected={connectionId} onSelect={setConnectionId} />
      </Card>

      <Card title="Role-based access control auditor">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="max-w-3xl text-[11.5px] leading-relaxed text-slate-400">
            Compares what an unauthenticated client, a dedicated Viewer account and an Admin account receive for each configured route.
            A Viewer receiving data from a restricted route is reported as a vertical privilege escalation. Hiding a button in the UI does not protect the backend.
            <span className="text-slate-500"> Only GET/HEAD/OPTIONS are sent — write methods are skipped so production data cannot be modified.</span>
          </p>
          <Button variant="primary" disabled={!connectionId} busy={busy} onClick={() => void run()}>
            <Play size={14} /> Test boundaries
          </Button>
        </div>
        <div className="mt-3"><ErrorBox message={error} /></div>
      </Card>

      {busy && <Card><Spinner label="Comparing responses by role…" /></Card>}

      {!result && !busy && (
        <Card>
          <Empty title="No RBAC test yet" hint="Configure a profile with an administrator credential and a dedicated low-privilege test account, plus the routes to compare. Requires level 3 approval." />
        </Card>
      )}

      {result && (
        <>
          <Card title="Access matrix">
            <p className="mb-3 flex items-center gap-2 text-[12px] text-slate-300">
              <KeyRound size={14} className="text-cyan-400" /> {result.result.summary}
            </p>
            <div className="overflow-x-auto">
              <table className="table">
                <thead>
                  <tr>
                    <th>Route</th>
                    <th>Expected Viewer</th>
                    <th>Unauthenticated</th>
                    <th>Viewer</th>
                    <th>Admin</th>
                    <th>Verdict</th>
                    <th>Detail</th>
                  </tr>
                </thead>
                <tbody>
                  {result.result.rows.map((r, i) => (
                    <tr key={i}>
                      <td>
                        <div className="text-[12px] text-slate-200">{r.name}</div>
                        <div className="font-mono text-[10px] text-slate-500">{r.method} {r.path}</div>
                      </td>
                      <td>
                        <Badge tone={r.viewerAllowed ? 'ok' : 'bad'}>{r.viewerAllowed ? 'allowed' : 'denied'}</Badge>
                      </td>
                      <td className="font-mono text-[11px] text-slate-400">{r.unauthenticatedStatus ?? 'n/a'}</td>
                      <td className="font-mono text-[11px] text-slate-400">{r.viewerStatus ?? 'n/a'}</td>
                      <td className="font-mono text-[11px] text-slate-400">{r.adminStatus ?? 'n/a'}</td>
                      <td>
                        <span className={`inline-flex rounded border px-1.5 py-0.5 text-[10px] font-medium ${VERDICT[r.verdict].cls}`}>{VERDICT[r.verdict].label}</span>
                      </td>
                      <td className="max-w-md text-[11px] text-slate-400">{r.detail}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </Card>

          {result.result.notes.length > 0 && (
            <Card title="Notes and skipped routes">
              <ul className="list-disc space-y-1 pl-4 text-[11px] text-slate-400">
                {result.result.notes.map((n, i) => (
                  <li key={i}>{n}</li>
                ))}
              </ul>
            </Card>
          )}

          <Card title="Write operations">
            <div className="flex items-start gap-2 rounded border border-amber-900/50 bg-amber-950/20 px-3 py-2 text-[11px] leading-relaxed text-amber-200/80">
              <AlertTriangle size={14} className="mt-0.5 shrink-0" />
              <span>
                PUT/POST/DELETE/PATCH routes are intentionally never exercised. To verify write authorization safely, point JAKE at a staging environment with disposable test records. Testing write access on production would modify data.
              </span>
            </div>
          </Card>
        </>
      )}
    </div>
  );
}
