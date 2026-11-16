import initSqlJs from 'sql.js';
import type { Database as SqlJsDatabase } from 'sql.js';
import fs from 'node:fs';
import path from 'node:path';

/**
 * SQLite persistence using SQLite compiled to WebAssembly (sql.js).
 *
 * Real SQLite semantics (foreign keys, transactions, constraints) with no native
 * module build step, so the packaged Electron app runs on any Windows machine.
 * Writes are debounced and persisted atomically (temp file + rename).
 */
const SCHEMA_VERSION = 1;

const SCHEMA = `
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS conversations (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK (role IN ('user','assistant','tool','system')),
  content TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_conversation ON messages(conversation_id);

CREATE TABLE IF NOT EXISTS targets (
  id TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  url TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('web','api','php','mysql','localhost')),
  authorization_note TEXT NOT NULL,
  scope_patterns TEXT NOT NULL DEFAULT '[]',
  rate_limit REAL NOT NULL DEFAULT 2,
  allow_private INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS assessments (
  id TEXT PRIMARY KEY,
  target_id TEXT REFERENCES targets(id) ON DELETE SET NULL,
  target_label TEXT NOT NULL,
  type TEXT NOT NULL,
  status TEXT NOT NULL,
  progress INTEGER NOT NULL DEFAULT 0,
  summary TEXT,
  error TEXT,
  started_at TEXT NOT NULL,
  finished_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_assessments_started ON assessments(started_at);

CREATE TABLE IF NOT EXISTS findings (
  id TEXT PRIMARY KEY,
  assessment_id TEXT NOT NULL REFERENCES assessments(id) ON DELETE CASCADE,
  target TEXT NOT NULL,
  name TEXT NOT NULL,
  category TEXT NOT NULL,
  severity TEXT NOT NULL,
  confidence TEXT NOT NULL,
  endpoint TEXT NOT NULL,
  evidence TEXT NOT NULL,
  description TEXT NOT NULL,
  impact TEXT NOT NULL,
  reproduction TEXT NOT NULL,
  remediation TEXT NOT NULL,
  "references" TEXT NOT NULL DEFAULT '[]',
  status TEXT NOT NULL DEFAULT 'open',
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_findings_assessment ON findings(assessment_id);
CREATE INDEX IF NOT EXISTS idx_findings_severity ON findings(severity);

CREATE TABLE IF NOT EXISTS recon_results (
  id TEXT PRIMARY KEY,
  assessment_id TEXT NOT NULL REFERENCES assessments(id) ON DELETE CASCADE,
  target TEXT NOT NULL,
  data TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS account_audits (
  id TEXT PRIMARY KEY,
  assessment_id TEXT NOT NULL REFERENCES assessments(id) ON DELETE CASCADE,
  source TEXT NOT NULL,
  summary TEXT NOT NULL,
  data TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS rbac_results (
  id TEXT PRIMARY KEY,
  assessment_id TEXT NOT NULL REFERENCES assessments(id) ON DELETE CASCADE,
  profile TEXT NOT NULL,
  summary TEXT NOT NULL,
  matrix TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS api_results (
  id TEXT PRIMARY KEY,
  assessment_id TEXT NOT NULL REFERENCES assessments(id) ON DELETE CASCADE,
  profile TEXT NOT NULL,
  summary TEXT NOT NULL,
  data TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tool_runs (
  id TEXT PRIMARY KEY,
  tool TEXT NOT NULL,
  target TEXT,
  args TEXT NOT NULL,
  status TEXT NOT NULL,
  exit_code INTEGER,
  output TEXT NOT NULL DEFAULT '',
  started_at TEXT NOT NULL,
  finished_at TEXT
);

CREATE TABLE IF NOT EXISTS approvals (
  id TEXT PRIMARY KEY,
  assessment_id TEXT,
  level INTEGER NOT NULL,
  title TEXT NOT NULL,
  detail TEXT NOT NULL,
  decision TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS audit_logs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  actor TEXT NOT NULL,
  action TEXT NOT NULL,
  target TEXT,
  level INTEGER NOT NULL DEFAULT 1,
  detail TEXT,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_created ON audit_logs(created_at);

CREATE TABLE IF NOT EXISTS reports (
  id TEXT PRIMARY KEY,
  assessment_id TEXT NOT NULL REFERENCES assessments(id) ON DELETE CASCADE,
  format TEXT NOT NULL,
  path TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS connections (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  target_id TEXT REFERENCES targets(id) ON DELETE SET NULL,
  base_url TEXT,
  username TEXT,
  config_enc TEXT NOT NULL,
  config_public TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS access_captures (
  id TEXT PRIMARY KEY,
  url TEXT NOT NULL,
  title TEXT NOT NULL,
  summary TEXT NOT NULL,
  html_path TEXT NOT NULL,
  captured_at TEXT NOT NULL
);
`;

