import { randomUUID } from 'node:crypto';
import type { Severity } from '../../shared/types';
import type { Database } from './db';
import { isoNow } from './db';
import type { Assessments } from './assessments';
import type { Connections } from './connections';
import { restRequest, restContext, parseJson, isSuccess } from './rest';

export interface ApiCheckRow {
  name: string;
  method: string;
  path: string;
  unauthenticatedStatus: number | null;
  authenticatedStatus: number | null;
  issues: string[];
}

export interface ApiScanResult {
  rows: ApiCheckRow[];
  summary: string;
  notes: string[];
}

const SENSITIVE_KEYS = /^(password|passwd|pwd|password_hash|hash|secret|api_key|apikey|access_token|refresh_token|private_key|ssn|credit_card|card_number|session_id)$/i;

function sensitivePaths(value: unknown, prefix = '', depth = 0): string[] {
  if (depth > 6 || !value) return [];
  if (Array.isArray(value)) return value.slice(0, 3).flatMap((v, i) => sensitivePaths(v, `${prefix}[${i}]`, depth + 1));
  if (typeof value === 'object') {
    return Object.entries(value as Record<string, unknown>).flatMap(([k, v]) => {
      const next = prefix ? `${prefix}.${k}` : k;
      if (SENSITIVE_KEYS.test(k)) return [next];
      return sensitivePaths(v, next, depth + 1);
    });
  }
  return [];
}

