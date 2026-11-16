import dns from 'node:dns/promises';
import net from 'node:net';
import http from 'node:http';
import https from 'node:https';
import tls from 'node:tls';
import type { Target } from '../../shared/types';

/**
 * Target authorization and scope enforcement.
 * Requests must stay inside the registered target's allowlist, private ranges
 * are blocked unless the target explicitly permits them, and rate limits /
 * redirects / time windows are enforced centrally.
 */
export class ScopeError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ScopeError';
  }
}

const PRIVATE_V4 = [
  ['10.0.0.0', 8],
  ['172.16.0.0', 12],
  ['192.168.0.0', 16],
  ['127.0.0.0', 8],
  ['169.254.0.0', 16],
  ['0.0.0.0', 8],
] as const;

export function parseTarget(row: Record<string, unknown>): Target {
  return {
    id: String(row.id),
    label: String(row.label),
    url: String(row.url),
    kind: row.kind as Target['kind'],
    authorization_note: String(row.authorization_note ?? ''),
    scope_patterns: String(row.scope_patterns ?? '[]'),
    rate_limit: Number(row.rate_limit ?? 2),
    allow_private: Number(row.allow_private ?? 0),
    created_at: String(row.created_at),
  };
}

export function targetHosts(target: Target): string[] {
  const hosts = new Set<string>();
  try {
    hosts.add(new URL(target.url).hostname.toLowerCase());
  } catch {
    hosts.add(target.url.toLowerCase());
  }
  try {
    const extra = JSON.parse(target.scope_patterns || '[]') as unknown;
    if (Array.isArray(extra)) extra.filter((h) => typeof h === 'string').forEach((h) => hosts.add(String(h).toLowerCase()));
  } catch {
    /* ignore malformed allowlist */
  }
  return [...hosts];
}

export function isPrivateAddress(ip: string): boolean {
  if (net.isIPv4(ip)) {
    const num = ip.split('.').reduce((acc, p) => (acc << 8) + Number(p), 0) >>> 0;
    return PRIVATE_V4.some(([base, bits]) => {
      const b = base.split('.').reduce((acc, p) => (acc << 8) + Number(p), 0) >>> 0;
      const mask = (0xffffffff << (32 - bits)) >>> 0;
      return (num & mask) === (b & mask);
    });
  }
  if (net.isIPv6(ip)) {
    const v = ip.toLowerCase();
    return v === '::1' || v.startsWith('fc') || v.startsWith('fd') || v.startsWith('fe80');
  }
  return false;
}

export function hostMatches(host: string, pattern: string): boolean {
  host = host.toLowerCase().replace(/\.$/, '');
  pattern = pattern.toLowerCase().replace(/\.$/, '');
  if (pattern === host) return true;
  if (pattern.startsWith('*.')) {
    const suffix = pattern.slice(1);
    return host.endsWith(suffix) && host.length > suffix.length;
  }
  return false;
}

export async function assertUrlInScope(target: Target, rawUrl: string): Promise<URL> {
  let url: URL;
  try {
    url = new URL(rawUrl);
  } catch {
    throw new ScopeError(`Not a valid absolute URL: ${rawUrl}`);
  }
  if (!['http:', 'https:'].includes(url.protocol)) throw new ScopeError(`Protocol ${url.protocol} is not allowed`);
  const host = url.hostname.toLowerCase();
  const patterns = targetHosts(target);
  if (!patterns.some((p) => hostMatches(host, p))) {
    throw new ScopeError(
      `Scope violation: ${host} is not in the authorized scope for "${target.label}" (${patterns.join(', ')}). Register the host and its authorization first.`,
    );
  }
  let addresses: string[] = [];
  if (net.isIP(host)) addresses = [host];
  else {
    try {
      addresses = (await dns.lookup(host, { all: true })).map((a) => a.address);
    } catch (err) {
      throw new ScopeError(`DNS resolution failed for ${host}: ${(err as Error).message}`);
    }
  }
  if (!target.allow_private) {
    const priv = addresses.find(isPrivateAddress);
    if (priv) {
      throw new ScopeError(
        `${host} resolves to private/loopback address ${priv}. This target must enable "Allow private/local network" in Target Management with verified authorization.`,
      );
    }
  }
  return url;
}

