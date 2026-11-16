import { randomUUID } from 'node:crypto';
import type { Database } from './db';
import { isoNow } from './db';
import type { Store } from './store';
import type { ConnectionProfile, Target } from '../../shared/types';
import { parseTarget } from './scope';
import { redact } from './redact';

export interface ApiEndpoint {
  name: string;
  method: string;
  path: string;
  /** RBAC expectation: whether the low-privilege test account should be allowed. */
  viewerAllowed?: boolean;
  requiresAuth?: boolean;
}

export interface ConnectionConfig {
  auth?: { type: 'bearer' | 'basic' | 'cookie' | 'none'; value?: string; username?: string };
  viewerAuth?: { type: 'bearer' | 'basic' | 'cookie' | 'none'; value?: string; username?: string };
  usersEndpoint?: string;
  usersPath?: string;
  rolesPath?: string;
  loginEventsPath?: string;
  policyPath?: string;
  endpoints?: ApiEndpoint[];
  mysql?: { host: string; port: number; user: string; password?: string; database: string; readOnly: boolean };
}

const SECRET_KEY_PREFIX = 'conn.';

export type ResolvedConnection = Omit<ConnectionProfile, 'config'> & { config: ConnectionConfig; target: Target | null };

export class Connections {
  constructor(private db: Database, private store: Store) {}

  list(): ConnectionProfile[] {
    return this.db.all<Record<string, unknown>>('SELECT id, name, kind, target_id, base_url, username, config_public, created_at FROM connections ORDER BY created_at DESC').map((r) => this.toProfile(r));
  }

  get(id: string): ResolvedConnection {
    const row = this.db.get<Record<string, unknown>>('SELECT * FROM connections WHERE id = ?', [id]);
    if (!row) throw new Error('Connection profile not found');
    const config = this.decrypt(String(row.id), String(row.config_enc));
    const target = row.target_id ? this.target(String(row.target_id)) : null;
    return { ...this.toProfile(row), config, target };
  }

  target(id: string): Target | null {
    const row = this.db.get<Record<string, unknown>>('SELECT * FROM targets WHERE id = ?', [id]);
    return row ? parseTarget(row) : null;
  }

  save(input: {
    id?: string;
    name: string;
    kind: 'rest' | 'mysql' | 'database';
    targetId: string;
    baseUrl?: string;
    username?: string;
    config: ConnectionConfig;
  }): ResolvedConnection {
    const id = input.id ?? randomUUID();
    const existing = this.db.get<Record<string, unknown>>('SELECT id FROM connections WHERE id = ?', [id]);
    const enc = this.encrypt(id, input.config);
    const pub = JSON.stringify(this.publicConfig(input.config));
    if (existing) {
      this.db.run('UPDATE connections SET name = ?, kind = ?, target_id = ?, base_url = ?, username = ?, config_enc = ?, config_public = ? WHERE id = ?', [
        input.name.slice(0, 120),
        input.kind,
        input.targetId,
        input.baseUrl ?? null,
        input.username ?? null,
        enc,
        pub,
        id,
      ]);
    } else {
      this.db.run('INSERT INTO connections (id, name, kind, target_id, base_url, username, config_enc, config_public, created_at) VALUES (?,?,?,?,?,?,?,?,?)', [
        id,
        input.name.slice(0, 120),
        input.kind,
        input.targetId,
        input.baseUrl ?? null,
        input.username ?? null,
        enc,
        pub,
        isoNow(),
      ]);
    }
    this.store.setSetting('last.connection', id);
    return this.get(id);
  }

  remove(id: string): void {
    this.db.run('DELETE FROM connections WHERE id = ?', [id]);
    this.store.clearSecret(`${SECRET_KEY_PREFIX}${id}`);
  }

  private toProfile(row: Record<string, unknown>): ConnectionProfile {
    return {
      id: String(row.id),
      name: String(row.name),
      kind: row.kind as ConnectionProfile['kind'],
      base_url: row.base_url ? String(row.base_url) : null,
      username: row.username ? String(row.username) : null,
      has_secret: this.store.hasSecret(`${SECRET_KEY_PREFIX}${row.id}`) ? 1 : 0,
      created_at: String(row.created_at),
      config: parseJsonObject(row.config_public) ?? {},
    };
  }

  /** Non-secret configuration for editing in the UI (endpoints, paths, host). */
  publicConfig(config: ConnectionConfig): ConnectionConfig {
    return {
      usersEndpoint: config.usersEndpoint,
      usersPath: config.usersPath,
      rolesPath: config.rolesPath,
      loginEventsPath: config.loginEventsPath,
      policyPath: config.policyPath,
      endpoints: config.endpoints,
      mysql: config.mysql ? { host: config.mysql.host, port: config.mysql.port, user: config.mysql.user, database: config.mysql.database, readOnly: true } : undefined,
      auth: config.auth ? { type: config.auth.type, username: config.auth.username } : undefined,
      viewerAuth: config.viewerAuth ? { type: config.viewerAuth.type, username: config.viewerAuth.username } : undefined,
    };
  }

  hasCredentials(id: string): boolean {
    return this.store.hasSecret(`${SECRET_KEY_PREFIX}${id}`);
  }

  /** Config contains credentials, so it is only ever held encrypted with OS secure storage. */
  private encrypt(id: string, config: ConnectionConfig): string {
    const hasSecrets = Boolean(
      config.auth?.value || config.auth?.username || config.viewerAuth?.value || config.viewerAuth?.username || config.mysql?.password,
    );
    if (!hasSecrets) return JSON.stringify({ ...config, _plain: true });
    if (!this.store.secureAvailable()) {
      throw new Error('Secure OS storage is unavailable, so credentials cannot be stored. Connection profiles require secure storage.');
    }
    this.store.putSecret(`${SECRET_KEY_PREFIX}${id}`, JSON.stringify(config));
    return JSON.stringify({ _encrypted: true });
  }

  private decrypt(id: string, enc: string): ConnectionConfig {
    const meta = parseJsonObject(enc) as { _encrypted?: boolean } | null;
    if (meta && meta._encrypted) {
      const raw = this.store.getSecret(`${SECRET_KEY_PREFIX}${id}`);
      if (!raw) throw new Error('Stored credentials could not be decrypted. Re-enter them in the connection profile.');
      return JSON.parse(raw) as ConnectionConfig;
    }
    return (meta ?? {}) as ConnectionConfig;
  }
}

function parseJsonObject(raw: unknown): Record<string, unknown> | null {
  if (typeof raw !== 'string') return null;
  try {
    const v = JSON.parse(raw);
    return v && typeof v === 'object' ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

export function authHeaders(auth: ConnectionConfig['auth']): Record<string, string> {
  if (!auth || auth.type === 'none') return {};
  if (auth.type === 'bearer' && auth.value) return { Authorization: `Bearer ${auth.value}` };
  if (auth.type === 'basic') return { Authorization: `Basic ${Buffer.from(`${auth.username ?? ''}:${auth.value ?? ''}`).toString('base64')}` };
  if (auth.type === 'cookie' && auth.value) return { Cookie: auth.value };
  return {};
}

export function safeProfileForAudit(c: ConnectionProfile): Record<string, unknown> {
  return { id: c.id, name: c.name, base_url: c.base_url, username: c.username, created_at: c.created_at };
}

export { redact };
