import fs from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import type { Severity } from '../../shared/types';
import type { Database } from './db';
import { isoNow } from './db';
import type { Assessments } from './assessments';
import { redact } from './redact';

export interface PhpIssue {
  file: string;
  line: number;
  ruleId: string;
  severity: Severity;
  confidence: 'low' | 'medium' | 'high';
  snippet: string;
  message: string;
  impact: string;
  remediation: string;
  proposedFix?: string;
  references: string[];
}

interface Rule {
  id: string;
  severity: Severity;
  confidence: 'low' | 'medium' | 'high';
  message: string;
  impact: string;
  remediation: string;
  test: (file: string, content: string) => Array<{ line: number; snippet: string; proposedFix?: string }>;
  references: string[];
}

const PHP_REFS = 'https://cheatsheetseries.owasp.org/cheatsheets/PHP_Configuration_Cheat_Sheet.html';
const SQLI_REFS = 'https://cheatsheetseries.owasp.org/cheatsheets/SQL_Injection_Prevention_Cheat_Sheet.html';
const XSS_REFS = 'https://cheatsheetseries.owasp.org/cheatsheets/Cross_Site_Scripting_Prevention_Cheat_Sheet.html';
const CSRF_REFS = 'https://cheatsheetseries.owasp.org/cheatsheets/Cross-Site_Request_Forgery_Prevention_Cheat_Sheet.html';

function lines(content: string): string[] {
  return content.split(/\r?\n/);
}

