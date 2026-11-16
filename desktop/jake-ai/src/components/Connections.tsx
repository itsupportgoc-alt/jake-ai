import { useState } from 'react';
import { Pencil, Plus, Trash2, PlugZap } from 'lucide-react';
import { Button, ErrorBox, Field, Modal, Select, Badge, Empty } from './ui';
import { call, useLoad, type ConnectionRow, type TargetRow } from '../lib/api';

interface AuthDraft {
  type: 'bearer' | 'basic' | 'cookie' | 'none';
  value: string;
  username: string;
}

interface Draft {
  id?: string;
  name: string;
  kind: 'rest' | 'mysql';
  targetId: string;
  baseUrl: string;
  admin: AuthDraft;
  viewer: AuthDraft;
  usersEndpoint: string;
  usersPath: string;
  rolesPath: string;
  loginEventsPath: string;
  policyPath: string;
  endpoints: string;
  mysqlHost: string;
  mysqlPort: string;
  mysqlUser: string;
  mysqlPassword: string;
  mysqlDatabase: string;
}

const SAMPLE_ENDPOINTS = `[
  { "name": "List users", "method": "GET", "path": "/api/admin/users", "viewerAllowed": false, "requiresAuth": true },
  { "name": "Restricted reports", "method": "GET", "path": "/api/reports", "viewerAllowed": false, "requiresAuth": true },
  { "name": "Public health", "method": "GET", "path": "/api/health", "viewerAllowed": true, "requiresAuth": false }
]`;

function blank(kind: 'rest' | 'mysql'): Draft {
  return {
    name: '',
    kind,
    targetId: '',
    baseUrl: '',
    admin: { type: 'bearer', value: '', username: '' },
    viewer: { type: 'bearer', value: '', username: '' },
    usersEndpoint: '/api/admin/users',
    usersPath: '',
    rolesPath: '',
    loginEventsPath: '',
    policyPath: '',
    endpoints: kind === 'rest' ? SAMPLE_ENDPOINTS : '[]',
    mysqlHost: '127.0.0.1',
    mysqlPort: '3306',
    mysqlUser: 'root',
    mysqlPassword: '',
    mysqlDatabase: '',
  };
}

