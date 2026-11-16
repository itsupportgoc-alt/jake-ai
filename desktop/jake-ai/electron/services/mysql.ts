import mysql from 'mysql2/promise';
import { randomUUID } from 'node:crypto';
import type { Severity } from '../../shared/types';
import type { Database } from './db';
import { isoNow } from './db';
import type { Assessments } from './assessments';
import type { Connections } from './connections';
import { maskEmail, redact } from './redact';

/**
 * Read-only MySQL / MariaDB security review.
 * The session is set READ ONLY and only metadata/aggregates plus minimal
 * account columns are read. Hashes and secrets are never returned or logged.
 */
const IDENT = /^[A-Za-z0-9_$]+$/;

export interface MysqlAuditResult {
  server: string;
  database: string;
  tables: number;
  userTables: string[];
  issues: Array<{ severity: Severity; name: string; detail: string; remediation: string }>;
  notes: string[];
}

export async function runMysqlAudit(opts: {
  db: Database;
  assessments: Assessments;
  connections: Connections;
  connectionId: string;
  assessmentId: string;
  signal: AbortSignal;
  onProgress: (p: number, msg: string) => void;
}): Promise<MysqlAuditResult> {
  const { db, assessments, connections, connectionId, assessmentId, signal, onProgress } = opts;
  const profile = connections.get(connectionId);
  const cfg = profile.config.mysql;
  if (!cfg) throw new Error('This connection profile has no MySQL settings. Edit it and fill in host, port, user, database.');
  if (!IDENT.test(cfg.database)) throw new Error('Invalid database name.');
  onProgress(5, `Connecting to MySQL at ${cfg.host}:${cfg.port}`);
  const conn = await mysql.createConnection({
    host: cfg.host,
    port: cfg.port,
    user: cfg.user,
    password: cfg.password ?? '',
    database: cfg.database,
    connectTimeout: 10000,
    multipleStatements: false,
    dateStrings: true,
    // Read-only session; no writes are ever issued.
  });
  const notes: string[] = [];
  const issues: MysqlAuditResult['issues'] = [];
  const addIssue = (severity: Severity, name: string, detail: string, remediation: string) =>
    issues.push({ severity, name, detail, remediation });

  try {
    await conn.query('SET SESSION TRANSACTION READ ONLY');
    await conn.query('SET SESSION max_execution_time = 15000').catch(() => undefined);
    const [verRows] = await conn.query<mysql.RowDataPacket[]>('SELECT VERSION() AS version, CURRENT_USER() AS currentUser, @@hostname AS hostname');
    const server = `${verRows[0].version} @ ${verRows[0].hostname}`;
    onProgress(20, 'Reviewing database permissions');

    const [grants] = await conn.query<mysql.RowDataPacket[]>('SHOW GRANTS FOR CURRENT_USER()');
    const grantText = grants.map((g) => Object.values(g).map(String).join(' ')).join('\n');
    if (/GRANT ALL PRIVILEGES ON \*\.\*/i.test(grantText)) {
      addIssue('high', 'Application database user has global privileges', 'The user used by the application can administer the whole server. A SQL injection would become server compromise.', 'Create a least-privilege user limited to this database with SELECT/INSERT/UPDATE/DELETE only.');
    } else if (/GRANT OPTION/i.test(grantText)) {
      addIssue('medium', 'Application database user can grant privileges', 'GRANT OPTION lets a compromised application create new database users.', 'Remove GRANT OPTION from the application user.');
    }
    if (/TO '[^']+'@'%'/i.test(grantText)) {
      addIssue('medium', 'Database account allows connections from any host', "The account is defined for host '%', so it can be used from any network address.", 'Restrict the account host to localhost or the application subnet.');
    }
    if (/WITH GRANT OPTION|ALL PRIVILEGES/i.test(grantText)) notes.push('Privilege review flagged broad grants.');

    onProgress(35, 'Inspecting schema and foreign keys');
    const [tables] = await conn.query<mysql.RowDataPacket[]>(
      `SELECT TABLE_NAME AS name, ENGINE AS engine FROM information_schema.TABLES WHERE TABLE_SCHEMA = ?`,
      [cfg.database],
    );
    const tableNames = tables.map((t) => String(t.name)).filter((n) => IDENT.test(n));
    const [pk] = await conn.query<mysql.RowDataPacket[]>(
      `SELECT TABLE_NAME AS t FROM information_schema.TABLE_CONSTRAINTS WHERE TABLE_SCHEMA = ? AND CONSTRAINT_TYPE = 'PRIMARY KEY'`,
      [cfg.database],
    );
    const withPk = new Set(pk.map((r) => String(r.t)));
    const withoutPk = tableNames.filter((t) => !withPk.has(t));
    if (withoutPk.length) {
      addIssue('low', 'Tables without a primary key', `${withoutPk.length} table(s) have no primary key: ${withoutPk.slice(0, 10).join(', ')}.`, 'Add a primary key to every table for integrity and efficient scoped updates.');
    }
    const [fks] = await conn.query<mysql.RowDataPacket[]>(
      `SELECT COUNT(*) AS n FROM information_schema.REFERENTIAL_CONSTRAINTS WHERE CONSTRAINT_SCHEMA = ?`,
      [cfg.database],
    );
    const fkCount = Number(fks[0].n);
    notes.push(`${tableNames.length} tables, ${fkCount} foreign keys.`);
    if (tableNames.length > 3 && fkCount === 0) {
      addIssue('info', 'No foreign keys defined', 'The schema has multiple tables but no referential constraints, so orphaned or inconsistent records cannot be prevented by the database.', 'Add foreign keys with appropriate ON DELETE behaviour where relationships exist.');
    }

    onProgress(50, 'Identifying account tables');
    const [pwCols] = await conn.query<mysql.RowDataPacket[]>(
      `SELECT TABLE_NAME AS t, COLUMN_NAME AS c FROM information_schema.COLUMNS
       WHERE TABLE_SCHEMA = ? AND LOWER(COLUMN_NAME) IN ('password','password_hash','passwd','user_password','pwd_hash','hash')`,
      [cfg.database],
    );
    const userTables = [...new Set(pwCols.map((r) => String(r.t)))].filter((t) => IDENT.test(t));
    if (userTables.length === 0) notes.push('No obvious account tables (password column) found in this database.');

    for (const table of userTables.slice(0, 5)) {
      if (signal.aborted) throw new Error('Operation cancelled');
      const t = `\`${table}\``;
      const [cols] = await conn.query<mysql.RowDataPacket[]>(
        `SELECT COLUMN_NAME AS c FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?`,
        [cfg.database, table],
      );
      const colNames = cols.map((c) => String(c.c)).filter((c) => IDENT.test(c));
      const lower = new Map(colNames.map((c) => [c.toLowerCase(), c]));
      const pick = (cands: string[]) => cands.map((c) => lower.get(c)).find(Boolean);
      const pwCol = pick(['password', 'password_hash', 'passwd', 'user_password', 'pwd_hash', 'hash'])!;
      const nameCol = pick(['username', 'user_name', 'email', 'login', 'name']);
      const roleCol = pick(['role', 'role_name', 'user_role', 'type', 'group']);
      const activeCol = pick(['active', 'is_active', 'enabled', 'status']);

      const [[counts]] = await conn.query<mysql.RowDataPacket[]>(`SELECT COUNT(*) AS total FROM ${t}`);
      const total = Number(counts.total);
      notes.push(`Account table ${table}: ${total} rows.`);

      // Hash algorithm detection uses only a short prefix, never the hash itself.
      const [prefixes] = await conn.query<mysql.RowDataPacket[]>(
        `SELECT SUBSTRING(${`\`${pwCol}\``}, 1, 4) AS prefix, COUNT(*) AS n FROM ${t} GROUP BY prefix LIMIT 20`,
      );
      const md5 = prefixes.filter((p) => /^[0-9a-f]{4}$/i.test(String(p.prefix))).reduce((n, p) => n + Number(p.n), 0);
      const modern = prefixes.filter((p) => /^\$2[aby]|^\$argon2/i.test(String(p.prefix))).reduce((n, p) => n + Number(p.n), 0);
      if (md5 > 0 && modern === 0) {
        addIssue('critical', 'Passwords stored with a legacy fast hash', `Table ${table}: ${md5} hashes appear to be unsalted MD5/SHA-family. Hashes were not read.`, 'Migrate to password_hash()/bcrypt or Argon2id, and force a reset at next login.');
      }
      if (total > 0 && modern === 0 && md5 === 0) {
        addIssue('medium', 'Password hash format not recognised', `Table ${table}: hash prefixes are neither bcrypt nor Argon2. Verify the application hashing algorithm.`, 'Use PASSWORD_DEFAULT (bcrypt/Argon2id) and document the algorithm.');
      }
      if (nameCol) {
        const [dupes] = await conn.query<mysql.RowDataPacket[]>(
          `SELECT ${`\`${nameCol}\``} AS name, COUNT(*) AS n FROM ${t} GROUP BY ${`\`${nameCol}\``} HAVING COUNT(*) > 1 LIMIT 20`,
        );
        if (dupes.length) {
          addIssue('medium', 'Duplicate usernames in database', `${dupes.length} duplicate identifier group(s) found in ${table}.`, 'Add a UNIQUE constraint on the username/email column and resolve existing duplicates.');
        }
        if (activeCol) {
          const [dormant] = await conn.query<mysql.RowDataPacket[]>(
            `SELECT COUNT(*) AS n FROM ${t} WHERE ${`\`${activeCol}\``} IN (1,'1','active','enabled')`,
          );
          notes.push(`${Number(dormant[0].n)} active rows in ${table}.`);
        }
        if (roleCol) {
          const [admins] = await conn.query<mysql.RowDataPacket[]>(
            `SELECT COUNT(*) AS n FROM ${t} WHERE ${`\`${roleCol}\``} LIKE '%admin%'`,
          );
          const adminCount = Number(admins[0].n);
          if (adminCount > 3) {
            addIssue('medium', 'Many administrator accounts', `${adminCount} rows in ${table} have an admin-like role.`, 'Apply least privilege and limit permanent administrator accounts.');
          }
          // Masked usernames only; never export full account dumps.
          const [sample] = await conn.query<mysql.RowDataPacket[]>(
            `SELECT ${`\`${nameCol}\``} AS u, ${`\`${roleCol}\``} AS r FROM ${t} WHERE ${`\`${roleCol}\``} LIKE '%admin%' LIMIT 10`,
          );
          if (sample.length) notes.push(`Admin sample: ${sample.map((s) => `${maskEmail(String(s.u))} (${redact(String(s.r))})`).join(', ')}`);
        }
      }
    }

    onProgress(75, 'Checking server authentication hardening');
    const [vars] = await conn.query<mysql.RowDataPacket[]>('SHOW VARIABLES');
    const v = new Map(vars.map((r) => [String(r.Variable_name).toLowerCase(), String(r.Value)]));
    if (v.get('local_infile') === 'ON') addIssue('medium', 'LOCAL INFILE enabled', 'MySQL can read client-side files, which amplifies SQL injection impact.', 'Set local_infile=OFF unless required.');
    if (v.get('validate_password.policy') === undefined && v.get('validate_password_policy') === undefined) {
      addIssue('low', 'No password validation plugin configured', 'The server does not enforce password strength for database accounts.', 'Install validate_password and set a medium/strong policy.');
    }
    const plugin = v.get('default_authentication_plugin') ?? '';
    if (/mysql_native_password/i.test(plugin)) addIssue('medium', 'Legacy default authentication plugin', 'mysql_native_password uses weaker challenge-response than caching_sha2_password.', 'Set default_authentication_plugin=caching_sha2_password.');
    if (v.get('skip_networking') === 'OFF' && v.get('bind_address') === '0.0.0.0') {
      addIssue('high', 'MySQL listens on all interfaces', 'The database is reachable from the network; it should be bound to localhost in a XAMPP/desktop setup.', "Set bind-address=127.0.0.1 and use a firewall.");
    }
    const secureFile = v.get('secure_file_priv');
    if (secureFile === '' || secureFile === undefined) addIssue('low', 'secure_file_priv is unrestricted', 'File import/export can touch the whole filesystem.', "Set secure_file_priv to a dedicated directory or NULL.");

    onProgress(90, 'Writing audit results');
    const result: MysqlAuditResult = {
      server,
      database: cfg.database,
      tables: tableNames.length,
      userTables,
      issues,
      notes,
    };
    for (const issue of issues) {
      assessments.addFinding(assessmentId, profile.base_url ?? profile.name, {
        target: profile.name,
        name: issue.name,
        category: 'mysql',
        severity: issue.severity,
        confidence: 'high',
        endpoint: `${cfg.host}:${cfg.port}/${cfg.database}`,
        evidence: issue.detail,
        description: issue.detail,
        impact: 'Weak database configuration turns an application flaw into full data compromise.',
        reproduction: `Connect with the authorized read-only account and run: ${issue.name.includes('foreign') ? 'SHOW CREATE TABLE' : 'SHOW VARIABLES / SHOW GRANTS'}`,
        remediation: issue.remediation,
        references: ['https://dev.mysql.com/doc/refman/8.0/en/security.html', 'https://cheatsheetseries.owasp.org/cheatsheets/Database_Security_Cheat_Sheet.html'],
      });
    }
    db.run('INSERT INTO account_audits (id, assessment_id, source, summary, data, created_at) VALUES (?,?,?,?,?,?)', [
      randomUUID(),
      assessmentId,
      `mysql:${cfg.host}/${cfg.database}`,
      `${tableNames.length} tables, ${userTables.length} account tables, ${issues.length} issues`,
      JSON.stringify({ server, database: cfg.database, tables: tableNames.length, userTables, issues, notes }),
      isoNow(),
    ]);
    onProgress(100, 'Database audit complete');
    return result;
  } finally {
    await conn.end().catch(() => undefined);
  }
}