export async function runApiScan(opts: {
  db: Database;
  assessments: Assessments;
  connections: Connections;
  connectionId: string;
  assessmentId: string;
  signal: AbortSignal;
  onProgress: (p: number, msg: string) => void;
}): Promise<ApiScanResult> {
  const { db, assessments, connections, connectionId, assessmentId, signal, onProgress } = opts;
  const profile = connections.get(connectionId);
  const ctx = restContext(profile);
  const endpoints = profile.config.endpoints ?? [];
  if (endpoints.length === 0) throw new Error('Add at least one endpoint to this API connection profile first.');
  const notes: string[] = [];
  const rows: ApiCheckRow[] = [];
  const add = (f: { name: string; severity: Severity; confidence: 'low' | 'medium' | 'high'; endpoint: string; evidence: string; description: string; impact: string; reproduction: string; remediation: string; references: string[] }) => {
    if (db.get('SELECT id FROM findings WHERE assessment_id = ? AND name = ? AND endpoint = ?', [assessmentId, f.name, f.endpoint])) return;
    assessments.addFinding(assessmentId, ctx.target.url, { target: ctx.target.url, category: 'api', ...f });
  };

  for (let i = 0; i < endpoints.length; i++) {
    if (signal.aborted) throw new Error('Operation cancelled');
    const ep = endpoints[i];
    const method = (ep.method || 'GET').toUpperCase();
    onProgress(5 + Math.round((i / endpoints.length) * 60), `Checking ${method} ${ep.path}`);
    const url = new URL(ep.path, profile.base_url || ctx.target.url).toString();
    const issues: string[] = [];

    const unauth = method === 'GET' && ep.requiresAuth !== false ? await restRequest(ctx, ep.path, { auth: 'none', signal }).catch(() => null) : null;
    const authed = await restRequest(ctx, ep.path, { auth: 'admin', method, signal, body: method === 'POST' ? '{}' : undefined }).catch(() => null);

    if (unauth && isSuccess(unauth.status) && unauth.body.length > 2) {
      issues.push(`Unauthenticated request succeeded (HTTP ${unauth.status}).`);
      add({
        name: 'API endpoint lacks authentication',
        severity: 'critical',
        confidence: 'high',
        endpoint: url,
        evidence: `HTTP ${unauth.status} without credentials; ${unauth.bodyBytes} bytes returned.`,
        description: 'The endpoint returned data to an unauthenticated client.',
        impact: 'Anyone who can reach the API can read this resource.',
        reproduction: `curl -s ${url}`,
        remediation: 'Require authentication on the route and default-deny unauthenticated access at the API gateway.',
        references: ['https://owasp.org/www-project-api-security/', 'https://cheatsheetseries.owasp.org/cheatsheets/Authorization_Cheat_Sheet.html'],
      });
    }

    if (authed) {
      const json = parseJson(authed.body);
      const leaks = sensitivePaths(json);
      if (leaks.length) {
        issues.push(`Sensitive fields in response: ${[...new Set(leaks)].join(', ')}`);
        add({
          name: 'API response exposes sensitive fields',
          severity: 'high',
          confidence: 'high',
          endpoint: url,
          evidence: `Field paths (values redacted): ${[...new Set(leaks)].slice(0, 12).join(', ')}`,
          description: 'The serialized response includes credential, token or personal data fields.',
          impact: 'Data can be harvested by any client permitted to call the endpoint, and leaks into logs/browser history.',
          reproduction: `Call ${method} ${url} with authorized credentials and inspect the JSON keys.`,
          remediation: 'Use explicit response DTOs/serializers that omit secret and unnecessary personal fields.',
          references: ['https://owasp.org/www-project-api-security/'],
        });
      }
      if (authed.headers['access-control-allow-origin'] === '*') {
        issues.push('CORS wildcard.');
        add({
          name: 'API allows any CORS origin',
          severity: 'medium',
          confidence: 'medium',
          endpoint: url,
          evidence: 'Access-Control-Allow-Origin: *',
          description: 'Any origin can read this API response.',
          impact: 'Cross-origin data exposure if responses are not strictly public.',
          reproduction: `curl -sI -H "Origin: https://example.invalid" ${url}`,
          remediation: 'Allowlist trusted origins in CORS configuration.',
          references: ['https://cheatsheetseries.owasp.org/cheatsheets/Cross-Origin_Resource_Sharing_Cheat_Sheet.html'],
        });
      }
      const h = authed.headers;
      const missingHeaders = ['x-content-type-options', 'strict-transport-security', 'cache-control'].filter((x) => !h[x]);
      if (missingHeaders.length) issues.push(`Missing headers: ${missingHeaders.join(', ')}`);
      if (!/no-store|no-cache/i.test(h['cache-control'] ?? '') && /json/i.test(h['content-type'] ?? '')) {
        add({
          name: 'API response may be cached',
          severity: 'low',
          confidence: 'low',
          endpoint: url,
          evidence: `Cache-Control: ${h['cache-control'] ?? '(absent)'}`,
          description: 'Authenticated JSON responses do not set an explicit no-store policy.',
          impact: 'Sensitive responses may persist in shared caches or browser history.',
          reproduction: `curl -sI ${url}`,
          remediation: 'Send Cache-Control: no-store on authenticated API responses.',
          references: ['https://cheatsheetseries.owasp.org/cheatsheets/REST_Security_Cheat_Sheet.html'],
        });
      }
    }

    if (authed) {
      const allow = authed.headers.allow;
      if (allow && /\b(TRACE|PUT|DELETE)\b/i.test(allow)) {
        issues.push(`Methods advertised: ${allow}`);
        add({
          name: 'Unexpected HTTP methods advertised on API route',
          severity: 'low',
          confidence: 'medium',
          endpoint: url,
          evidence: `Allow: ${allow}`,
          description: 'The route advertises write/verb methods that may not be intended.',
          impact: 'Expands the attack surface if authorization is incomplete.',
          reproduction: `curl -sI -X OPTIONS ${url}`,
          remediation: 'Advertise and accept only the methods each route implements.',
          references: ['https://cheatsheetseries.owasp.org/cheatsheets/REST_Security_Cheat_Sheet.html'],
        });
      }
    }

    rows.push({
      name: ep.name || ep.path,
      method,
      path: ep.path,
      unauthenticatedStatus: unauth?.status ?? null,
      authenticatedStatus: authed?.status ?? null,
      issues,
    });
  }

  // Rate limiting on the first GET endpoint.
  const firstGet = endpoints.find((e) => (e.method || 'GET').toUpperCase() === 'GET');
  if (firstGet) {
    onProgress(75, 'Sampling rate-limit behaviour (15 requests)');
    let throttled = 0;
    let errors = 0;
    for (let i = 0; i < 15; i++) {
      if (signal.aborted) break;
      const res = await restRequest(ctx, firstGet.path, { signal }).catch(() => null);
      if (!res) errors++;
      else if (res.status === 429 || res.status === 503) throttled++;
    }
    if (throttled === 0 && errors === 0) {
      notes.push('No HTTP 429 observed in a 15-request burst. Rate limiting may be absent or configured at another layer (WAF/CDN).');
      add({
        name: 'No rate limiting observed on API endpoint',
        severity: 'low',
        confidence: 'low',
        endpoint: new URL(firstGet.path, profile.base_url || ctx.target.url).toString(),
        evidence: '15 sequential authenticated requests produced no HTTP 429 or 503 response.',
        description: 'A short burst was not throttled. A CDN/WAF in front of the app could still enforce limits, so confidence is low.',
        impact: 'If unthrottled in production, endpoints are open to brute force and resource exhaustion.',
        reproduction: 'Send a short burst of requests and observe status codes; stop if the service degrades.',
        remediation: 'Apply per-identity and per-IP rate limits with 429 responses and monitoring.',
        references: ['https://owasp.org/www-project-api-security/'],
      });
    } else if (throttled > 0) {
      notes.push(`Rate limiting observed: ${throttled}/15 requests throttled.`);
    }
  }

  // API documentation exposure.
  onProgress(88, 'Checking for exposed API documentation');
  for (const p of ['/openapi.json', '/swagger.json', '/api-docs', '/v3/api-docs', '/.well-known/openapi.json']) {
    const res = await restRequest(ctx, p, { auth: 'none', signal }).catch(() => null);
    if (res && res.status === 200 && /("openapi"|"swagger"|"paths")/.test(res.body)) {
      add({
        name: 'API documentation publicly exposed',
        severity: 'low',
        confidence: 'high',
        endpoint: new URL(p, profile.base_url || ctx.target.url).toString(),
        evidence: `HTTP 200 with an OpenAPI/Swagger document (${res.bodyBytes} bytes).`,
        description: 'Machine-readable API documentation is reachable without authentication.',
        impact: 'Gives attackers a complete route and parameter map.',
        reproduction: `curl -s ${new URL(p, profile.base_url || ctx.target.url)} | head`,
        remediation: 'Serve API docs only to authenticated developers or restrict by network.',
        references: ['https://owasp.org/www-project-api-security/'],
      });
      break;
    }
  }

  const summary = `${rows.length} endpoints tested, ${rows.reduce((n, r) => n + r.issues.length, 0)} observations.`;
  db.run('INSERT INTO api_results (id, assessment_id, profile, summary, data, created_at) VALUES (?,?,?,?,?,?)', [
    randomUUID(),
    assessmentId,
    profile.name,
    summary,
    JSON.stringify({ rows, notes }),
    isoNow(),
  ]);
  onProgress(100, summary);
  return { rows, summary, notes };
}