export function ConnectionsPanel({ kind, selected, onSelect }: { kind: 'rest' | 'mysql'; selected: string | null; onSelect: (id: string | null) => void }) {
  const list = useLoad(() => call<ConnectionRow[]>('connections:list'), []);
  const targets = useLoad(() => call<TargetRow[]>('targets:list'), []);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [testResult, setTestResult] = useState<string | null>(null);

  const profiles = (list.data ?? []).filter((p) => (kind === 'rest' ? p.kind !== 'mysql' : p.kind === 'mysql'));

  const openNew = () => {
    setError(null);
    setTestResult(null);
    setDraft(blank(kind));
  };

  const openEdit = async (id: string) => {
    setError(null);
    setTestResult(null);
    try {
      const p = await call<ConnectionRow & { config: Record<string, any> }>('connections:get', { id });
      const c = p.config ?? {};
      setDraft({
        id,
        name: p.name,
        kind: p.kind === 'mysql' ? 'mysql' : 'rest',
        targetId: (p as unknown as { target?: { id: string } }).target?.id ?? '',
        baseUrl: p.base_url ?? '',
        admin: { type: (c.auth?.type as AuthDraft['type']) ?? 'bearer', value: '', username: c.auth?.username ?? '' },
        viewer: { type: (c.viewerAuth?.type as AuthDraft['type']) ?? 'bearer', value: '', username: c.viewerAuth?.username ?? '' },
        usersEndpoint: c.usersEndpoint ?? '/api/admin/users',
        usersPath: c.usersPath ?? '',
        rolesPath: c.rolesPath ?? '',
        loginEventsPath: c.loginEventsPath ?? '',
        policyPath: c.policyPath ?? '',
        endpoints: JSON.stringify(c.endpoints ?? [], null, 2),
        mysqlHost: c.mysql?.host ?? '127.0.0.1',
        mysqlPort: String(c.mysql?.port ?? 3306),
        mysqlUser: c.mysql?.user ?? 'root',
        mysqlPassword: '',
        mysqlDatabase: c.mysql?.database ?? '',
      });
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const save = async () => {
    if (!draft) return;
    setBusy(true);
    setError(null);
    try {
      let endpoints: unknown = [];
      if (draft.endpoints.trim()) {
        endpoints = JSON.parse(draft.endpoints);
        if (!Array.isArray(endpoints)) throw new Error('Endpoints must be a JSON array.');
      }
      const config: Record<string, unknown> = {
        auth: draft.admin.type === 'none' ? { type: 'none' } : { type: draft.admin.type, value: draft.admin.value || undefined, username: draft.admin.username || undefined },
        viewerAuth: draft.viewer.type === 'none' ? { type: 'none' } : { type: draft.viewer.type, value: draft.viewer.value || undefined, username: draft.viewer.username || undefined },
        usersEndpoint: draft.usersEndpoint || undefined,
        usersPath: draft.usersPath || undefined,
        rolesPath: draft.rolesPath || undefined,
        loginEventsPath: draft.loginEventsPath || undefined,
        policyPath: draft.policyPath || undefined,
        endpoints,
        mysql:
          draft.kind === 'mysql'
            ? { host: draft.mysqlHost, port: Number(draft.mysqlPort) || 3306, user: draft.mysqlUser, password: draft.mysqlPassword || undefined, database: draft.mysqlDatabase, readOnly: true }
            : undefined,
      };
      const saved = await call<ConnectionRow>('connections:save', {
        id: draft.id,
        name: draft.name,
        kind: draft.kind,
        targetId: draft.targetId,
        baseUrl: draft.baseUrl || undefined,
        username: draft.admin.username || draft.mysqlUser || undefined,
        config,
      });
      setDraft(null);
      list.reload();
      onSelect(saved.id);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const test = async (id: string) => {
    setBusy(true);
    try {
      const res = await call<{ ok: boolean; message: string; preview?: string }>('connections:test', { id });
      setTestResult(`${res.ok ? '✓' : '✕'} ${res.message}${res.preview ? `\n\nPreview:\n${res.preview}` : ''}`);
    } catch (e) {
      setTestResult(`✕ ${(e as Error).message}`);
    } finally {
      setBusy(false);
    }
  };

  const remove = async (id: string) => {
    if (!window.confirm('Delete this connection profile and its stored credentials?')) return;
    await call('connections:delete', { id });
    if (selected === id) onSelect(null);
    list.reload();
  };

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <div className="flex flex-wrap items-center gap-2">
          {profiles.length === 0 && <span className="text-xs text-slate-500">No {kind === 'mysql' ? 'database' : 'API'} profiles yet.</span>}
          {profiles.map((p) => (
            <div key={p.id} className={`flex items-center gap-2 rounded-md border px-2.5 py-1.5 ${selected === p.id ? 'border-cyan-700 bg-cyan-950/30' : 'border-edge bg-panel2'}`}>
              <button className="text-left" onClick={() => onSelect(p.id)}>
                <div className="text-[13px] font-medium text-slate-200">{p.name}</div>
                <div className="text-[10px] text-slate-500">{p.base_url ?? p.kind}</div>
              </button>
              <div className="flex items-center gap-1">
                <button title="Edit" className="rounded p-1 text-slate-400 hover:bg-slate-800 hover:text-slate-200" onClick={() => void openEdit(p.id)}>
                  <Pencil size={13} />
                </button>
                <button title="Test connection" className="rounded p-1 text-slate-400 hover:bg-slate-800 hover:text-slate-200" onClick={() => void test(p.id)}>
                  <PlugZap size={13} />
                </button>
                <button title="Delete" className="rounded p-1 text-slate-400 hover:bg-rose-950 hover:text-rose-300" onClick={() => void remove(p.id)}>
                  <Trash2 size={13} />
                </button>
              </div>
            </div>
          ))}
        </div>
        <Button onClick={openNew} className="btn-sm">
          <Plus size={13} /> Profile
        </Button>
      </div>
      {testResult && <pre className="code max-h-40 whitespace-pre-wrap">{testResult}</pre>}
      <ErrorBox message={error} />

      {draft && (
        <Modal title={draft.id ? `Edit profile: ${draft.name}` : 'New connection profile'} onClose={() => setDraft(null)} wide>
          <div className="space-y-3">
            <ErrorBox message={error} />
            <div className="grid grid-cols-2 gap-3">
              <Field label="Profile name">
                <input className="input" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} placeholder="Staging admin API" />
              </Field>
              <Field label="Authorized target">
                <Select value={draft.targetId} onChange={(v) => setDraft({ ...draft, targetId: v })}>
                  <option value="">Select a registered target…</option>
                  {(targets.data ?? []).map((t) => (
                    <option key={t.id} value={t.id}>
                      {t.label} — {t.url}
                    </option>
                  ))}
                </Select>
              </Field>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <Field label="Profile type">
                <Select value={draft.kind} onChange={(v) => setDraft({ ...draft, kind: v as 'rest' | 'mysql' })}>
                  <option value="rest">REST admin API</option>
                  <option value="mysql">MySQL / MariaDB</option>
                </Select>
              </Field>
              {draft.kind === 'rest' && (
                <Field label="Base URL" hint="Defaults to the target URL when empty.">
                  <input className="input" value={draft.baseUrl} onChange={(e) => setDraft({ ...draft, baseUrl: e.target.value })} placeholder="https://staging.example.com" />
                </Field>
              )}
            </div>

            {draft.kind === 'rest' ? (
              <>
                <div className="rounded-md border border-edge p-3">
                  <p className="mb-2 text-[11px] font-semibold uppercase tracking-wider text-muted">Administrator credential (encrypted)</p>
                  <div className="grid grid-cols-3 gap-3">
                    <Field label="Auth type">
                      <Select value={draft.admin.type} onChange={(v) => setDraft({ ...draft, admin: { ...draft.admin, type: v as AuthDraft['type'] } })}>
                        <option value="bearer">Bearer token</option>
                        <option value="basic">Basic auth</option>
                        <option value="cookie">Cookie header</option>
                        <option value="none">None</option>
                      </Select>
                    </Field>
                    <Field label="Username (basic only)">
                      <input className="input" value={draft.admin.username} onChange={(e) => setDraft({ ...draft, admin: { ...draft.admin, username: e.target.value } })} autoComplete="off" />
                    </Field>
                    <Field label="Token / password" hint={draft.id ? 'Leave blank to keep the stored secret.' : 'Stored with OS secure storage.'}>
                      <input className="input" type="password" value={draft.admin.value} onChange={(e) => setDraft({ ...draft, admin: { ...draft.admin, value: e.target.value } })} autoComplete="new-password" />
                    </Field>
                  </div>
                </div>
                <div className="rounded-md border border-edge p-3">
                  <p className="mb-2 text-[11px] font-semibold uppercase tracking-wider text-muted">Viewer / low-privilege test credential (RBAC only)</p>
                  <div className="grid grid-cols-3 gap-3">
                    <Field label="Auth type">
                      <Select value={draft.viewer.type} onChange={(v) => setDraft({ ...draft, viewer: { ...draft.viewer, type: v as AuthDraft['type'] } })}>
                        <option value="bearer">Bearer token</option>
                        <option value="basic">Basic auth</option>
                        <option value="cookie">Cookie header</option>
                        <option value="none">None</option>
                      </Select>
                    </Field>
                    <Field label="Username (basic only)">
                      <input className="input" value={draft.viewer.username} onChange={(e) => setDraft({ ...draft, viewer: { ...draft.viewer, username: e.target.value } })} autoComplete="off" />
                    </Field>
                    <Field label="Token / password" hint="Dedicated test account only.">
                      <input className="input" type="password" value={draft.viewer.value} onChange={(e) => setDraft({ ...draft, viewer: { ...draft.viewer, value: e.target.value } })} autoComplete="new-password" />
                    </Field>
                  </div>
                </div>
                <div className="grid grid-cols-2 gap-3">
                  <Field label="Users endpoint" hint="URL path that returns the account list.">
                    <input className="input" value={draft.usersEndpoint} onChange={(e) => setDraft({ ...draft, usersEndpoint: e.target.value })} placeholder="/api/admin/users" />
                  </Field>
                  <Field label="Users JSON path" hint="Optional dotted path inside the response, e.g. data.users. Blank = auto-detect.">
                    <input className="input" value={draft.usersPath} onChange={(e) => setDraft({ ...draft, usersPath: e.target.value })} />
                  </Field>
                  <Field label="Password policy endpoint">
                    <input className="input" value={draft.policyPath} onChange={(e) => setDraft({ ...draft, policyPath: e.target.value })} placeholder="/api/admin/security-policy" />
                  </Field>
                  <Field label="Login events endpoint" hint="Used to count recent failed logins.">
                    <input className="input" value={draft.loginEventsPath} onChange={(e) => setDraft({ ...draft, loginEventsPath: e.target.value })} />
                  </Field>
                  <Field label="Roles endpoint (optional)">
                    <input className="input" value={draft.rolesPath} onChange={(e) => setDraft({ ...draft, rolesPath: e.target.value })} />
                  </Field>
                </div>
                <Field label="Endpoints under test (JSON)" hint="RBAC sends only GET/HEAD/OPTIONS. Mark viewerAllowed=false for routes a Viewer must not reach.">
                  <textarea className="input h-44 font-mono text-[11px]" value={draft.endpoints} onChange={(e) => setDraft({ ...draft, endpoints: e.target.value })} />
                </Field>
              </>
            ) : (
              <div className="grid grid-cols-2 gap-3">
                <Field label="Host">
                  <input className="input" value={draft.mysqlHost} onChange={(e) => setDraft({ ...draft, mysqlHost: e.target.value })} />
                </Field>
                <Field label="Port">
                  <input className="input" value={draft.mysqlPort} onChange={(e) => setDraft({ ...draft, mysqlPort: e.target.value })} />
                </Field>
                <Field label="User" hint="Use a read-only account where possible.">
                  <input className="input" value={draft.mysqlUser} onChange={(e) => setDraft({ ...draft, mysqlUser: e.target.value })} />
                </Field>
                <Field label="Password" hint={draft.id ? 'Leave blank to keep the stored password.' : 'Encrypted with OS secure storage.'}>
                  <input className="input" type="password" value={draft.mysqlPassword} onChange={(e) => setDraft({ ...draft, mysqlPassword: e.target.value })} autoComplete="new-password" />
                </Field>
                <Field label="Database">
                  <input className="input" value={draft.mysqlDatabase} onChange={(e) => setDraft({ ...draft, mysqlDatabase: e.target.value })} />
                </Field>
              </div>
            )}
            <div className="flex items-center justify-between pt-1">
              <span className="text-[11px] text-slate-500">Credentials are encrypted in the OS credential store and never sent to the AI.</span>
              <div className="flex gap-2">
                <Button onClick={() => setDraft(null)}>Cancel</Button>
                <Button variant="primary" busy={busy} onClick={() => void save()}>
                  Save profile
                </Button>
              </div>
            </div>
          </div>
        </Modal>
      )}
      {profiles.length === 0 && !draft && (
        <Empty
          title="A connection profile is required"
          hint="JAKE needs an authorized admin API or database connection to inspect accounts and permissions. Credentials are stored encrypted and are never sent to the AI."
        />
      )}
      {profiles.length === 0 && <div className="hidden"><Badge>—</Badge></div>}
    </div>
  );
}