/** Per-target token bucket, shared by every scan module. */
class RateLimiter {
  private last = new Map<string, number>();
  async take(key: string, perSecond: number): Promise<void> {
    const minGap = 1000 / Math.max(0.1, perSecond);
    const now = Date.now();
    const prev = this.last.get(key) ?? 0;
    const wait = prev + minGap - now;
    if (wait > 0) await new Promise((r) => setTimeout(r, wait));
    this.last.set(key, Date.now());
  }
}

const limiter = new RateLimiter();

export interface TlsInfo {
  protocol: string | null;
  cipher: string | null;
  authorized: boolean;
  authorizationError: string | null;
  subject: string | null;
  issuer: string | null;
  validFrom: string | null;
  validTo: string | null;
  daysRemaining: number | null;
  altNames: string[];
}

export interface FetchResult {
  url: string;
  status: number;
  headers: Record<string, string>;
  body: string;
  bodyBytes: number;
  truncated: boolean;
  elapsedMs: number;
  redirectChain: string[];
  tls: TlsInfo | null;
}

const ALLOWED_HEADER = /^[A-Za-z0-9-]{1,64}$/;

function certString(v: unknown): string | null {
  return typeof v === 'string' ? v : null;
}

export function describeTls(sock: tls.TLSSocket): TlsInfo {
  const cert = sock.getPeerCertificate(true);
  const validTo = cert?.valid_to ? new Date(cert.valid_to) : null;
  return {
    protocol: sock.getProtocol?.() ?? null,
    cipher: sock.getCipher?.()?.name ?? null,
    authorized: sock.authorized,
    authorizationError: sock.authorizationError ? String(sock.authorizationError) : null,
    subject: certString((cert?.subject as Record<string, unknown> | undefined)?.CN) ?? null,
    issuer: certString((cert?.issuer as Record<string, unknown> | undefined)?.O) ?? null,
    validFrom: cert?.valid_from ? new Date(cert.valid_from).toISOString() : null,
    validTo: validTo ? validTo.toISOString() : null,
    daysRemaining: validTo ? Math.round((validTo.getTime() - Date.now()) / 86_400_000) : null,
    altNames: (cert?.subjectaltname ?? '')
      .split(',')
      .map((s) => s.trim().replace(/^DNS:/, ''))
      .filter(Boolean)
      .slice(0, 50),
  };
}

interface RawResponse {
  status: number;
  headers: Record<string, string>;
  body: string;
  bodyBytes: number;
  truncated: boolean;
  location: string | null;
  tls: TlsInfo | null;
}

function rawRequest(
  url: URL,
  method: string,
  headers: Record<string, string>,
  body: string | undefined,
  timeoutMs: number,
  maxBytes: number,
  signal: AbortSignal | undefined,
): Promise<RawResponse> {
  return new Promise((resolve, reject) => {
    const isHttps = url.protocol === 'https:';
    const mod = isHttps ? https : http;
    const req = mod.request(
      url,
      {
        method,
        headers,
        timeout: timeoutMs,
        // Assessment targets are authorized; invalid certificates are recorded
        // as a finding instead of silently blocking the scan.
        rejectUnauthorized: false,
        servername: isHttps ? url.hostname : undefined,
      },
      (res) => {
        const chunks: Buffer[] = [];
        let total = 0;
        let truncated = false;
        res.on('data', (c: Buffer) => {
          total += c.length;
          if (total <= maxBytes) chunks.push(c);
          else truncated = true;
        });
        res.on('end', () => {
          const outHeaders: Record<string, string> = {};
          for (const [k, v] of Object.entries(res.headers)) {
            // set-cookie must stay multi-line: commas also appear inside Expires.
            outHeaders[k.toLowerCase()] = Array.isArray(v) ? v.join(k.toLowerCase() === 'set-cookie' ? '\n' : ', ') : String(v ?? '');
          }
          const sock = res.socket;
          const tlsInfo = sock && 'getPeerCertificate' in sock ? describeTls(sock as tls.TLSSocket) : null;
          resolve({
            status: res.statusCode ?? 0,
            headers: outHeaders,
            body: Buffer.concat(chunks).toString('utf8'),
            bodyBytes: total,
            truncated,
            location: outHeaders.location ?? null,
            tls: tlsInfo,
          });
        });
      },
    );
    req.on('timeout', () => req.destroy(new Error(`Request timed out after ${timeoutMs} ms`)));
    req.on('error', (err) => reject(err));
    const onAbort = () => req.destroy(new Error('Operation cancelled'));
    signal?.addEventListener('abort', onAbort, { once: true });
    req.on('close', () => signal?.removeEventListener('abort', onAbort));
    if (body) req.write(body);
    req.end();
  });
}

