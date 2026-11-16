import { randomUUID } from 'node:crypto';
import https from 'node:https';
import tls from 'node:tls';
import type { Severity, Target } from '../../shared/types';
import { isoNow } from './db';
import type { Assessments } from './assessments';
import type { Database } from './db';
import { assertUrlInScope, describeTls, dnsRecords, scopedFetch, type FetchResult, type TlsInfo } from './scope';

export interface ScanContext {
  db: Database;
  assessments: Assessments;
  target: Target;
  assessmentId: string;
  signal: AbortSignal;
  onProgress: (progress: number, message: string) => void;
}

export interface ReconData {
  target: string;
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
  tls: TlsInfo | null;
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

const HEADER_RULES: Array<{ header: string; severity: 'critical' | 'high' | 'medium' | 'low' | 'info'; name: string; impact: string; remediation: string }> = [
  {
    header: 'strict-transport-security',
    severity: 'high',
    name: 'HSTS not enforced',
    impact: 'Clients can be downgraded to plain HTTP, enabling session theft on hostile networks.',
    remediation: 'Send Strict-Transport-Security: max-age=31536000; includeSubDomains (add preload once verified).',
  },
  {
    header: 'content-security-policy',
    severity: 'medium',
    name: 'Content-Security-Policy missing',
    impact: 'No browser-enforced mitigation against cross-site scripting and data injection.',
    remediation: "Define a restrictive CSP (default-src 'self') and remove unsafe-inline where possible.",
  },
  {
    header: 'x-frame-options',
    severity: 'medium',
    name: 'Clickjacking protection missing',
    impact: 'The site can be framed by a hostile page and used for clickjacking.',
    remediation: "Send X-Frame-Options: DENY, or CSP frame-ancestors 'none'.",
  },
  {
    header: 'x-content-type-options',
    severity: 'low',
    name: 'X-Content-Type-Options missing',
    impact: 'Browsers may MIME-sniff responses and execute content as a different type.',
    remediation: 'Send X-Content-Type-Options: nosniff.',
  },
  {
    header: 'referrer-policy',
    severity: 'low',
    name: 'Referrer-Policy not set',
    impact: 'URLs and query data may leak to third parties through the Referer header.',
    remediation: 'Send Referrer-Policy: strict-origin-when-cross-origin (or no-referrer).',
  },
  {
    header: 'permissions-policy',
    severity: 'info',
    name: 'Permissions-Policy not set',
    impact: 'Browser features (camera, geolocation) are not explicitly restricted.',
    remediation: 'Send a Permissions-Policy that disables unused features.',
  },
];

const OWASP_HEADERS = 'https://owasp.org/www-project-secure-headers/';

export function analyzeHeaders(url: string, headers: Record<string, string>): Array<{ name: string; severity: Severity; detail: string; remediation: string; impact: string; evidence: string }> {
  const out: Array<{ name: string; severity: Severity; detail: string; remediation: string; impact: string; evidence: string }> = [];
  const isHttps = url.startsWith('https:');
  for (const rule of HEADER_RULES) {
    if (rule.header === 'strict-transport-security' && !isHttps) continue;
    const value = headers[rule.header];
    if (!value) {
      out.push({
        name: rule.name,
        severity: rule.severity,
        detail: `Response header "${rule.header}" is absent.`,
        remediation: rule.remediation,
        impact: rule.impact,
        evidence: `Missing header: ${rule.header}`,
      });
      continue;
    }
    if (rule.header === 'strict-transport-security') {
      const maxAge = /max-age=(\d+)/i.exec(value);
      if (!maxAge || Number(maxAge[1]) < 15552000) {
        out.push({
          name: 'HSTS max-age too short',
          severity: 'medium',
          detail: `HSTS is present but weak: "${value}".`,
          remediation: 'Use max-age of at least 15552000 (six months); 31536000 recommended.',
          impact: 'A short HSTS lifetime leaves a window for protocol downgrade attacks.',
          evidence: `strict-transport-security: ${value}`,
        });
      }
    }
    if (rule.header === 'content-security-policy' && /unsafe-inline|unsafe-eval/i.test(value)) {
      out.push({
        name: 'Weak Content-Security-Policy',
        severity: 'low',
        detail: `CSP allows unsafe sources: "${value.slice(0, 300)}".`,
        remediation: "Remove 'unsafe-inline'/'unsafe-eval' and use nonces or hashes.",
        impact: 'XSS mitigation is weakened.',
        evidence: `content-security-policy: ${value.slice(0, 300)}`,
      });
    }
    if (rule.header === 'x-frame-options' && value && !/deny|sameorigin/i.test(value)) {
      out.push({
        name: 'Weak X-Frame-Options',
        severity: 'low',
        detail: `Unexpected value "${value}".`,
        remediation: 'Use DENY or SAMEORIGIN.',
        impact: 'Framing protection may not behave as intended.',
        evidence: `x-frame-options: ${value}`,
      });
    }
  }
  if (headers.server && /\d+\.\d+/.test(headers.server)) {
    out.push({
      name: 'Server version disclosure',
      severity: 'low',
      detail: `The Server header exposes a version: "${headers.server}".`,
      remediation: 'Remove or genericise version information from server banners.',
      impact: 'Attackers can target known vulnerabilities for that exact version.',
      evidence: `server: ${headers.server}`,
    });
  }
  if (headers['x-powered-by']) {
    out.push({
      name: 'Technology disclosure via X-Powered-By',
      severity: 'info',
      detail: `X-Powered-By: ${headers['x-powered-by']}`,
      remediation: 'Disable the X-Powered-By header in the application/php.ini.',
      impact: 'Minor fingerprinting aid for attackers.',
      evidence: `x-powered-by: ${headers['x-powered-by']}`,
    });
  }
  return out;
}

export function parseCookies(setCookie: string | undefined): Array<{ name: string; secure: boolean; httpOnly: boolean; sameSite: string | null }> {
  if (!setCookie) return [];
  return setCookie
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const [pair, ...attrs] = line.split(';');
      const name = pair?.split('=')[0]?.trim() ?? '';
      const lower = attrs.join(';').toLowerCase();
      const ss = /samesite=([^;]+)/.exec(lower);
      return {
        name,
        secure: /;\s*secure/.test(`;${lower}`),
        httpOnly: /;\s*httponly/.test(`;${lower}`),
        sameSite: ss ? ss[1].trim() : null,
      };
    })
    .filter((c) => c.name.length > 0 && c.name.length < 128);
}

