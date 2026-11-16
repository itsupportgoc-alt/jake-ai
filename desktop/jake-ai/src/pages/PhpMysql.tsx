import { useState } from 'react';
import Editor from '@monaco-editor/react';
import { Database, FileCode2, Play, Save } from 'lucide-react';
import { call } from '../lib/api';
import { Badge, Button, Card, Empty, ErrorBox, Field, Modal, Severity, Spinner } from '../components/ui';
import { ConnectionsPanel } from '../components/Connections';

interface PhpIssue {
  file: string;
  line: number;
  ruleId: string;
  severity: string;
  confidence: string;
  snippet: string;
  message: string;
  impact: string;
  remediation: string;
  proposedFix?: string;
  references: string[];
}

interface PhpResult {
  assessmentId: string;
  result: { root: string; filesScanned: number; issues: PhpIssue[]; truncated: boolean };
}

interface MysqlResult {
  assessmentId: string;
  result: { server: string; database: string; tables: number; userTables: string[]; issues: Array<{ severity: string; name: string; detail: string; remediation: string }>; notes: string[] };
}

export default function PhpMysql() {
  const [tab, setTab] = useState<'php' | 'mysql'>('php');

  return (
    <div className="space-y-4">
      <div className="flex gap-1 border-b border-edge">
        <button className={`px-4 py-2 text-[12.5px] font-medium ${tab === 'php' ? 'border-b-2 border-cyan-500 text-cyan-300' : 'text-slate-500 hover:text-slate-300'}`} onClick={() => setTab('php')}>
          <FileCode2 size={13} className="mr-1 inline" /> PHP source auditor
        </button>
        <button className={`px-4 py-2 text-[12.5px] font-medium ${tab === 'mysql' ? 'border-b-2 border-cyan-500 text-cyan-300' : 'text-slate-500 hover:text-slate-300'}`} onClick={() => setTab('mysql')}>
          <Database size={13} className="mr-1 inline" /> MySQL / MariaDB auditor
        </button>
      </div>
      {tab === 'php' ? <PhpPanel /> : <MysqlPanel />}
    </div>
  );
}