export async function scopedFetch(
  target: Target,
  rawUrl: string,
  init: { method?: string; headers?: Record<string, string>; body?: string; timeoutMs?: number; maxBytes?: number; signal?: AbortSignal; maxRedirects?: number } = {},
): Promise<FetchResult> {
  const signal = init.signal;
  if (signal?.aborted) throw new Error('Operation cancelled');
  const started = Date.now();
  const maxRedirects = init.maxRedirects ?? 3;
  const maxBytes = init.maxBytes ?? 1_000_000;
  const timeoutMs = init.timeoutMs ?? 15000;
  let url = await assertUrlInScope(target, rawUrl);
  const chain: string[] = [];
  let method = (init.method ?? 'GET').toUpperCase();
  let body = init.body;
  let headers: Record<string, string> = {
    'User-Agent': 'JAKE-AI-Security-Assessment/1.0 (authorized testing)',
    Accept: '*/*',
    ...(init.headers ?? {}),
  };
  for (const [k, v] of Object.entries(headers)) {
    if (!ALLOWED_HEADER.test(k) || /[\r\n]/.test(v)) throw new Error(`Invalid header ${k}`);
  }

  for (let hop = 0; hop <= maxRedirects; hop++) {
    await limiter.take(target.id, target.rate_limit);
    if (signal?.aborted) throw new Error('Operation cancelled');
    const res = await rawRequest(url, method, headers, body, timeoutMs, maxBytes, signal);
    if ([301, 302, 303, 307, 308].includes(res.status)) {
      if (!res.location) throw new Error(`Redirect ${res.status} without Location header`);
      const next = new URL(res.location, url).toString();
      chain.push(next);
      // Re-validates scope on every hop: redirects cannot leave the allowlist.
      url = await assertUrlInScope(target, next);
      if (res.status === 303 || ((res.status === 301 || res.status === 302) && method === 'POST')) {
        method = 'GET';
        body = undefined;
        delete headers['Content-Type'];
      }
      continue;
    }
    return {
      url: url.toString(),
      status: res.status,
      headers: res.headers,
      body: res.body,
      bodyBytes: res.bodyBytes,
      truncated: res.truncated,
      elapsedMs: Date.now() - started,
      redirectChain: chain,
      tls: res.tls,
    };
  }
  throw new ScopeError(`Too many redirects (>${maxRedirects}); stopped to keep testing inside scope`);
}

export async function dnsRecords(hostname: string): Promise<Record<string, string[]>> {
  const out: Record<string, string[]> = {};
  const queries: Array<[string, () => Promise<unknown>]> = [
    ['A', () => dns.resolve4(hostname)],
    ['AAAA', () => dns.resolve6(hostname)],
    ['CNAME', () => dns.resolveCname(hostname)],
    ['MX', () => dns.resolveMx(hostname).then((r) => r.map((x) => `${x.exchange} (${x.priority})`))],
    ['TXT', () => dns.resolveTxt(hostname).then((r) => r.map((x) => x.join('')))],
    ['NS', () => dns.resolveNs(hostname)],
  ];
  await Promise.all(
    queries.map(async ([name, fn]) => {
      try {
        out[name] = (await fn()) as string[];
      } catch {
        out[name] = [];
      }
    }),
  );
  return out;
}
