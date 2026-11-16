import { useState } from 'react';
import { Play, ShieldCheck, Users } from 'lucide-react';
import { call } from '../lib/api';
import { Badge, Button, Card, Empty, ErrorBox, KeyValue, Severity, Spinner } from '../components/ui';
import { ConnectionsPanel } from '../components/Connections';

interface Issue {
  severity: string;
  name: string;
  detail: string;
  accounts: string[];
  remediation: string;
}

interface Result {
  assessmentId: string;
  result: {
    total: number;
    active: number;
    inactive: number;
    admins: number;
    dormant: number;
    withoutMfa: number | null;
    duplicates: number;
    issues: Issue[];
    policy: Record<string, unknown> | null;
    fieldsDetected: string[];
    notes: string[];
  };
}

export default function Accounts() {
  const [connectionId, setConnectionId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<Result | null>(null);

  const run = async () => {
    if (!connectionId) return;
    setBusy(true);
    setError(null);
    try {
      setResult(await call<Result>('accounts:run', { connectionId }));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      <Card title="Connection profile" actions={<span className="text-[10px] text-slate-500">Credentials encrypted · never sent to AI</span>}>
        <ConnectionsPanel kind="rest" selected={connectionId} onSelect={setConnectionId} />
      </Card>

      <Card title="Account Security Auditor">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="max-w-3xl text-[11.5px] leading-relaxed text-slate-400">
            Read-only inspection of registered accounts through your authorized admin API: dormant and duplicate accounts, privileged accounts without MFA, default-looking names, disabled accounts with recent activity, password policy and lockout configuration, and recent failed logins.
            <span className="text-slate-500"> JAKE never retrieves, displays or exports passwords, hashes or session tokens.</span>
          </div>
          <Button variant="primary" disabled={!connectionId} busy={busy} onClick={() => void run()}>
            <Play size={14} /> Audit accounts
          </Button>
        </div>
        <div className="mt-3"><ErrorBox message={error} /></div>
      </Card>

      {busy && <Card><Spinner label="Reading and analyzing accounts…" /></Card>}

      {!result && !busy && (
        <Card>
          <Empty
            title="No account audit yet"
            hint="Create or select a connection profile above (an admin endpoint that returns users as JSON), then run the audit. Requires level 3 approval."
          />
        </Card>
      )}

      {result && (
        <>
          <div className="grid grid-cols-2 gap-3 md:grid-cols-4 lg:grid-cols-7">
            {[
              ['Accounts', result.result.total],
              ['Active', result.result.active],
              ['Inactive', result.result.inactive],
              ['Admins', result.result.admins],
              ['Dormant', result.result.dormant],
              ['No MFA', result.result.withoutMfa ?? 'n/a'],
              ['Duplicates', result.result.duplicates],
            ].map(([label, value]) => (
              <div key={String(label)} className="card-pad">
                <div className="text-[10px] uppercase tracking-wider text-muted">{label}</div>
                <div className="mt-1 text-2xl font-semibold text-slate-200">{String(value)}</div>
              </div>
            ))}
          </div>

          <div className="grid grid-cols-1 gap-4 xl:grid-cols-3">
            <Card title={`Issue groups (${result.result.issues.length})`} className="xl:col-span-2">
              {result.result.issues.length === 0 ? (
                <div className="flex items-center gap-2 rounded border border-emerald-900/50 bg-emerald-950/20 px-3 py-2 text-xs text-emerald-300">
                  <ShieldCheck size={14} /> No account issues detected from the available fields.
                </div>
              ) : (
                <div className="space-y-2">
                  {result.result.issues.map((issue, i) => (
                    <div key={i} className="rounded border border-edge p-3">
                      <div className="flex flex-wrap items-center gap-2">
                        <Severity value={issue.severity} />
                        <span className="text-[12.5px] font-medium text-slate-200">{issue.name}</span>
                      </div>
                      <p className="mt-1 text-[11.5px] leading-relaxed text-slate-400">{issue.detail}</p>
                      {issue.accounts.length > 0 && (
                        <p className="mt-1 break-words font-mono text-[10.5px] text-amber-300/80">Affected: {issue.accounts.join(', ')}</p>
                      )}
                      <p className="mt-1 text-[11px] text-cyan-300/80">Fix: {issue.remediation}</p>
                    </div>
                  ))}
                </div>
              )}
            </Card>

            <div className="space-y-4">
              <Card title="Detected fields">
                {result.result.fieldsDetected.length ? (
                  <div className="flex flex-wrap gap-1">{result.result.fieldsDetected.map((f) => <Badge key={f}>{f}</Badge>)}</div>
                ) : (
                  <p className="text-xs text-slate-500">No standard account fields detected.</p>
                )}
                <p className="mt-2 text-[10.5px] text-slate-600">Field detection is automatic; configure the correct JSON path if a field is missing.</p>
              </Card>
              <Card title="Policy">
                {result.result.policy ? <KeyValue items={Object.entries(result.result.policy).map(([k, v]) => [k, v === null ? 'not set' : String(v)])} /> : <p className="text-xs text-slate-500">No policy endpoint configured. Add one in the connection profile to verify password and lockout settings.</p>}
              </Card>
              {result.result.notes.length > 0 && (
                <Card title="Notes">
                  <ul className="list-disc space-y-1 pl-4 text-[10.5px] text-slate-500">
                    {result.result.notes.map((n, i) => <li key={i}>{n}</li>)}
                  </ul>
                </Card>
              )}
              <Card title="Assessment">
                <p className="flex items-center gap-2 text-[11px] text-slate-400"><Users size={12} /> {result.assessmentId}</p>
                <p className="mt-1 text-[10.5px] text-slate-600">Stored in SQLite and included in reports.</p>
              </Card>
            </div>
          </div>
        </>
      )}
    </div>
  );
}