export function fingerprintTechnologies(headers: Record<string, string>, body: string, cookies: string[]): string[] {
  const tech = new Set<string>();
  const cookieNames = cookies.join(' ').toLowerCase();
  const server = (headers.server ?? '').toLowerCase();
  const powered = (headers['x-powered-by'] ?? '').toLowerCase();
  if (server.includes('nginx')) tech.add('Nginx');
  if (server.includes('apache')) tech.add('Apache HTTP Server');
  if (server.includes('iis') || server.includes('microsoft-httpapi')) tech.add('Microsoft IIS');
  if (server.includes('cloudflare')) tech.add('Cloudflare');
  if (powered.includes('php')) tech.add('PHP');
  if (powered.includes('express')) tech.add('Express');
  if (powered.includes('asp.net')) tech.add('ASP.NET');
  if (cookieNames.includes('phpsessid')) tech.add('PHP');
  if (cookieNames.includes('jsessionid')) tech.add('Java / Servlet');
  if (cookieNames.includes('asp.net_sessionid')) tech.add('ASP.NET');
  if (cookieNames.includes('laravel_session')) tech.add('Laravel');
  if (cookieNames.includes('csrftoken')) tech.add('Django');
  if (cookieNames.includes('ci_session')) tech.add('CodeIgniter');
  if (/wp-content|wp-includes/.test(body)) tech.add('WordPress');
  if (/__NEXT_DATA__/.test(body)) tech.add('Next.js');
  if (/__NUXT__|nuxt-link/.test(body)) tech.add('Nuxt');
  if (/ng-version=|ng-app/.test(body)) tech.add('Angular');
  if (/react-dom|data-reactroot|_reactRootContainer/.test(body)) tech.add('React');
  if (/vue(?:\.js|@|\.runtime)/i.test(body) || /data-v-[0-9a-f]{8}/.test(body)) tech.add('Vue.js');
  if (/jquery/i.test(body)) tech.add('jQuery');
  if (/bootstrap(?:\.min)?\.(?:css|js)/i.test(body)) tech.add('Bootstrap');
  if (/x-shopify|shopify\.com/.test(body + server)) tech.add('Shopify');
  if (/x-generator.*drupal|Drupal\.settings/i.test(body + JSON.stringify(headers))) tech.add('Drupal');
  const gen = /<meta[^>]+name=["']generator["'][^>]+content=["']([^"']+)/i.exec(body);
  if (gen) tech.add(gen[1].slice(0, 60));
  return [...tech];
}

function getTls(url: string): Promise<TlsInfo | null> {
  if (!url.startsWith('https:')) return Promise.resolve(null);
  const u = new URL(url);
  return new Promise((resolve) => {
    const socket = tls.connect(
      { host: u.hostname, port: Number(u.port || 443), servername: u.hostname, rejectUnauthorized: false, timeout: 10000 },
      () => {
        resolve(describeTls(socket));
        socket.destroy();
      },
    );
    socket.on('error', () => resolve(null));
    socket.on('timeout', () => {
      socket.destroy();
      resolve(null);
    });
  });
}

export async function runRecon(ctx: ScanContext): Promise<ReconData> {
  const { target, signal, onProgress } = ctx;
  const url = target.url;
  const host = new URL(url).hostname;
  onProgress(5, `Resolving DNS for ${host}`);
  const dns = await dnsRecords(host);
  const ips = [...(dns.A ?? []), ...(dns.AAAA ?? [])];

  onProgress(25, 'Requesting the target over HTTPS/HTTP');
  let main: FetchResult | null = null;
  let error: string | null = null;
  try {
    await assertUrlInScope(target, url);
    main = await scopedFetch(target, url, { signal, timeoutMs: 15000 });
  } catch (err) {
    error = (err as Error).message;
  }

  const tlsInfo = main?.tls ?? (await getTls(url));

  let robots: ReconData['robots'] = { status: null, disallow: [], sitemaps: [] };
  let sitemap: ReconData['sitemap'] = { status: null, urls: [] };
  let securityTxt = false;
  if (main && main.status < 500) {
    onProgress(50, 'Reading robots.txt and sitemap.xml');
    try {
      const r = await scopedFetch(target, new URL('/robots.txt', url).toString(), { signal, timeoutMs: 8000 });
      robots.status = r.status;
      if (r.status === 200) {
        robots.disallow = r.body
          .split(/\r?\n/)
          .map((l) => l.trim())
          .filter((l) => /^disallow:/i.test(l))
          .map((l) => l.split(':')[1]?.trim())
          .filter((p): p is string => Boolean(p));
        robots.sitemaps = r.body
          .split(/\r?\n/)
          .filter((l) => /^sitemap:/i.test(l))
          .map((l) => l.split(':').slice(1).join(':').trim())
          .slice(0, 10);
      }
    } catch {
      /* robots.txt is optional */
    }
    try {
      const s = await scopedFetch(target, new URL('/sitemap.xml', url).toString(), { signal, timeoutMs: 8000 });
      sitemap.status = s.status;
      if (s.status === 200) {
        sitemap.urls = [...s.body.matchAll(/<loc>\s*([^<\s]+)\s*<\/loc>/gi)].map((m) => m[1]).slice(0, 200);
      }
    } catch {
      /* sitemap.xml is optional */
    }
    try {
      const st = await scopedFetch(target, new URL('/.well-known/security.txt', url).toString(), { signal, timeoutMs: 6000 });
      securityTxt = st.status === 200 && /contact:/i.test(st.body);
    } catch {
      securityTxt = false;
    }
  }

  const cookies = parseCookies(main?.headers['set-cookie']);
  const technologies = main ? fingerprintTechnologies(main.headers, main.body, cookies.map((c) => c.name)) : [];
  const endpoints = main ? discoverEndpoints(main.body, main.url, host) : [];

  onProgress(70, 'Checking passive DNS / Certificate Transparency sources');
  const subdomains = main ? await crtShSubdomains(host) : [];

  const txt = dns.TXT ?? [];
  const spf = txt.find((t) => t.toLowerCase().startsWith('v=spf1')) ?? null;
  let dmarc: string | null = null;
  try {
    const dmarcRecords = await dnsRecords(`_dmarc.${host}`);
    dmarc = (dmarcRecords.TXT ?? []).find((t) => t.toLowerCase().startsWith('v=dmarc1')) ?? null;
  } catch {
    dmarc = null;
  }

  const data: ReconData = {
    target: target.label,
    url,
    assessedAt: isoNow(),
    dns,
    ips,
    httpStatus: main?.status ?? null,
    finalUrl: main?.url ?? null,
    redirects: main?.redirectChain ?? [],
    server: main?.headers.server ?? null,
    poweredBy: main?.headers['x-powered-by'] ?? null,
    technologies,
    tls: tlsInfo,
    headers: main?.headers ?? {},
    cookies,
    robots,
    sitemap,
    endpoints,
    subdomains,
    emailSecurity: { spf, dmarc },
    securityTxt,
    error,
  };

  onProgress(85, 'Analyzing security configuration');
  writeReconFindings(ctx, data);
  ctx.db.run('INSERT INTO recon_results (id, assessment_id, target, data, created_at) VALUES (?,?,?,?,?)', [
    randomUUID(),
    ctx.assessmentId,
    target.url,
    JSON.stringify(data),
    isoNow(),
  ]);
  return data;
}

function discoverEndpoints(body: string, base: string, host: string): string[] {
  const found = new Set<string>();
  const re = /(?:href|src|action)\s*=\s*["']([^"'#][^"']*)["']/gi;
  for (const m of body.matchAll(re)) {
    try {
      const u = new URL(m[1], base);
      if (u.hostname.toLowerCase() === host.toLowerCase() && ['http:', 'https:'].includes(u.protocol)) {
        u.hash = '';
        found.add(u.toString());
      }
    } catch {
      /* ignore malformed links */
    }
    if (found.size >= 150) break;
  }
  return [...found];
}

/** Passive OSINT: certificate transparency log query (crt.sh). Read-only third-party lookup. */
async function crtShSubdomains(host: string): Promise<string[]> {
  const root = host.split('.').slice(-2).join('.');
  const targetUrl = `https://crt.sh/?q=${encodeURIComponent('%25.' + root)}&output=json`;
  return new Promise((resolve) => {
    const req = https.get(
      targetUrl,
      { headers: { 'User-Agent': 'JAKE-AI/1.0 (certificate transparency lookup)', Accept: 'application/json' }, timeout: 12000 },
      (res) => {
        if (res.statusCode !== 200) {
          res.resume();
          return resolve([]);
        }
        const chunks: Buffer[] = [];
        let size = 0;
        res.on('data', (c: Buffer) => {
          size += c.length;
          if (size < 4_000_000) chunks.push(c);
        });
        res.on('end', () => {
          try {
            const rows = JSON.parse(Buffer.concat(chunks).toString('utf8')) as Array<{ name_value?: string }>;
            const names = new Set<string>();
            for (const r of rows) {
              for (const n of String(r.name_value ?? '').split('\n')) {
                const h = n.trim().toLowerCase().replace(/^\*\./, '');
                if (h.endsWith(`.${root}`) || h === root) names.add(h);
              }
            }
            resolve([...names].sort().slice(0, 100));
          } catch {
            resolve([]);
          }
        });
      },
    );
    req.on('error', () => resolve([]));
    req.on('timeout', () => {
      req.destroy();
      resolve([]);
    });
  });
}

function writeReconFindings(ctx: ScanContext, data: ReconData): void {
  const { assessments, assessmentId, target } = ctx;
  const endpoint = target.url;
  const add = (f: Parameters<Assessments['addFinding']>[2]) => assessments.addFinding(assessmentId, target.url, f);

  for (const h of analyzeHeaders(data.finalUrl ?? data.url, data.headers)) {
    add({
      target: target.url,
      name: h.name,
      category: 'security-misconfiguration',
      severity: h.severity,
      confidence: 'high',
      endpoint,
      evidence: h.evidence,
      description: h.detail,
      impact: h.impact,
      reproduction: `Request ${endpoint} with curl -sI and inspect response headers.`,
      remediation: h.remediation,
      references: [OWASP_HEADERS],
    });
  }
  if (data.tls) {
    if (!data.tls.authorized) {
      add({
        target: target.url,
        name: 'Invalid or untrusted TLS certificate',
        category: 'tls',
        severity: 'high',
        confidence: 'high',
        endpoint,
        evidence: `TLS handshake authorization error: ${data.tls.authorizationError ?? 'unknown'}; subject ${data.tls.subject ?? '?'}; issuer ${data.tls.issuer ?? '?'}`,
        description: 'The server certificate could not be validated against the system trust store.',
        impact: 'Users cannot distinguish the legitimate server from an interception proxy; browser warnings train users to ignore certificate errors.',
        reproduction: 'openssl s_client -connect host:443 -servername host',
        remediation: 'Install a valid certificate from a trusted CA and serve the complete chain.',
        references: ['https://owasp.org/www-project-top-ten/2017/A3_2017-Sensitive_Data_Exposure'],
      });
    }
    if (data.tls.protocol && /TLSv1(\.1)?$/.test(data.tls.protocol)) {
      add({
        target: target.url,
        name: 'Obsolete TLS protocol negotiated',
        category: 'tls',
        severity: 'high',
        confidence: 'high',
        endpoint,
        evidence: `Negotiated ${data.tls.protocol} with cipher ${data.tls.cipher}`,
        description: 'TLS 1.0/1.1 are deprecated and known to be weak.',
        impact: 'Traffic may be decryptable or downgraded by a network attacker.',
        reproduction: 'openssl s_client -tls1_1 -connect host:443',
        remediation: 'Disable TLS 1.0/1.1; require TLS 1.2+ (prefer 1.3).',
        references: ['https://cheatsheetseries.owasp.org/cheatsheets/Transport_Layer_Security_Cheat_Sheet.html'],
      });
    }
    if (data.tls.daysRemaining !== null && data.tls.daysRemaining < 30) {
      add({
        target: target.url,
        name: data.tls.daysRemaining < 0 ? 'TLS certificate expired' : 'TLS certificate expiring soon',
        category: 'tls',
        severity: data.tls.daysRemaining < 0 ? 'critical' : 'medium',
        confidence: 'high',
        endpoint,
        evidence: `Certificate valid to ${data.tls.validTo} (${data.tls.daysRemaining} days remaining)`,
        description: 'Certificate lifetime is insufficient.',
        impact: 'Clients will reject the connection when the certificate expires.',
        reproduction: 'openssl s_client -connect host:443 | openssl x509 -noout -dates',
        remediation: 'Renew the certificate and automate renewal with monitoring.',
        references: ['https://cheatsheetseries.owasp.org/cheatsheets/Transport_Layer_Security_Cheat_Sheet.html'],
      });
    }
  }
  for (const c of data.cookies) {
    if (data.url.startsWith('https:') && !c.secure) {
      add({
        target: target.url,
        name: `Cookie "${c.name}" without Secure flag`,
        category: 'session-management',
        severity: 'medium',
        confidence: 'high',
        endpoint,
        evidence: `Set-Cookie ${c.name}: Secure=false HttpOnly=${c.httpOnly} SameSite=${c.sameSite ?? 'unset'}`,
        description: 'A session cookie can be transmitted over cleartext HTTP.',
        impact: 'Session hijacking on mixed-content or downgraded connections.',
        reproduction: `curl -sI ${endpoint} and inspect Set-Cookie.`,
        remediation: 'Add the Secure attribute (and HttpOnly) to all session cookies.',
        references: ['https://owasp.org/www-project-top-ten/2017/A2_2017-Broken_Authentication'],
      });
    }
    if (!c.httpOnly) {
      add({
        target: target.url,
        name: `Cookie "${c.name}" without HttpOnly flag`,
        category: 'session-management',
        severity: 'low',
        confidence: 'high',
        endpoint,
        evidence: `Set-Cookie ${c.name}: HttpOnly=false`,
        description: 'Cookie value is readable from JavaScript.',
        impact: 'Any XSS can steal the session token.',
        reproduction: `curl -sI ${endpoint} and inspect Set-Cookie.`,
        remediation: 'Set HttpOnly on session and authentication cookies.',
        references: ['https://cheatsheetseries.owasp.org/cheatsheets/Session_Management_Cheat_Sheet.html'],
      });
    }
  }
  if (data.subdomains.length > 0) {
    add({
      target: target.url,
      name: 'Public subdomains visible in Certificate Transparency',
      category: 'reconnaissance',
      severity: 'info',
      confidence: 'high',
      endpoint,
      evidence: data.subdomains.slice(0, 60).join(', '),
      description: `${data.subdomains.length} hostnames were found through public certificate transparency logs.`,
      impact: 'Exposes additional attack surface; verify each host is patched and in scope before testing.',
      reproduction: `https://crt.sh/?q=%25.${target.url.replace(/^https?:\/\//, '').split('/')[0]}`,
      remediation: 'Inventory public hostnames, retire unused ones, and monitor newly issued certificates.',
      references: ['https://owasp.org/www-community/attacks/Information_exposure'],
    });
  }
  if (data.emailSecurity.spf === null && data.ips.length > 0) {
    add({
      target: target.url,
      name: 'SPF record missing',
      category: 'security-misconfiguration',
      severity: 'low',
      confidence: 'medium',
      endpoint,
      evidence: 'No TXT record starting with v=spf1',
      description: 'The domain does not publish a Sender Policy Framework record.',
      impact: 'Email spoofing of the domain is easier.',
      reproduction: `dig TXT ${target.url.replace(/^https?:\/\//, '').split('/')[0]}`,
      remediation: 'Publish a restrictive SPF record and pair it with DKIM and DMARC.',
      references: ['https://owasp.org/www-community/attacks/Email_spoofing'],
    });
  }
  if (data.emailSecurity.dmarc === null && data.ips.length > 0) {
    add({
      target: target.url,
      name: 'DMARC record missing',
      category: 'security-misconfiguration',
      severity: 'low',
      confidence: 'medium',
      endpoint,
      evidence: 'No v=DMARC1 TXT record at _dmarc',
      description: 'No DMARC policy is published for the domain.',
      impact: 'Receivers cannot enforce rejection of spoofed mail.',
      reproduction: `dig TXT _dmarc.${target.url.replace(/^https?:\/\//, '').split('/')[0]}`,
      remediation: 'Publish _dmarc TXT with p=quarantine or p=reject after monitoring.',
      references: ['https://dmarc.org/overview/'],
    });
  }
}
