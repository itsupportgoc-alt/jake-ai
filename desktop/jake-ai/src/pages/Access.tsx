import { useState } from 'react';
import { Camera, ExternalLink, LogIn, LogOut, ShieldAlert } from 'lucide-react';
import { call, fmtDate, useLoad, type CapturesRow, type TargetRow } from '../lib/api';
import { Button, Card, Empty, ErrorBox, Field, Select, Spinner } from '../components/ui';

export default function Access() {
  const targets = useLoad(() => call<TargetRow[]>('targets:list'), []);
  const captures = useLoad(() => call<CapturesRow[]>('access:list'), []);
  const [targetId, setTargetId] = useState('');
  const [url, setUrl] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [capture, setCapture] = useState<CapturesRow | null>(null);

  const selected = (targets.data ?? []).find((t) => t.id === targetId);

  const pickTarget = (id: string) => {
    setTargetId(id);
    const t = (targets.data ?? []).find((x) => x.id === id);
    if (t) setUrl(`${new URL(t.url).origin}/login`);
  };

  const openLogin = async () => {
    setBusy('open');
    setError(null);
    try {
      await call('access:open', { targetId, url });
      setOpen(true);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  const doCapture = async () => {
    setBusy('capture');
    setError(null);
    try {
      const c = await call<CapturesRow>('access:capture');
      setCapture(c);
      captures.reload();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  const close = async () => {
    await call('access:close');
    setOpen(false);
  };

  return (
    <div className="space-y-4">
      <Card title="Authorized system access">
        <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
          <Field label="Authorized target">
            <Select value={targetId} onChange={pickTarget}>
              <option value="">Select a registered target…</option>
              {(targets.data ?? []).map((t) => (
                <option key={t.id} value={t.id}>
                  {t.label} — {t.url}
                </option>
              ))}
            </Select>
          </Field>
          <div className="md:col-span-2">
            <Field label="Login URL (must be in the target's scope)">
              <input className="input font-mono text-[11.5px]" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://staging.example.com/login" />
            </Field>
          </div>
        </div>
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <Button variant="primary" disabled={!targetId || !url} busy={busy === 'open'} onClick={() => void openLogin()}>
            <LogIn size={14} /> Open login window
          </Button>
          <Button disabled={!open} busy={busy === 'capture'} onClick={() => void doCapture()}>
            <Camera size={14} /> Capture page structure
          </Button>
          <Button disabled={!open} onClick={() => void close()}>
            <LogOut size={14} /> Close session
          </Button>
          {open && <span className="text-[11px] text-emerald-400">Session open — sign in in the browser window, then capture.</span>}
        </div>
        <div className="mt-3 flex items-start gap-2 rounded border border-cyan-900/50 bg-cyan-950/10 px-3 py-2 text-[11px] leading-relaxed text-cyan-200/80">
          <ShieldAlert size={14} className="mt-0.5 shrink-0" />
          <div>
            JAKE opens the system Edge/Chrome browser for you to sign in yourself. Your password is typed into the site, never into JAKE, and is never transmitted to the AI.
            The browser profile is stored in JAKE's encrypted Electron session directory. Capture stores the page structure with password field values stripped.
          </div>
        </div>
        <div className="mt-2"><ErrorBox message={error} /></div>
      </Card>

      {capture && (
        <Card title={`Captured page — ${capture.title || capture.url}`}>
          <pre className="code max-h-80 whitespace-pre-wrap">{capture.summary}</pre>
          <p className="mt-2 break-all text-[10.5px] text-slate-500">HTML snapshot: {capture.htmlPath}</p>
        </Card>
      )}

      <Card title={`Session captures (${captures.data?.length ?? 0})`}>
        {captures.loading ? (
          <Spinner />
        ) : (captures.data ?? []).length === 0 ? (
          <Empty title="No captured pages" hint="Open an authorized login, sign in manually, then capture the page structure to review navigation, forms, tables and permission hints." />
        ) : (
          <div className="space-y-2">
            {(captures.data ?? []).map((c) => (
              <div key={c.id} className="rounded border border-edge p-3">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="min-w-0">
                    <div className="truncate text-[12.5px] font-medium text-slate-200">{c.title || '(no title)'}</div>
                    <div className="break-all font-mono text-[10px] text-slate-500">{c.url}</div>
                  </div>
                  <div className="flex items-center gap-2">
                    <span className="text-[10px] text-slate-600">{fmtDate(c.captured_at)}</span>
                    <Button className="btn-sm" onClick={() => setCapture(c)}>
                      <ExternalLink size={11} /> Details
                    </Button>
                  </div>
                </div>
                <pre className="mt-2 max-h-24 overflow-auto whitespace-pre-wrap text-[10.5px] text-slate-500">{c.summary}</pre>
              </div>
            ))}
          </div>
        )}
        {selected && <p className="mt-2 text-[10px] text-slate-600">Tip: capture the user-management, roles and permissions pages to gather evidence for authorized account review.</p>}
      </Card>
    </div>
  );
}