function matchLines(content: string, re: RegExp, skipComments = true): Array<{ line: number; snippet: string }> {
  const out: Array<{ line: number; snippet: string }> = [];
  lines(content).forEach((line, i) => {
    if (skipComments && /^\s*(\/\/|#|\*|\/\*)/.test(line)) return;
    if (re.test(line)) out.push({ line: i + 1, snippet: line.trim().slice(0, 400) });
    re.lastIndex = 0;
  });
  return out;
}

const RULES: Rule[] = [
  {
    id: 'sql-injection',
    severity: 'critical',
    confidence: 'high',
    message: 'Superglobal input is interpolated into a SQL query without parameter binding.',
    impact: 'An attacker can alter the query, read or modify arbitrary data, and possibly execute database functions.',
    remediation: 'Use PDO or mysqli prepared statements with bound parameters. Never concatenate request data into SQL.',
    references: [SQLI_REFS],
    test: (file, content) => {
      if (!/\.(php|inc)$/i.test(file)) return [];
      const out: Array<{ line: number; snippet: string }> = [];
      // Track whether the file handles request input at all, so a query built on
      // a later line is still attributed to user input without flagging files
      // that only use bound parameters.
      const fileUsesRequestInput = /\$_(GET|POST|REQUEST|COOKIE|FILES)\b/.test(content);
      lines(content).forEach((line, i) => {
        const usesSuperglobal = /\$_(GET|POST|REQUEST|COOKIE|FILES)\b/.test(line);
        const queryContext = /(select|insert|update|delete|where|order\s+by|limit)\b/i.test(line) || /(mysqli_query|mysql_query|->query)/i.test(line);
        const concatenated = /(\.\s*\$|"\s*\.\s*\$|\$[A-Za-z_]\w*\s*\.\s*["'])/.test(line) || /\{\$/.test(line) || /"[^"]*\$[A-Za-z_]\w*[^"]*"/.test(line);
        const usesPlaceholder = /\?\s*['"]|:\w+\s*['"]/.test(line) || /->prepare\s*\(/i.test(line) || /\bbind_param\b/i.test(line);
        const suspicious = (usesSuperglobal || fileUsesRequestInput) && concatenated && queryContext;
        if (suspicious && !usesPlaceholder && !/htmlspecialchars|htmlentities/i.test(line)) {
          out.push({ line: i + 1, snippet: line.trim().slice(0, 400) });
        }
      });
      return out.slice(0, 40);
    },
  },
  {
    id: 'weak-password-hash',
    severity: 'high',
    confidence: 'high',
    message: 'A fast, unsalted hash (md5/sha1) appears to be used for passwords.',
    impact: 'Hashed passwords can be cracked quickly, exposing user credentials reused elsewhere.',
    remediation: 'Use password_hash($password, PASSWORD_DEFAULT) and verify with password_verify().',
    references: ['https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html'],
    test: (file, content) => {
      if (!/\.php$/i.test(file)) return [];
      return matchLines(content, /(md5|sha1|crc32)\s*\([^)]*(pass|pwd|secret)/i).slice(0, 20);
    },
  },
  {
    id: 'unencoded-output',
    severity: 'medium',
    confidence: 'medium',
    message: 'Request data is echoed to HTML without output encoding.',
    impact: 'Cross-site scripting: attacker-supplied script runs in a victim browser.',
    remediation: "Escape output with htmlspecialchars($value, ENT_QUOTES, 'UTF-8') in the correct context.",
    references: [XSS_REFS],
    test: (file, content) => {
      if (!/\.(php|inc)$/i.test(file)) return [];
      const out: Array<{ line: number; snippet: string; proposedFix?: string }> = [];
      lines(content).forEach((line, i) => {
        const m = /(echo|print)\s+(\(?\s*\$_(GET|POST|REQUEST|COOKIE)\s*\[\s*['"]([^'"]+)['"]\s*\]\s*\)?)/i.exec(line);
        if (m && !/htmlspecialchars|htmlentities/i.test(line)) {
          out.push({
            line: i + 1,
            snippet: line.trim().slice(0, 400),
            proposedFix: line.replace(m[2], `htmlspecialchars(${m[2]}, ENT_QUOTES, 'UTF-8')`),
          });
        }
      });
      return out.slice(0, 40);
    },
  },
  {
    id: 'hardcoded-secret',
    severity: 'high',
    confidence: 'medium',
    message: 'A credential appears to be hardcoded in source code.',
    impact: 'Anyone with repository or backup access obtains production credentials.',
    remediation: 'Load secrets from environment variables or a secret manager; rotate the exposed value.',
    references: [PHP_REFS],
    test: (file, content) => {
      if (!/\.(php|inc|ini|env)$/i.test(file)) return [];
      const out = matchLines(content, /\$(password|passwd|db_pass|dbpass|api_key|apikey|secret|token)\s*=\s*['"][^'"]{3,}['"]/i);
      const defines = matchLines(content, /define\s*\(\s*['"](DB_PASSWORD|DB_PASS|API_KEY|SECRET)['"]\s*,\s*['"][^'"]{3,}['"]/i);
      return [...out, ...defines].slice(0, 20);
    },
  },
  {
    id: 'command-execution',
    severity: 'critical',
    confidence: 'high',
    message: 'Variable input reaches a shell-executing function.',
    impact: 'If user-controlled, this is remote code execution on the server.',
    remediation: 'Avoid shell functions; use native PHP APIs. If unavoidable, use escapeshellarg() with a strict allowlist.',
    references: ['https://cheatsheetseries.owasp.org/cheatsheets/OS_Command_Injection_Defense_Cheat_Sheet.html'],
    test: (file, content) => {
      if (!/\.php$/i.test(file)) return [];
      return matchLines(content, /\b(eval|system|exec|shell_exec|passthru|popen|proc_open)\s*\(\s*[^)]*\$/i).slice(0, 20);
    },
  },
  {
    id: 'file-upload',
    severity: 'high',
    confidence: 'low',
    message: 'Uploaded files are moved without visible type/extension validation.',
    impact: 'An uploaded PHP file can lead to remote code execution if stored in the web root.',
    remediation: 'Validate extension allowlist, MIME and magic bytes; randomise names; store outside the web root.',
    references: ['https://cheatsheetseries.owasp.org/cheatsheets/File_Upload_Cheat_Sheet.html'],
    test: (file, content) => {
      if (!/\.php$/i.test(file)) return [];
      if (!/move_uploaded_file\s*\(/i.test(content)) return [];
      if (/pathinfo\s*\(|mime_content_type|finfo|in_array\s*\([^)]*ext|getimagesize/i.test(content)) return [];
      return matchLines(content, /move_uploaded_file\s*\(/i).slice(0, 5);
    },
  },
  {
    id: 'csrf-missing',
    severity: 'medium',
    confidence: 'low',
    message: 'POST data is processed without a visible CSRF token check.',
    impact: 'Another site can submit state-changing requests as the logged-in user.',
    remediation: 'Require and validate a per-session CSRF token for every state-changing request; set SameSite cookies.',
    references: [CSRF_REFS],
    test: (file, content) => {
      if (!/\.php$/i.test(file)) return [];
      if (!/\$_POST\b/.test(content)) return [];
      if (/csrf|xsrf|_token|hash_equals\s*\(\s*\$_(POST|SESSION)/i.test(content)) return [];
      return matchLines(content, /\$_POST\s*\[/i).slice(0, 1);
    },
  },
  {
    id: 'error-disclosure',
    severity: 'low',
    confidence: 'high',
    message: 'Error display or raw database errors are enabled.',
    impact: 'Leaks file paths, SQL and framework details to users.',
    remediation: "Set display_errors=Off in production and log errors server-side; never echo mysqli_error output.",
    references: [PHP_REFS],
    test: (file, content) => {
      if (!/\.(php|ini)$/i.test(file)) return [];
      return [
        ...matchLines(content, /ini_set\s*\(\s*['"]display_errors['"]\s*,\s*(['"]?1['"]?|['"]?On['"]?)/i),
        ...matchLines(content, /(mysqli_error|mysql_error|->error)\s*(\(|\.)/i),
        ...matchLines(content, /^\s*display_errors\s*=\s*On/i),
      ].slice(0, 10);
    },
  },
  {
    id: 'session-hardening',
    severity: 'medium',
    confidence: 'low',
    message: 'Session handling lacks visible cookie hardening or ID rotation.',
    impact: 'Session fixation and cookie theft become more likely.',
    remediation: "Call session_set_cookie_params(['httponly'=>true,'secure'=>true,'samesite'=>'Lax']) and session_regenerate_id(true) after login.",
    references: ['https://cheatsheetseries.owasp.org/cheatsheets/Session_Management_Cheat_Sheet.html'],
    test: (file, content) => {
      if (!/\.php$/i.test(file)) return [];
      if (!/session_start\s*\(/i.test(content)) return [];
      if (/session_set_cookie_params|session_regenerate_id|ini_set\s*\(\s*['"]session\.cookie_httponly/i.test(content)) return [];
      return matchLines(content, /session_start\s*\(/i).slice(0, 2);
    },
  },
  {
    id: 'local-file-inclusion',
    severity: 'high',
    confidence: 'medium',
    message: 'Include/require uses variable input.',
    impact: 'Local or remote file inclusion can execute attacker-controlled code.',
    remediation: 'Include only fixed, allowlisted paths; never pass request data to include/require.',
    references: ['https://cheatsheetseries.owasp.org/cheatsheets/File_Upload_Cheat_Sheet.html'],
    test: (file, content) => {
      if (!/\.php$/i.test(file)) return [];
      return matchLines(content, /\b(include|include_once|require|require_once)\s*\(?\s*\$_(GET|POST|REQUEST|COOKIE)/i).slice(0, 10);
    },
  },
  {
    id: 'sql-error-echo',
    severity: 'medium',
    confidence: 'high',
    message: 'Database error text is rendered to the response.',
    impact: 'Reveals schema and confirms injection points.',
    remediation: 'Log errors server-side; return a generic message to users.',
    references: [PHP_REFS],
    test: (file, content) => matchLines(content, /(echo|print|die|exit)\s*\(?[^;]*(mysqli_error|mysql_error|pg_last_error)/i).slice(0, 10),
  },
];

export interface PhpAuditResult {
  root: string;
  filesScanned: number;
  issues: PhpIssue[];
  truncated: boolean;
}

const SENSITIVE_FILE = /(^\.env|\.sql$|\.bak$|\.old$|\.zip$|\.tar\.gz$|id_rsa|\.pem$)/i;
const SKIP_DIRS = new Set(['node_modules', '.git', 'vendor', 'cache', 'storage', 'logs', '.idea', '.vscode']);
const MAX_FILE = 1_500_000;

export function auditPhpDirectory(root: string): PhpAuditResult {
  const resolved = path.resolve(root);
  if (!fs.existsSync(resolved) || !fs.statSync(resolved).isDirectory()) throw new Error(`Directory not found: ${resolved}`);
  const issues: PhpIssue[] = [];
  let filesScanned = 0;
  let visited = 0;
  let truncated = false;

  const walk = (dir: string, depth: number) => {
    if (depth > 8 || truncated) return;
    let entries: fs.Dirent[];
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      if (truncated) return;
      if (visited++ > 6000) {
        truncated = true;
        return;
      }
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        if (SKIP_DIRS.has(entry.name)) continue;
        walk(full, depth + 1);
        continue;
      }
      if (SENSITIVE_FILE.test(entry.name)) {
        const rel = path.relative(resolved, full);
        issues.push({
          file: rel,
          line: 1,
          ruleId: 'sensitive-file-in-project',
          severity: entry.name.includes('.sql') || entry.name.startsWith('.env') ? 'high' : 'medium',
          confidence: 'high',
          snippet: entry.name,
          message: 'A sensitive file (environment, SQL dump, backup or key) is present in the project directory.',
          impact: 'If the web root serves this directory, secrets or full data dumps are downloadable.',
          remediation: 'Remove it from the web root, add it to .gitignore, and rotate exposed credentials.',
          references: ['https://owasp.org/www-project-top-ten/2017/A6_2017-Security_Misconfiguration'],
        });
        continue;
      }
      if (!/\.(php|inc|phtml|ini|module)$/i.test(entry.name) && entry.name !== '.htaccess') continue;
      try {
        const stat = fs.statSync(full);
        if (stat.size > MAX_FILE) continue;
        const content = fs.readFileSync(full, 'utf8');
        filesScanned++;
        const rel = path.relative(resolved, full);
        for (const rule of RULES) {
          for (const hit of rule.test(entry.name, content)) {
            issues.push({
              file: rel,
              line: hit.line,
              ruleId: rule.id,
              severity: rule.severity,
              confidence: rule.confidence,
              snippet: hit.snippet,
              message: rule.message,
              impact: rule.impact,
              remediation: rule.remediation,
              proposedFix: hit.proposedFix,
              references: rule.references,
            });
          }
        }
        // Prepared-statement credit is intentionally not a finding; it is reported as a positive note by the UI.
      } catch {
        /* skip unreadable file */
      }
    }
  };
  walk(resolved, 0);
  issues.sort((a, b) => severityOrder(b.severity) - severityOrder(a.severity));
  return { root: resolved, filesScanned, issues: issues.slice(0, 400), truncated };
}

function severityOrder(s: Severity): number {
  return { critical: 5, high: 4, medium: 3, low: 2, info: 1 }[s];
}

export async function runPhpAudit(opts: {
  db: Database;
  assessments: Assessments;
  directory: string;
  assessmentId: string;
  targetLabel: string;
  onProgress: (p: number, msg: string) => void;
}): Promise<PhpAuditResult> {
  const { db, assessments, assessmentId, targetLabel, directory, onProgress } = opts;
  onProgress(10, 'Walking PHP project files');
  const result = auditPhpDirectory(directory);
  onProgress(70, `Analyzing ${result.filesScanned} PHP files`);
  const grouped = new Map<string, PhpIssue[]>();
  for (const i of result.issues) {
    const key = `${i.ruleId}:${i.message}`;
    grouped.set(key, [...(grouped.get(key) ?? []), i]);
  }
  for (const [, group] of grouped) {
    const first = group[0];
    const files = [...new Set(group.map((g) => g.file))];
    assessments.addFinding(assessmentId, targetLabel, {
      target: targetLabel,
      name: first.message.replace(/\.$/, ''),
      category: 'php',
      severity: first.severity,
      confidence: first.confidence,
      endpoint: files.slice(0, 10).join(', '),
      evidence: group.slice(0, 6).map((g) => `${g.file}:${g.line} → ${redact(g.snippet)}`).join('\n'),
      description: `${first.message} Affected files: ${files.slice(0, 20).join(', ')}${files.length > 20 ? '…' : ''}`,
      impact: first.impact,
      reproduction: `Open ${files[0]}:${first.line} in the PHP editor and review the highlighted line.`,
      remediation: first.remediation,
      references: first.references,
    });
  }
  db.run('INSERT INTO account_audits (id, assessment_id, source, summary, data, created_at) VALUES (?,?,?,?,?,?)', [
    randomUUID(),
    assessmentId,
    `php:${result.root}`,
    `${result.filesScanned} PHP files scanned, ${result.issues.length} issues`,
    JSON.stringify({ filesScanned: result.filesScanned, issues: result.issues.slice(0, 200), truncated: result.truncated }),
    isoNow(),
  ]);
  onProgress(100, 'PHP review complete');
  return result;
}
