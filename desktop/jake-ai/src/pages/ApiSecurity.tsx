import { useState } from 'react';
import { Braces, Play } from 'lucide-react';
import { call } from '../lib/api';
import { Badge, Button, Card, Empty, ErrorBox, Spinner } from '../components/ui';
import { ConnectionsPanel } from '../components/Connections';

interface Row {
  name: string;
  method: string;
  path: string;
  unauthenticatedStatus: number | null;
  authenticatedStatus: number | null;
  issues: string[];
}

interface Result {
  assessmentId: string;
  result: { rows: Row[]; summary: string; notes: string[] };
}

export default function ApiSecurity() {
  const [connectionId, setConnectionId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<Result | null>(null);

  const run = async () => {
    if (!connectionId) return;
    setBusy(true);
    setError(null);
    try {
      setResult(await call<Result>('api:run', { connectionId }));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      <Card title="Connection profile">
        <ConnectionsPanel kind="rest" selected={connectionId} onSelect={setConnectionId} />
      </Card>

      <Card title="API security assessment">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="max-w-3xl text-[11.5px] leading-relaxed text-slate-400">
            Tests configured routes for missing authentication, sensitive response fields, permissive CORS, cacheability, advertised HTTP methods, exposed API documentation and rate limiting (a 15-request sample).
            Response values are redacted before storage or AI analysis.
          </p>
          <Button variant="primary" disabled={!connectionId} busy={busy} onClick={() => void run()}>
            <Play size={14} /> Scan API
          </Button>
        </div>
        <div className="mt-3"><ErrorBox message={error} /></div>
      </Card>

      {busy && <Card><Spinner label="Testing API routes…" /></Card>}

      {!result && !busy && (
        <Card>
          <Empty title="No API scan yet" hint="Add routes to a connection profile (method, path, requiresAuth), then run the assessment. Requires level 3 approval." />
        </Card>
      )}

      {result && (
        <>
          <Card title={result.result.summary}>
            <div className="overflow-x-auto">
              <table className="table">
                <thead>
                  <tr>
                    <th>Endpoint</th>
                    <th>Unauthenticated</th>
                    <th>Authenticated</th>
                    <th>Observations</th>
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
                        <span className={`font-mono text-[11px] ${r.unauthenticatedStatus && r.unauthenticatedStatus < 400 ? 'text-rose-300' : 'text-slate-500'}`}>{r.unauthenticatedStatus ?? 'not tested'}</span>
                      </td>
                      <td className="font-mono text-[11px] text-slate-400">{r.authenticatedStatus ?? 'n/a'}</td>
                      <td>
                        {r.issues.length ? (
                          <ul className="list-disc space-y-0.5 pl-4 text-[10.5px] text-amber-300/80">
                            {r.issues.map((x, j) => <li key={j}>{x}</li>)}
                          </ul>
                        ) : (
                          <Badge tone="ok">no issues detected</Badge>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </Card>
          <Card title="Notes">
            <ul className="list-disc space-y-1 pl-4 text-[11px] text-slate-400">
              {result.result.notes.map((n, i) => <li key={i}>{n}</li>)}
              <li>Rate-limit results depend on WAF/CDN layers and can be false negatives.</li>
            </ul>
          </Card>
        </>
      )}

      <Card title="What is checked">
        <div className="grid grid-cols-2 gap-2 text-[11px] text-slate-400 md:grid-cols-3">
          {['Authentication required', 'Authorization consistency', 'Sensitive data exposure', 'Input validation errors', 'Rate limiting', 'CORS policy', 'Cache headers', 'HTTP methods', 'API documentation exposure', 'Response structure', 'Security headers', 'Error verbosity'].map((x) => (
            <div key={x} className="flex items-center gap-1.5 rounded border border-edge px-2 py-1.5">
              <Braces size={11} className="text-slate-600" /> {x}
            </div>
          ))}
        </div>
      </Card>
    </div>
  );
}