function PhpPanel() {
  const [directory, setDirectory] = useState(localStorage.getItem('jake.phpDir') ?? '');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<PhpResult | null>(null);
  const [issue, setIssue] = useState<PhpIssue | null>(null);
  const [fileContent, setFileContent] = useState('');
  const [oldText, setOldText] = useState('');
  const [newText, setNewText] = useState('');
  const [patchMsg, setPatchMsg] = useState<string | null>(null);
  const [filter, setFilter] = useState('');

  const scan = async () => {
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      const res = await call<PhpResult>('php:scan', { directory });
      setResult(res);
      localStorage.setItem('jake.phpDir', directory);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const openIssue = async (i: PhpIssue) => {
    setIssue(i);
    setOldText(i.snippet);
    setNewText(i.proposedFix ?? i.snippet);
    setPatchMsg(null);
    setFileContent('');
    setError(null);
    try {
      const f = await call<{ path: string; content: string }>('php:file', { file: i.file });
      setFileContent(f.content);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const apply = async () => {
    if (!issue) return;
    setBusy(true);
    setPatchMsg(null);
    try {
      const res = await call<{ backup: string; path: string }>('php:apply', { file: issue.file, oldText, newText });
      setPatchMsg(`Patch applied to ${res.path}. Backup created at ${res.backup}.`);
    } catch (e) {
      setPatchMsg(`Not applied: ${(e as Error).message}`);
    } finally {
      setBusy(false);
    }
  };

  const shown = (result?.result.issues ?? []).filter((i) => !filter || i.message.toLowerCase().includes(filter.toLowerCase()) || i.file.toLowerCase().includes(filter.toLowerCase()));

  return (
    <div className="space-y-4">
      <Card title="Read-only PHP source review">
        <div className="flex flex-wrap items-end gap-3">
          <div className="min-w-[340px] flex-1">
            <Field label="Local PHP project directory" hint="Read-only. JAKE only reads files inside this directory; edits require approval and create a backup.">
              <input className="input font-mono text-[11.5px]" value={directory} onChange={(e) => setDirectory(e.target.value)} placeholder="C:\xampp\htdocs\myapp" />
            </Field>
          </div>
          <Button variant="primary" disabled={!directory.trim()} busy={busy} onClick={() => void scan()}>
            <Play size={14} /> Scan project
          </Button>
        </div>
        <p className="mt-2 text-[11px] text-slate-500">
          Inspects authentication logic, password hashing, SQL construction/prepared statements, sessions, CSRF, output escaping, upload validation, access control, secrets, error disclosure and sensitive files.
          Requires level 2 approval.
        </p>
        <div className="mt-3"><ErrorBox message={error} /></div>
      </Card>

      {busy && !result && <Card><Spinner label="Walking and analyzing PHP files…" /></Card>}

      {result && (
        <Card
          title={`${result.result.filesScanned} files scanned · ${result.result.issues.length} issues`}
          actions={
            <input className="input h-7 w-56 text-[11px]" placeholder="Filter by rule or file" value={filter} onChange={(e) => setFilter(e.target.value)} />
          }
        >
          {result.result.truncated && <p className="mb-2 text-[11px] text-amber-400">Directory traversal was truncated at the safety limit.</p>}
          {shown.length === 0 ? (
            <Empty title="No matching issues" hint="Static analysis is heuristic: no finding does not guarantee the code is secure." />
          ) : (
            <div className="divide-y divide-edge/60">
              {shown.map((i, idx) => (
                <button key={`${i.file}:${i.line}:${i.ruleId}:${idx}`} className="block w-full py-2 text-left hover:bg-slate-800/20" onClick={() => void openIssue(i)}>
                  <div className="flex flex-wrap items-center gap-2">
                    <Severity value={i.severity} />
                    <span className="text-[12.5px] text-slate-200">{i.message}</span>
                    <span className="chip">{i.ruleId}</span>
                  </div>
                  <div className="mt-0.5 font-mono text-[10.5px] text-cyan-500/80">
                    {i.file}:{i.line}
                  </div>
                  <pre className="mt-1 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[10.5px] text-slate-500">{i.snippet}</pre>
                </button>
              ))}
            </div>
          )}
        </Card>
      )}

      {!result && !busy && (
        <Card>
          <Empty title="No PHP scan yet" hint="Point JAKE at a local PHP project (for example C:\xampp\htdocs\app). Analysis is read-only and never changes your files." />
        </Card>
      )}

      {issue && (
        <Modal title={`${issue.ruleId} — ${issue.file}:${issue.line}`} onClose={() => setIssue(null)} wide>
          <div className="space-y-3">
            <div className="flex flex-wrap items-center gap-2">
              <Severity value={issue.severity} />
              <span className="text-[12px] text-slate-400">confidence: {issue.confidence}</span>
            </div>
            <p className="text-[12px] leading-relaxed text-slate-300">{issue.message}</p>
            <p className="text-[11px] text-slate-400"><span className="text-slate-500">Impact:</span> {issue.impact}</p>
            <p className="text-[11px] text-cyan-300/80"><span className="text-slate-500">Remediation:</span> {issue.remediation}</p>
            {fileContent && (
              <div className="overflow-hidden rounded border border-edge">
                <Editor height="300px" language="php" theme="vs-dark" value={fileContent} options={{ readOnly: true, minimap: { enabled: false }, fontSize: 12, scrollBeyondLastLine: false }} />
              </div>
            )}
            <div className="grid grid-cols-2 gap-2">
              <Field label="Exact code to replace">
                <textarea className="input h-24 font-mono text-[10.5px]" value={oldText} onChange={(e) => setOldText(e.target.value)} />
              </Field>
              <Field label="Replacement code">
                <textarea className="input h-24 font-mono text-[10.5px]" value={newText} onChange={(e) => setNewText(e.target.value)} />
              </Field>
            </div>
            {patchMsg && <pre className="code whitespace-pre-wrap">{patchMsg}</pre>}
            <div className="flex items-center justify-between">
              <p className="max-w-md text-[10px] text-slate-500">Applying a patch requires level 4 approval and writes a timestamped .jake-backup file next to the original.</p>
              <Button variant="primary" busy={busy} onClick={() => void apply()}>
                <Save size={13} /> Apply patch
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  );
}

function MysqlPanel() {
  const [connectionId, setConnectionId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<MysqlResult | null>(null);

  const run = async () => {
    if (!connectionId) return;
    setBusy(true);
    setError(null);
    try {
      setResult(await call<MysqlResult>('mysql:audit', { connectionId }));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      <Card title="Database connection profile" actions={<span className="text-[10px] text-slate-500">Read-only session</span>}>
        <ConnectionsPanel kind="mysql" selected={connectionId} onSelect={setConnectionId} />
      </Card>
      <Card title="MySQL / MariaDB security audit">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="max-w-3xl text-[11.5px] leading-relaxed text-slate-400">
            Connects with a read-only session and inspects privileges, database account uniqueness, password hash algorithm (prefix only — hashes are never read), schema integrity, foreign keys and server authentication hardening.
            Never dumps table rows or credentials.
          </p>
          <Button variant="primary" disabled={!connectionId} busy={busy} onClick={() => void run()}>
            <Play size={14} /> Audit database
          </Button>
        </div>
        <div className="mt-3"><ErrorBox message={error} /></div>
      </Card>
      {busy && <Card><Spinner label="Inspecting database metadata…" /></Card>}
      {!result && !busy && (
        <Card><Empty title="No database audit yet" hint="Add a MySQL profile pointing at your authorized local database (XAMPP defaults: 127.0.0.1:3306). Requires level 3 approval." /></Card>
      )}
      {result && (
        <>
          <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
            {[
              ['Server', result.result.server],
              ['Database', result.result.database],
              ['Tables', String(result.result.tables)],
              ['Account tables', result.result.userTables.join(', ') || 'none detected'],
            ].map(([k, v]) => (
              <div key={k} className="card-pad">
                <div className="text-[10px] uppercase tracking-wider text-muted">{k}</div>
                <div className="mt-1 break-words text-[13px] text-slate-200">{v}</div>
              </div>
            ))}
          </div>
          <Card title={`Issues (${result.result.issues.length})`}>
            {result.result.issues.length === 0 ? (
              <p className="text-xs text-emerald-400">No database issues detected from the available metadata.</p>
            ) : (
              <div className="space-y-2">
                {result.result.issues.map((x, i) => (
                  <div key={i} className="rounded border border-edge p-3">
                    <div className="flex items-center gap-2">
                      <Severity value={x.severity} />
                      <span className="text-[12.5px] font-medium text-slate-200">{x.name}</span>
                    </div>
                    <p className="mt-1 text-[11.5px] text-slate-400">{x.detail}</p>
                    <p className="mt-1 text-[11px] text-cyan-300/80">Fix: {x.remediation}</p>
                  </div>
                ))}
              </div>
            )}
            {result.result.notes.length > 0 && (
              <div className="mt-3 border-t border-edge pt-2">
                {result.result.notes.map((n, i) => (
                  <div key={i} className="text-[10.5px] text-slate-500">· {n}</div>
                ))}
              </div>
            )}
            <div className="mt-2"><Badge>Assessment {result.assessmentId}</Badge></div>
          </Card>
        </>
      )}
    </div>
  );
}
