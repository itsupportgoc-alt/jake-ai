import { randomUUID } from 'node:crypto';
import type { Severity } from '../../shared/types';
import type { Database } from './db';
import { isoNow } from './db';
import type { Assessments } from './assessments';
import type { Connections } from './connections';
import { restRequest, restContext, parseJson, getPath, findRecordArray, pickField, toBool } from './rest';
import { maskEmail, redact } from './redact';

export interface AccountIssue {
  severity: Severity;
  name: string;
  detail: string;
  accounts: string[];
  remediation: string;
}

export interface AccountAuditResult {
  total: number;
  active: number;
  inactive: number;
  admins: number;
  dormant: number;
  withoutMfa: number | null;
  duplicates: number;
  issues: AccountIssue[];
  policy: Record<string, unknown> | null;
  fieldsDetected: string[];
  notes: string[];
}

const DEFAULT_NAMES = /^(admin|administrator|root|test|guest|demo|temp|user|default)$/i;
const DORMANT_DAYS = 90;

export async function runAccountAudit(opts: {
  db: Database;
  assessments: Assessments;
  connections: Connections;
  connectionId: string;
  assessmentId: string;
  signal: AbortSignal;
  onProgress: (p: number, msg: string) => void;
}): Promise<AccountAuditResult> {
  const { db, assessments, connections, connectionId, assessmentId, signal, onProgress } = opts;
  const profile = connections.get(connectionId);
  const ctx = restContext(profile);
  const notes: string[] = [];

  onProgress(10, 'Fetching account directory');
  // usersEndpoint is the URL path; usersPath is the dotted JSON path inside the response.
  const usersRes = await restRequest(ctx, profile.config.usersEndpoint || '/api/admin/users', { signal });
  if (usersRes.status === 401 || usersRes.status === 403) {
    throw new Error(`The account directory rejected the request (HTTP ${usersRes.status}). Check the admin credential and that it has user-management permission.`);
  }
  const usersJson = parseJson(usersRes.body);
  const records = findRecordArray(getPath(usersJson, profile.config.usersPath) ?? usersJson);
  if (!usersJson) {
    throw new Error(`The account endpoint did not return JSON (HTTP ${usersRes.status}, content-type ${usersRes.headers['content-type'] ?? 'unknown'}). Set the correct users path in the connection profile.`);
  }
  if (records.length === 0) {
    throw new Error('No account records found in the response. Set the JSON path to the users array (for example "data.users").');
  }

  const sample = records[0];
  const fields = {
    id: pickField(sample, ['id', 'user_id', 'uuid', 'account_id']),
    username: pickField(sample, ['username', 'user_name', 'email', 'login', 'name']),
    role: pickField(sample, ['role', 'roles', 'user_role', 'role_name', 'roleName', 'group', 'type']),
    active: pickField(sample, ['active', 'is_active', 'enabled', 'is_enabled', 'status', 'locked']),
    lastLogin: pickField(sample, ['last_login', 'lastLogin', 'last_login_at', 'last_seen', 'lastSeen', 'last_activity']),
    mfa: pickField(sample, ['mfa', 'mfa_enabled', 'two_factor_enabled', 'twoFactorEnabled', 'otp_enabled', 'totp_enabled']),
    created: pickField(sample, ['created_at', 'createdAt', 'created', 'registered_at', 'date_joined']),
    password: pickField(sample, ['password', 'password_hash', 'passwordHash', 'hash', 'passwd']),
  };
  const fieldsDetected = Object.entries(fields)
    .filter(([, v]) => v)
    .map(([k, v]) => `${k} → ${(v as { key: string }).key}`);

  const accounts = records.map((r) => {
    const rawName = fields.username ? String(r[fields.username.key] ?? '') : '';
    const rawRole = fields.role ? r[fields.role.key] : '';
    const role = Array.isArray(rawRole) ? rawRole.join(', ') : String(rawRole ?? 'unknown');
    const lastRaw = fields.lastLogin ? r[fields.lastLogin.key] : null;
    const lastDate = lastRaw ? new Date(String(lastRaw)) : null;
    return {
      id: fields.id ? String(r[fields.id.key] ?? '') : '',
      username: rawName,
      masked: maskEmail(rawName),
      role,
      active: fields.active ? toBool(r[fields.active.key]) : null,
      lastLogin: lastDate && !Number.isNaN(lastDate.getTime()) ? lastDate.toISOString() : null,
      mfa: fields.mfa ? toBool(r[fields.mfa.key]) : null,
      created: fields.created ? String(r[fields.created.key] ?? '') : null,
      hasPasswordMaterial: Boolean(fields.password && r[fields.password.key]),
    };
  });

  onProgress(45, 'Reviewing password policy and lockout configuration');
  let policy: Record<string, unknown> | null = null;
  if (profile.config.policyPath) {
    try {
      const res = await restRequest(ctx, profile.config.policyPath, { signal });
      const json = parseJson(res.body);
      if (res.status === 200 && json && typeof json === 'object') {
        const obj = (getPath(json, profile.config.policyPath) ?? json) as Record<string, unknown>;
        policy = {
          minLength: readNum(obj, ['min_length', 'minLength', 'passwordMinLength', 'minimum_length']),
          requireComplexity: readBool(obj, ['require_complexity', 'complexity', 'requireComplexity']),
          lockoutThreshold: readNum(obj, ['lockout_threshold', 'lockoutThreshold', 'max_attempts', 'maxLoginAttempts']),
          lockoutDurationMinutes: readNum(obj, ['lockout_duration', 'lockoutDurationMinutes', 'lockout_minutes']),
          sessionTimeoutMinutes: readNum(obj, ['session_timeout', 'sessionTimeoutMinutes', 'session_lifetime']),
          mfaRequired: readBool(obj, ['mfa_required', 'require_mfa', 'mfaRequired']),
          passwordExpiryDays: readNum(obj, ['password_expiry_days', 'passwordExpiryDays', 'max_password_age']),
        };
      } else {
        notes.push(`Policy endpoint returned HTTP ${res.status} or non-JSON.`);
      }
    } catch (err) {
      notes.push(`Policy endpoint failed: ${(err as Error).message}`);
    }
  } else {
    notes.push('No policy endpoint configured; password policy could not be verified.');
  }

  let failedLogins: Record<string, number> = {};
  if (profile.config.loginEventsPath) {
    try {
      const res = await restRequest(ctx, profile.config.loginEventsPath, { signal });
      const json = parseJson(res.body);
      const events = findRecordArray(json);
      const cutoff = Date.now() - 7 * 86_400_000;
      for (const e of events) {
        const ok = pickField(e, ['success', 'successful', 'ok', 'result', 'status']);
        const who = pickField(e, ['username', 'email', 'user', 'login']);
        const when = pickField(e, ['created_at', 'timestamp', 'time', 'date']);
        const success = ok ? toBool(ok.value) : null;
        const ts = when ? new Date(String(when.value)).getTime() : Date.now();
        if (success === false && ts >= cutoff && who) {
          const name = String(who.value);
          failedLogins[name] = (failedLogins[name] ?? 0) + 1;
        }
      }
    } catch (err) {
      notes.push(`Login events could not be read: ${(err as Error).message}`);
    }
  }

  onProgress(70, 'Analyzing account security');
  const issues: AccountIssue[] = [];
  const byName = new Map<string, number>();
  for (const a of accounts) byName.set(a.username.toLowerCase(), (byName.get(a.username.toLowerCase()) ?? 0) + 1);
  const dupNames = [...byName.entries()].filter(([, n]) => n > 1).map(([n]) => n);

  const dormant = accounts.filter((a) => a.active !== false && (!a.lastLogin || Date.now() - new Date(a.lastLogin).getTime() > DORMANT_DAYS * 86_400_000));
  const inactive = accounts.filter((a) => a.active === false);
  const admins = accounts.filter((a) => /admin|super|root|owner/i.test(a.role));
  const noMfa = accounts.filter((a) => a.mfa === false);
  const defaultish = accounts.filter((a) => DEFAULT_NAMES.test(a.username.split('@')[0]));
  const contradictions = accounts.filter((a) => a.active === false && a.lastLogin && Date.now() - new Date(a.lastLogin).getTime() < 30 * 86_400_000);
  const exposedHashes = accounts.filter((a) => a.hasPasswordMaterial);

  if (dupNames.length) {
    issues.push({
      severity: 'medium',
      name: 'Duplicate account identifiers',
      detail: `${dupNames.length} username(s) appear more than once. Duplicate identities undermine accountability and can bypass lockout logic.`,
      accounts: dupNames.map(maskEmail),
      remediation: 'Enforce a unique constraint on the username/email column and merge or remove duplicates.',
    });
  }
  if (dormant.length) {
    issues.push({
      severity: 'medium',
      name: 'Dormant active accounts',
      detail: `${dormant.length} active account(s) have no login in the last ${DORMANT_DAYS} days.`,
      accounts: dormant.slice(0, 25).map((a) => a.masked),
      remediation: 'Disable or delete dormant accounts and require manager approval to reactivate.',
    });
  }
  if (defaultish.length) {
    issues.push({
      severity: 'medium',
      name: 'Default or shared-looking account names',
      detail: `${defaultish.length} account(s) use generic names commonly targeted by credential attacks.`,
      accounts: defaultish.map((a) => a.masked),
      remediation: 'Use named accounts, remove default logins, and require unique credentials with MFA.',
    });
  }
  if (contradictions.length) {
    issues.push({
      severity: 'high',
      name: 'Disabled accounts with recent activity',
      detail: `${contradictions.length} account(s) are marked inactive but logged in within 30 days, which suggests the status flag is not enforced.`,
      accounts: contradictions.map((a) => a.masked),
      remediation: 'Enforce the active flag in the authentication path and revoke sessions on disable.',
    });
  }
  if (noMfa.length) {
    issues.push({
      severity: noMfa.some((a) => /admin|super|root/i.test(a.role)) ? 'high' : 'medium',
      name: 'Accounts without MFA',
      detail: `${noMfa.length} account(s) do not have MFA enabled.`,
      accounts: noMfa.slice(0, 25).map((a) => a.masked),
      remediation: 'Require MFA for all users, starting with administrators.',
    });
  }
  if (exposedHashes.length) {
    issues.push({
      severity: 'critical',
      name: 'Password material returned by the API',
      detail: `${exposedHashes.length} account record(s) include a password or hash field. Values were not stored, printed or sent to the AI.`,
      accounts: exposedHashes.slice(0, 10).map((a) => a.masked),
      remediation: 'Remove password/hash fields from API serialization and rotate any exposed credentials.',
    });
  }
  const noisy = Object.entries(failedLogins).filter(([, n]) => n >= 5);
  if (noisy.length) {
    issues.push({
      severity: 'medium',
      name: 'Repeated failed logins in the last 7 days',
      detail: `${noisy.length} account(s) had 5 or more failed logins.`,
      accounts: noisy.slice(0, 25).map(([n, c]) => `${maskEmail(n)} (${c})`),
      remediation: 'Verify lockout/throttling, alert on credential stuffing, and require password resets where compromise is suspected.',
    });
  }
  if (admins.length > 0 && admins.some((a) => a.mfa === false)) {
    issues.push({
      severity: 'high',
      name: 'Administrators without MFA',
      detail: 'One or more privileged accounts do not have MFA.',
      accounts: admins.filter((a) => a.mfa === false).map((a) => a.masked),
      remediation: 'Enforce MFA and just-in-time elevation for administrator accounts.',
    });
  }
  if (policy) {
    const minLen = policy.minLength as number | null;
    if (minLen !== null && minLen < 12) {
      issues.push({ severity: minLen < 8 ? 'high' : 'medium', name: 'Weak password length policy', detail: `Minimum password length is ${minLen}; 12+ is recommended.`, accounts: [], remediation: 'Require at least 12 characters and block breached passwords.' });
    }
    const threshold = policy.lockoutThreshold as number | null;
    if (threshold === null || threshold > 10 || threshold === 0) {
      issues.push({ severity: 'medium', name: 'Account lockout not effectively configured', detail: `Lockout threshold: ${threshold ?? 'not set'}.`, accounts: [], remediation: 'Lock accounts for 15 minutes after 5–10 failed attempts and add progressive delays.' });
    }
    const session = policy.sessionTimeoutMinutes as number | null;
    if (session === null || session > 60) {
      issues.push({ severity: 'low', name: 'Long or unset session timeout', detail: `Session timeout: ${session ?? 'not set'} minutes.`, accounts: [], remediation: 'Expire idle sessions after 15–30 minutes and rotate session IDs on login.' });
    }
    if (policy.mfaRequired === false) {
      issues.push({ severity: 'medium', name: 'MFA not required by policy', detail: 'The application policy does not require multi-factor authentication.', accounts: [], remediation: 'Require MFA for administrators and offer it to all users.' });
    }
    if (policy.requireComplexity === false) {
      issues.push({ severity: 'low', name: 'Password complexity not enforced', detail: 'No complexity requirement is enabled.', accounts: [], remediation: 'Prefer length and breached-password checks over composition rules.' });
    }
  }

  for (const issue of issues) {
    assessments.addFinding(assessmentId, ctx.target.url, {
      target: ctx.target.url,
      name: issue.name,
      category: 'accounts',
      severity: issue.severity,
      confidence: 'high',
      endpoint: profile.base_url ?? ctx.target.url,
      evidence: issue.accounts.length ? `Affected accounts: ${issue.accounts.join(', ')}` : issue.detail,
      description: issue.detail,
      impact: 'Weak account hygiene increases the chance of unauthorized access and hides who did what.',
      reproduction: 'Read the authorized account administration endpoint and compare the reported fields with your policy.',
      remediation: issue.remediation,
      references: ['https://cheatsheetseries.owasp.org/cheatsheets/Authentication_Cheat_Sheet.html', 'https://owasp.org/www-project-top-ten/2017/A2_2017-Broken_Authentication'],
    });
  }

  const result: AccountAuditResult = {
    total: accounts.length,
    active: accounts.filter((a) => a.active !== false).length,
    inactive: inactive.length,
    admins: admins.length,
    dormant: dormant.length,
    withoutMfa: fields.mfa ? noMfa.length : null,
    duplicates: dupNames.length,
    issues,
    policy,
    fieldsDetected,
    notes,
  };

  db.run('INSERT INTO account_audits (id, assessment_id, source, summary, data, created_at) VALUES (?,?,?,?,?,?)', [
    randomUUID(),
    assessmentId,
    profile.name,
    `${accounts.length} accounts reviewed, ${issues.length} issue groups`,
    JSON.stringify({
      counts: { total: result.total, active: result.active, inactive: result.inactive, admins: result.admins, dormant: result.dormant, withoutMfa: result.withoutMfa },
      issues,
      policy,
      fieldsDetected,
      notes,
      accounts: accounts.map((a) => ({ username: a.masked, role: a.role, active: a.active, lastLogin: a.lastLogin, mfa: a.mfa })),
    }),
    isoNow(),
  ]);
  onProgress(95, 'Account audit complete');
  return result;
}

function readNum(obj: Record<string, unknown>, keys: string[]): number | null {
  const f = pickField(obj, keys);
  if (!f) return null;
  const n = Number(f.value);
  return Number.isFinite(n) ? n : null;
}

function readBool(obj: Record<string, unknown>, keys: string[]): boolean | null {
  const f = pickField(obj, keys);
  return f ? toBool(f.value) : null;
}

export function summarizeAccounts(r: AccountAuditResult): string {
  return redact(
    `Accounts: ${r.total} total, ${r.active} active, ${r.inactive} inactive, ${r.admins} admin, ${r.dormant} dormant. Issue groups: ${r.issues.length}.`,
  );
}
