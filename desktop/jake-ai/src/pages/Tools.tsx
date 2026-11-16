import { useState } from 'react';
import { Play, Terminal, Wrench } from 'lucide-react';
import { call, fmtDate, useLoad, type TargetRow, type ToolRow } from '../lib/api';
import { Badge, Button, Card, Empty, ErrorBox, Field, Select, Spinner } from '../components/ui';

interface RunResult {
  assessmentId: string;
  result: { command: string; status: string; exitCode: number | null; output: string };
}

interface ToolRunRow {
  id: string;
  tool: string;
  target: string;
  args: string;
  status: string;
  exit_code: number | null;
  output: string;
  started_at: string;
  finished_at: string | null;
}

interface WslStatus {
  installed: boolean;
  distributions: string[];
  ubuntu: boolean;
  notes: string;
}

export default function ToolsPage() {
  const tools = useLoad(() => call<ToolRow[]>('tools:list'), []);
  const wsl = useLoad(() => call<WslStatus>('wsl:status'), []);
  const targets = useLoad(() => call<TargetRow[]>('targets:list'), []);
  const history = useLoad(() => call<ToolRunRow[]>('tools:history'), []);
  const [tool, setTool] = useState<'nmap' | 'nikto' | 'zap'>('nmap');
  const [targetId, setTargetId] = useState('');
  const [topPorts, setTopPorts] = useState('100');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<RunResult | null>(null);
  const [openRun, setOpenRun] = useState<string | null>(null);

  const run = async () => {
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      setResult(await call<RunResult>('tools:run', { tool, targetId, topPorts: Number(topPorts) }));
      history.reload();
      tools.reload();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 gap-4 xl:grid-cols-3">
        <Card title="Installed security tools" className="xl:col-span-2">
          {tools.loading ? (
            <Spinner />
          ) : (
            <table className="table">
              <thead>
                <tr>
                  <th>Tool</th>
                  <th>Status</th>
                  <th>Version</th>
                  <th>Source</th>
                  <th>Notes</th>
                </tr>
              </thead>
              <tbody>
                {(tools.data ?? []).map((t) => (
                  <tr key={t.id}>
                    <td className="font-medium text-slate-200">{t.name}</td>
                    <td>{t.installed ? <Badge tone="ok">installed</Badge> : <Badge tone="warn">not installed</Badge>}</td>
                    <td className="font-mono text-[10.5px] text-slate-400">{t.version ?? '—'}</td>
                    <td className="text-[11px] text-slate-500">{t.via === 'builtin' ? 'built-in' : t.via}</td>
                    <td className="max-w-md text-[10.5px] text-slate-500">{t.notes}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </Card>

        <Card title="WSL / Ubuntu">
          {wsl.loading ? (
            <Spinner />
          ) : (
            <div className="space-y-2 text-[11.5px]">
              <div className="flex items-center justify-between">
                <span className="text-slate-400">WSL installed</span>
                <Badge tone={wsl.data?.installed ? 'ok' : 'warn'}>{wsl.data?.installed ? 'yes' : 'no'}</Badge>
              </div>
              <div className="flex items-center justify-between">
                <span className="text-slate-400">Ubuntu available</span>
                <Badge tone={wsl.data?.ubuntu ? 'ok' : 'warn'}>{wsl.data?.ubuntu ? 'yes' : 'no'}</Badge>
              </div>
              <div>
                <span className="text-slate-500">Distributions:</span> <span className="text-slate-300">{wsl.data?.distributions.join(', ') || 'none'}</span>
              </div>
              <p className="text-[10.5px] leading-relaxed text-slate-500">{wsl.data?.notes}</p>
              {!wsl.data?.installed && (
                <pre className="code whitespace-pre-wrap text-[10px]">{`wsl --install -d Ubuntu-24.04
# then inside Ubuntu:
sudo apt update && sudo apt install -y nmap nikto`}</pre>
              )}
              <p className="text-[10px] text-slate-600">JAKE works without WSL; external Linux-only tools are optional.</p>
            </div>
          )}
        </Card>
      </div>

      <Card title="Run an authorized tool">
        <div className="grid grid-cols-1 gap-3 md:grid-cols-4">
          <Field label="Tool">
            <Select value={tool} onChange={(v) => setTool(v as 'nmap' | 'nikto' | 'zap')}>
              <option value="nmap">Nmap (top ports, version light)</option>
              <option value="nikto">Nikto (web server checks)</option>
              <option value="zap">OWASP ZAP baseline</option>
            </Select>
          </Field>
          <Field label="Authorized target">
            <Select value={targetId} onChange={setTargetId}>
              <option value="">Select target…</option>
              {(targets.data ?? []).map((t) => (
                <option key={t.id} value={t.id}>
                  {t.label}
                </option>
              ))}
            </Select>
          </Field>
          <Field label="Nmap top ports" hint="10–1000. Ignored by other tools.">
            <input className="input" type="number" min={10} max={1000} value={topPorts} onChange={(e) => setTopPorts(e.target.value)} />
          </Field>
          <div className="flex items-end">
            <Button variant="primary" disabled={!targetId} busy={busy} onClick={() => void run()}>
              <Play size={14} /> Run tool
            </Button>
          </div>
        </div>
        <p className="mt-2 text-[11px] text-slate-500">
          Arguments are fixed by JAKE and validated; arbitrary shell commands are impossible. Each run requires level 2 approval, a registered target, a timeout, and is recorded in the audit log.
        </p>
        <div className="mt-3"><ErrorBox message={error} /></div>
      </Card>

      {busy && <Card><Spinner label="Tool running…" /></Card>}

      {result && (
        <Card title={`${result.result.command} — ${result.result.status} (exit ${result.result.exitCode ?? 'n/a'})`} actions={<Badge>{result.assessmentId}</Badge>}>
          <pre className="code max-h-96 whitespace-pre-wrap">{result.result.output || '(no output)'}</pre>
        </Card>
      )}

      <Card title="Execution history">
        {history.loading ? (
          <Spinner />
        ) : (history.data ?? []).length === 0 ? (
          <Empty title="No tool runs yet" hint="External tool executions and their redacted output are recorded here." />
        ) : (
          <div className="space-y-1.5">
            {(history.data ?? []).map((r) => (
              <div key={r.id} className="rounded border border-edge">
                <button className="flex w-full flex-wrap items-center gap-2 px-3 py-2 text-left" onClick={() => setOpenRun(openRun === r.id ? null : r.id)}>
                  <Terminal size={12} className="text-slate-500" />
                  <span className="text-[12px] font-medium text-slate-200">{r.tool}</span>
                  <span className="text-[10.5px] text-slate-500">{r.target}</span>
                  <Badge tone={r.status === 'completed' ? 'ok' : r.status === 'timeout' ? 'warn' : 'bad'}>{r.status}</Badge>
                  <span className="ml-auto text-[10px] text-slate-600">{fmtDate(r.started_at)}</span>
                </button>
                {openRun === r.id && (
                  <div className="border-t border-edge p-2">
                    <p className="mb-1 font-mono text-[10.5px] text-slate-500">{r.args}</p>
                    <pre className="code max-h-72 whitespace-pre-wrap">{r.output || '(no output)'}</pre>
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </Card>

      <Card title="Setup guidance">
        <div className="flex items-start gap-2 text-[11px] leading-relaxed text-slate-400">
          <Wrench size={14} className="mt-0.5 shrink-0 text-slate-500" />
          <div>
            <p>Nmap on Windows: <code className="text-cyan-300">winget install Insecure.Nmap</code> · In WSL: <code className="text-cyan-300">sudo apt install nmap</code></p>
            <p>Nikto: <code className="text-cyan-300">sudo apt install nikto</code> inside Ubuntu WSL.</p>
            <p>OWASP ZAP: install ZAP Desktop and set <code className="text-cyan-300">ZAP_HOME</code>; JAKE runs <code className="text-cyan-300">zap-baseline.py</code> with passive rules.</p>
            <p className="text-slate-500">JAKE never assumes a tool exists: availability and versions are detected on every visit.</p>
          </div>
        </div>
      </Card>
    </div>
  );
}
