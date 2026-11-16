import { randomUUID } from 'node:crypto';
import type { Severity } from '../../shared/types';
import type { Database } from './db';
import { isoNow } from './db';
import type { Assessments } from './assessments';
import type { Connections } from './connections';
import { restRequest, restContext, looksLikeLoginPage, isSuccess } from './rest';

export interface RbacRow {
  name: string;
  method: string;
  path: string;
  viewerAllowed: boolean;
  unauthenticatedStatus: number | null;
  viewerStatus: number | null;
  adminStatus: number | null;
  verdict: 'ok' | 'exposed' | 'viewer-escalation' | 'auth-misconfigured' | 'inconclusive';
  detail: string;
}

export interface RbacResult {
  rows: RbacRow[];
  summary: string;
  notes: string[];
}

const SAFE_METHODS = new Set(['GET', 'HEAD', 'OPTIONS']);

export async function runRbacAudit(opts: {
  db: Database;
  assessments: Assessments;
  connections: Connections;
  connectionId: string;
  assessmentId: string;
  signal: AbortSignal;
  onProgress: (p: number, msg: string) => void;
}): Promise<RbacResult> {
  const { db, assessments, connections, connectionId, assessmentId, signal, onProgress } = opts;
  const profile = connections.get(connectionId);
  const ctx = restContext(profile);
  const endpoints = (profile.config.endpoints ?? []).filter((e) => e.path);
  if (endpoints.length === 0) throw new Error('Add at least one endpoint to this connection profile before running the RBAC audit.');
  const viewer = profile.config.viewerAuth;
  const hasViewerCredential = Boolean(viewer && viewer.type !== 'none' && (viewer.value || viewer.username));
  if (!hasViewerCredential) {
    throw new Error(
      'RBAC testing needs a dedicated low-privilege Viewer credential in this profile. Without it, "Viewer" requests would be unauthenticated and the comparison would be invalid.',
    );
  }
  const notes: string[] = [];
  const rows: RbacRow[] = [];

  for (let i = 0; i < endpoints.length; i++) {
    if (signal.aborted) throw new Error('Operation cancelled');
    const ep = endpoints[i];
    const method = (ep.method || 'GET').toUpperCase();
    if (!SAFE_METHODS.has(method)) {
      notes.push(`${ep.name || ep.path}: ${method} skipped — write methods are never sent by the RBAC auditor (would modify data).`);
      continue;
    }
    onProgress(Math.round((i / endpoints.length) * 90) + 5, `Testing ${ep.name || ep.path}`);

    const un = await restRequest(ctx, ep.path, { auth: 'none', method, signal }).catch(() => null);
    const viewer = await restRequest(ctx, ep.path, { auth: 'viewer', method, signal }).catch(() => null);
    const admin = await restRequest(ctx, ep.path, { auth: 'admin', method, signal }).catch(() => null);

    // A deliberately public endpoint is not an exposure.
    const unauthExposed = ep.requiresAuth !== false && un && isSuccess(un.status) && !looksLikeLoginPage(un) && un.body.length > 0;
    const viewerExposed = viewer && isSuccess(viewer.status) && !looksLikeLoginPage(viewer) && viewer.body.length > 0;
    const adminDenied = admin && (admin.status === 401 || admin.status === 403);
    const expected = ep.viewerAllowed === true;

    let verdict: RbacRow['verdict'] = 'ok';
    let detail = '';
    if (adminDenied) {
      verdict = 'auth-misconfigured';
      detail = `The admin credential was rejected (HTTP ${admin?.status}) on this endpoint; expectations cannot be evaluated.`;
    } else if (unauthExposed) {
      verdict = 'exposed';
      detail = `Unauthenticated request succeeded with HTTP ${un?.status}. This endpoint does not enforce authentication.`;
    } else if (!expected && viewerExposed) {
      verdict = 'viewer-escalation';
      detail = `A Viewer/test account received HTTP ${viewer?.status}. Expected denial (401/403). Backend authorization is missing or too permissive.`;
    } else if (viewer && (viewer.status === 401 || viewer.status === 403)) {
      detail = `Viewer correctly denied (HTTP ${viewer.status}).`;
    } else if (!viewer) {
      verdict = 'inconclusive';
      detail = 'Viewer request failed at the transport level. Check the connection profile.';
    } else {
      detail = `Viewer HTTP ${viewer.status}; matches expectation (allowed=${expected}).`;
    }

    rows.push({
      name: ep.name || ep.path,
      method,
      path: ep.path,
      viewerAllowed: expected,
      unauthenticatedStatus: un?.status ?? null,
      viewerStatus: viewer?.status ?? null,
      adminStatus: admin?.status ?? null,
      verdict,
      detail,
    });

    if (verdict === 'exposed' || verdict === 'viewer-escalation') {
      const severity: Severity = verdict === 'exposed' ? 'critical' : expected ? 'info' : 'high';
      const name = verdict === 'exposed' ? 'API endpoint accessible without authentication' : 'Vertical access control bypass (Viewer)';
      const exists = db.get('SELECT id FROM findings WHERE assessment_id = ? AND name = ? AND endpoint = ?', [
        assessmentId,
        name,
        new URL(ep.path, profile.base_url || ctx.target.url).toString(),
      ]);
      if (!exists) {
        assessments.addFinding(assessmentId, ctx.target.url, {
          target: ctx.target.url,
          name,
          category: verdict === 'exposed' ? 'access-control' : 'rbac',
          severity,
          confidence: 'high',
          endpoint: new URL(ep.path, profile.base_url || ctx.target.url).toString(),
          evidence: `Unauthenticated: HTTP ${un?.status ?? 'n/a'}; Viewer: HTTP ${viewer?.status ?? 'n/a'} (${viewer?.bodyBytes ?? 0} bytes); Admin: HTTP ${admin?.status ?? 'n/a'}. ${detail}`,
          description:
            verdict === 'exposed'
              ? 'The server returns data for this route without any credentials.'
              : 'A low-privilege test account can reach a route that is supposed to be restricted. Hiding UI elements does not protect the backend.',
          impact: 'Unauthorized users can read or act on data that should require a higher privilege level.',
          reproduction: `With an authorized test account, request ${method} ${ep.path} and compare the response to an administrator session.`,
          remediation: 'Enforce authorization server-side on every route (role/permission middleware), default-deny, and add automated tests for each role.',
          references: ['https://cheatsheetseries.owasp.org/cheatsheets/Authorization_Cheat_Sheet.html', 'https://owasp.org/www-project-top-ten/2017/A5_2017-Broken_Access_Control'],
        });
      }
    }
  }

  const exposed = rows.filter((r) => r.verdict === 'exposed').length;
  const escalated = rows.filter((r) => r.verdict === 'viewer-escalation').length;
  const summary = `${rows.length} routes tested: ${exposed} unauthenticated exposure(s), ${escalated} viewer privilege escalation(s).`;
  db.run('INSERT INTO rbac_results (id, assessment_id, profile, summary, matrix, created_at) VALUES (?,?,?,?,?,?)', [
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
