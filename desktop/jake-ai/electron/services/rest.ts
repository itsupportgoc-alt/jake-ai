import type { ConnectionProfile, Target } from '../../shared/types';
import { authHeaders, type ResolvedConnection } from './connections';
import { scopedFetch, type FetchResult } from './scope';

export interface RestContext {
  profile: ResolvedConnection;
  target: Target;
}

export function restContext(profile: ResolvedConnection): RestContext {
  if (!profile.target) throw new Error('This connection profile has no authorized target. Edit the profile and select one.');
  return { profile, target: profile.target };
}

/**
 * Authenticated request against an authorized target.
 * Credentials come from the encrypted profile and are never logged or sent to the AI.
 */
export async function restRequest(
  ctx: RestContext,
  path: string,
  opts: { auth?: 'admin' | 'viewer' | 'none'; method?: string; body?: string; headers?: Record<string, string>; signal?: AbortSignal; maxBytes?: number } = {},
): Promise<FetchResult> {
  const base = ctx.profile.base_url || ctx.target.url;
  const url = new URL(path, base.endsWith('/') ? base : `${base}/`).toString();
  const auth = opts.auth ?? 'admin';
  const authCfg = auth === 'viewer' ? ctx.profile.config.viewerAuth : auth === 'none' ? undefined : ctx.profile.config.auth;
  const headers: Record<string, string> = {
    Accept: 'application/json, text/html;q=0.9, */*;q=0.1',
    ...authHeaders(authCfg),
    ...(opts.headers ?? {}),
  };
  if (opts.body) headers['Content-Type'] = 'application/json';
  return scopedFetch(ctx.target, url, {
    method: opts.method ?? 'GET',
    headers,
    body: opts.body,
    signal: opts.signal,
    timeoutMs: 15000,
    maxBytes: opts.maxBytes ?? 1_000_000,
  });
}

export function parseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

/** Resolve a dotted path such as "data.users" and return the value. */
export function getPath(value: unknown, path: string | undefined): unknown {
  if (!path) return value;
  return path.split('.').reduce<unknown>((acc, key) => {
    if (acc && typeof acc === 'object') {
      const obj = acc as Record<string, unknown>;
      if (key in obj) return obj[key];
      if (Array.isArray(acc) && /^\d+$/.test(key)) return acc[Number(key)];
    }
    return undefined;
  }, value);
}

/** Find the first array of objects in a JSON response (common admin API shapes). */
export function findRecordArray(value: unknown, depth = 0): Record<string, unknown>[] {
  if (depth > 4 || !value) return [];
  if (Array.isArray(value)) {
    const objs = value.filter((v) => v && typeof v === 'object' && !Array.isArray(v)) as Record<string, unknown>[];
    if (objs.length > 0) return objs;
    for (const v of value) {
      const nested = findRecordArray(v, depth + 1);
      if (nested.length) return nested;
    }
    return [];
  }
  if (typeof value === 'object') {
    for (const v of Object.values(value as Record<string, unknown>)) {
      const nested = findRecordArray(v, depth + 1);
      if (nested.length) return nested;
    }
  }
  return [];
}

export function pickField(obj: Record<string, unknown>, candidates: string[]): { key: string; value: unknown } | null {
  const lower = new Map(Object.keys(obj).map((k) => [k.toLowerCase(), k]));
  for (const c of candidates) {
    const real = lower.get(c.toLowerCase());
    if (real) return { key: real, value: obj[real] };
  }
  return null;
}

export function toBool(v: unknown): boolean | null {
  if (typeof v === 'boolean') return v;
  if (typeof v === 'number') return v !== 0;
  if (typeof v === 'string') {
    const s = v.toLowerCase();
    if (['true', '1', 'yes', 'active', 'enabled', 'approved'].includes(s)) return true;
    if (['false', '0', 'no', 'inactive', 'disabled', 'suspended', 'locked'].includes(s)) return false;
  }
  return null;
}

export function looksLikeLoginPage(res: FetchResult): boolean {
  return /<input[^>]+type=["']?password/i.test(res.body) || /login|sign in/i.test(res.headers['content-type'] ?? '');
}

export function isSuccess(status: number): boolean {
  return status >= 200 && status < 300;
}