export function isoNow(): string {
  return new Date().toISOString();
}

export class Database {
  private constructor(private db: SqlJsDatabase, private file: string) {}

  static async open(file: string): Promise<Database> {
    const SQL = await initSqlJs({
      locateFile: (f: string) => {
        // sql.js has an exports map that blocks ./package.json, so resolve the
        // entry point and look for the wasm next to it.
        const candidates = [
          path.join(path.dirname(require.resolve('sql.js')), f),
          path.join(process.cwd(), 'node_modules/sql.js/dist', f),
        ];
        return candidates.find((p) => fs.existsSync(p)) ?? candidates[0];
      },
    });
    fs.mkdirSync(path.dirname(file), { recursive: true });
    const db = fs.existsSync(file) ? new SQL.Database(fs.readFileSync(file)) : new SQL.Database();
    const inst = new Database(db, file);
    db.run('PRAGMA foreign_keys = ON;');
    inst.migrate();
    return inst;
  }

  private migrate(): void {
    // A brand-new database has no meta table yet; treat that as version 0.
    let version = 0;
    try {
      const current = this.get<{ value: string }>('SELECT value FROM meta WHERE key = ?', ['schema_version']);
      version = current ? Number(current.value) : 0;
    } catch {
      version = 0;
    }
    if (version < SCHEMA_VERSION) {
      this.db.exec(SCHEMA);
      this.db.run('INSERT OR REPLACE INTO meta (key, value) VALUES (?, ?)', ['schema_version', String(SCHEMA_VERSION)]);
      this.persist();
    }
  }

  run(sql: string, params: unknown[] = []): void {
    this.db.run(sql, params.map(normalizeParam) as never[]);
    this.scheduleSave();
  }

  all<T = Record<string, unknown>>(sql: string, params: unknown[] = []): T[] {
    const stmt = this.db.prepare(sql);
    try {
      stmt.bind(params.map(normalizeParam) as never[]);
      const rows: T[] = [];
      while (stmt.step()) rows.push(stmt.getAsObject() as T);
      return rows;
    } finally {
      stmt.free();
    }
  }

  get<T = Record<string, unknown>>(sql: string, params: unknown[] = []): T | undefined {
    return this.all<T>(sql, params)[0];
  }

  /** Serialize a group of writes; rolls back on error. */
  tx<T>(fn: () => T): T {
    this.db.run('BEGIN');
    try {
      const out = fn();
      this.db.run('COMMIT');
      this.persist();
      return out;
    } catch (err) {
      this.db.run('ROLLBACK');
      throw err;
    }
  }

  private saveTimer: NodeJS.Timeout | null = null;

  private scheduleSave(): void {
    if (this.saveTimer) clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => {
      this.saveTimer = null;
      this.persist();
    }, 250);
  }

  persist(): void {
    if (this.saveTimer) {
      clearTimeout(this.saveTimer);
      this.saveTimer = null;
    }
    const data = Buffer.from(this.db.export());
    const tmp = `${this.file}.tmp`;
    fs.writeFileSync(tmp, data);
    fs.renameSync(tmp, this.file);
  }

  close(): void {
    this.persist();
    this.db.close();
  }
}

function normalizeParam(v: unknown): string | number | null | Uint8Array {
  if (v === undefined || v === null) return null;
  if (typeof v === 'boolean') return v ? 1 : 0;
  if (typeof v === 'number' || typeof v === 'string') return v;
  if (v instanceof Uint8Array) return v;
  return JSON.stringify(v);
}
