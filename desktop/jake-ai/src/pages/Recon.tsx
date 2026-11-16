import { useState } from 'react';
import { Radar } from 'lucide-react';
import { call, useLoad, type TargetRow } from '../lib/api';
import { Badge, Button, Card, Empty, ErrorBox, Field, KeyValue, Select, Spinner } from '../components/ui';

interface ReconData {
  url: string;
  assessedAt: string;
  dns: Record<string, string[]>;
  ips: string[];
  httpStatus: number | null;
  finalUrl: string | null;
  redirects: string[];
  server: string | null;
  poweredBy: string | null;
  technologies: string[];
  tls: { protocol: string; cipher: string; authorized: boolean; issuer: string; subject: string; validTo: string; daysRemaining: number; altNames: string[] } | null;
  headers: Record<string, string>;
  cookies: Array<{ name: string; secure: boolean; httpOnly: boolean; sameSite: string | null }>;
  robots: { status: number | null; disallow: string[]; sitemaps: string[] };
  sitemap: { status: number | null; urls: string[] };
  endpoints: string[];
  subdomains: string[];
  emailSecurity: { spf: string | null; dmarc: string | null };
  securityTxt: boolean;
  error: string | null;
}

export default function Recon() {
  const targets = useLoad(() => call<TargetRow[]>('targets:list'), []);
  const [targetId, setTargetId] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [data, setData] = useState<ReconData | null>(null);
  const [assessmentId, setAssessmentId] = useState<string | null>(null);

  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      const res = await call<{ assessmentId: string; data: ReconData }>('recon:run', { targetId });
      setData(res.data);
      setAssessmentId(res.assessmentId);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      <Card title="Passive website reconnaissance">
        <div className="flex flex-wrap items-end gap-3">
          <div className="min-w-[320px] flex-1">
            <Field label="Authorized target">
              <Select value={targetId} onChange={setTargetId}>
                <option value="">Select a registered target…</option>
                {(targets.data ?? []).map((t) => (
                  <option key={t.id} value={t.id}>
                    {t.label} — {t.url}
                  </option>
                ))}
              </Select>
            </Field>
          </div>
          <Button variant="primary" busy={busy} disabled={!targetId} onClick={() => void run()}>
            <Radar size={14} /> Run reconnaissance
          </Button>
        </div>
        <p className="mt-2 text-[11px] text-slate-500">
          Level 1 passive inspection: DNS, TLS, HTTP headers, technology fingerprinting, robots.txt, sitemap and certificate transparency. No exploitation and no scope expansion.
        </p>
        <div className="mt-3"><ErrorBox message={error} /></div>
      </Card>

      {busy && <Card><Spinner label="Collecting passive data…" /></Card>}

      {!data && !busy && (
        <Card>
          <Empty title="No reconnaissance result yet" hint="Select a target and run reconnaissance to see DNS, TLS, header and endpoint data collected live." />
        </Card>
      )}

      {data && (
        <div className="grid grid-cols-1 gap-4 xl:grid-cols-3">
          <Card title="Target overview" className="xl:col-span-2">
            {assessmentId && <p className="mb-2 text-[10.5px] text-slate-500">Assessment {assessmentId}</p>}
            <KeyValue
              items={[
                ['Target URL', <span className="font-mono text-[11.5px]">{data.url}</span>],
                ['Final URL', <span className="break-all font-mono text-[11.5px]">{data.finalUrl ?? '—'}</span>],
                ['HTTP status', data.httpStatus ?? 'unreachable'],
                ['Resolved IPs', data.ips.length ? <span className="font-mono text-[11.5px]">{data.ips.join(', ')}</span> : 'none'],
                ['Server', data.server ?? 'not disclosed'],
                ['X-Powered-By', data.poweredBy ?? 'not disclosed'],
                ['Technologies', (data.technologies ?? []).length ? <div className="flex flex-wrap gap-1">{data.technologies.map((t) => <Badge key={t}>{t}</Badge>)}</div> : 'not fingerprinted'],
                ['DNS A', (data.dns.A ?? []).join(', ') || '—'],
                ['DNS CNAME', (data.dns.CNAME ?? []).join(', ') || '—'],
                ['DNS MX', (data.dns.MX ?? []).join(', ') || '—'],
                ['SPF', data.emailSecurity.spf ?? 'missing'],
                ['DMARC', data.emailSecurity.dmarc ?? 'missing'],
                ['security.txt', data.securityTxt ? 'published' : 'not found'],
                ['Assessed at', data.assessedAt],
                ...(data.redirects.length ? ([['Redirect chain', <span key="r" className="break-all font-mono text-[11px]">{data.redirects.join(' → ')}</span>]] as Array<[string, JSX.Element]> as never) : []),
              ]}
            />
            {data.error && <div className="mt-2"><ErrorBox message={data.error} /></div>}
          </Card>

          <Card title="TLS">
            {data.tls ? (
              <KeyValue
                items={[
                  ['Protocol', data.tls.protocol],
                  ['Cipher', data.tls.cipher],
                  ['Trusted chain', data.tls.authorized ? <Badge tone="ok">valid</Badge> : <Badge tone="bad">invalid / untrusted</Badge>],
                  ['Subject', data.tls.subject ?? '—'],
                  ['Issuer', data.tls.issuer ?? '—'],
                  ['Expires', `${new Date(data.tls.validTo).toLocaleDateString()} (${data.tls.daysRemaining} days)`],
                  ['Alt names', <span className="break-all text-[11px]">{data.tls.altNames.slice(0, 20).join(', ')}</span>],
                ]}
              />
            ) : (
              <p className="text-xs text-slate-500">No TLS session (target is not HTTPS or was unreachable).</p>
            )}
          </Card>

          <Card title="Security headers">
            <table className="table">
              <tbody>
                {['strict-transport-security', 'content-security-policy', 'x-frame-options', 'x-content-type-options', 'referrer-policy', 'permissions-policy'].map((h) => (
                  <tr key={h}>
                    <th className="bg-panel2/40 font-mono text-[10.5px] normal-case">{h}</th>
                    <td className={data.headers[h] ? 'text-slate-300' : 'text-rose-400'}>
                      <span className="line-clamp-3 break-all text-[11px]">{data.headers[h] ?? 'missing'}</span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            {data.cookies.length > 0 && (
              <div className="mt-3">
                <p className="label">Cookies observed</p>
                {data.cookies.map((c) => (
                  <div key={c.name} className="mb-1 flex items-center justify-between text-[11px]">
                    <span className="font-mono text-slate-300">{c.name}</span>
                    <div className="flex gap-1">
                      <Badge tone={c.secure ? 'ok' : 'bad'}>Secure</Badge>
                      <Badge tone={c.httpOnly ? 'ok' : 'bad'}>HttpOnly</Badge>
                      <Badge tone={c.sameSite ? 'ok' : 'warn'}>{c.sameSite ?? 'no SameSite'}</Badge>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </Card>

          <Card title={`Public endpoints (${data.endpoints.length})`} className="xl:col-span-1">
            <div className="max-h-72 space-y-0.5 overflow-auto">
              {data.endpoints.length === 0 && <p className="text-xs text-slate-500">No same-host links discovered.</p>}
              {data.endpoints.map((e) => (
                <div key={e} className="break-all rounded px-1.5 py-1 font-mono text-[10.5px] text-slate-400 hover:bg-slate-800/40" title={e}>
                  {e}
                </div>
              ))}
            </div>
          </Card>

          <Card title={`Public subdomains from CT logs (${data.subdomains.length})`} className="xl:col-span-1">
            <div className="max-h-72 space-y-0.5 overflow-auto">
              {data.subdomains.length === 0 && <p className="text-xs text-slate-500">No subdomains returned by crt.sh (or the lookup failed).</p>}
              {data.subdomains.map((s) => (
                <div key={s} className="rounded px-1.5 py-1 font-mono text-[10.5px] text-slate-400">{s}</div>
              ))}
            </div>
            <p className="mt-2 text-[10px] text-slate-600">Passive OSINT only. These hosts are not automatically tested and are outside scope unless registered.</p>
          </Card>

          <Card title="robots.txt / sitemap">
            <KeyValue
              items={[
                ['robots.txt', data.robots.status === 200 ? <Badge tone="ok">present</Badge> : `HTTP ${data.robots.status ?? 'n/a'}`],
                ['Disallowed paths', (data.robots.disallow ?? []).join(', ') || '—'],
                ['Sitemaps', (data.robots.sitemaps ?? []).join(', ') || '—'],
                ['sitemap.xml', data.sitemap.status === 200 ? `${data.sitemap.urls.length} URLs` : `HTTP ${data.sitemap.status ?? 'n/a'}`],
              ]}
            />
            {data.sitemap.urls.length > 0 && (
              <div className="mt-2 max-h-40 overflow-auto">
                {data.sitemap.urls.slice(0, 60).map((u) => (
                  <div key={u} className="break-all font-mono text-[10px] text-slate-500">{u}</div>
                ))}
              </div>
            )}
          </Card>
        </div>
      )}
    </div>
  );
}
