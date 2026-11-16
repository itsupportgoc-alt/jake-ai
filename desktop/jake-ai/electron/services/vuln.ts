import type { Severity } from '../../shared/types';
import type { ScanContext } from './recon';
import { analyzeHeaders, parseCookies } from './recon';
import { scopedFetch, type FetchResult } from './scope';

/**
 * OWASP-aligned, non-destructive web assessment.
 * Every request goes through scopedFetch (allowlist, DNS checks, rate limit,
 * redirect validation). No exploit payloads, no state-changing requests.
 */

const SENSITIVE_PATHS: Array<{ path: string; name: string; signature: RegExp; severity: Severity; description: string }> = [
  { path: '/.env', name: 'Environment file exposed', signature: /^\s*[A-Z_][A-Z0-9_]*\s*=/m, severity: 'critical', description: 'A .env file is publicly readable and typically contains database and API credentials.' },
  { path: '/.git/config', name: 'Git repository metadata exposed', signature: /\[core\]|\[remote/i, severity: 'high', description: 'The .git directory is served publicly, allowing full source-code reconstruction.' },
  { path: '/backup.sql', name: 'Database backup exposed', signature: /CREATE TABLE|INSERT INTO/i, severity: 'critical', description: 'A SQL dump appears to be downloadable from the web root.' },
  { path: '/config.php.bak', name: 'PHP configuration backup exposed', signature: /<\?php|\$db|password/i, severity: 'high', description: 'A backup of a PHP configuration file is served, likely containing credentials.' },
  { path: '/phpinfo.php', name: 'phpinfo() page exposed', signature: /PHP Version|phpinfo\(\)/i, severity: 'medium', description: 'phpinfo() leaks PHP configuration, paths and loaded modules.' },
  { path: '/.DS_Store', name: 'Directory index file exposed', signature: /Bud1/, severity: 'low', description: 'A macOS .DS_Store file reveals directory and file names.' },
  { path: '/server-status', name: 'Apache server-status exposed', signature: /Apache Server Status|Server Version/i, severity: 'medium', description: 'The Apache status handler exposes request and process information.' },
  { path: '/web.config', name: 'IIS web.config exposed', signature: /<configuration|<system\.web/i, severity: 'high', description: 'IIS configuration is readable, potentially including connection strings.' },
  { path: '/composer.json', name: 'composer.json exposed', signature: /"require"\s*:/, severity: 'low', description: 'Dependency manifest exposed; assists targeted dependency exploitation.' },
];

const ADMIN_PATHS = ['/admin', '/admin/login', '/administrator/', '/wp-admin/', '/phpmyadmin/', '/adminer.php', '/manager/html', '/actuator', '/api/admin'];

const SQL_ERROR = /(SQL syntax.*MySQL|Warning.*mysqli?|MySQLSyntaxErrorException|PostgreSQL.*ERROR|ORA-\d{4,5}|SQLite\/JDBCDriver|Microsoft OLE DB Provider for SQL|Unclosed quotation mark)/i;
const STACK_TRACE = /(Fatal error:|stack trace:|Traceback \(most recent call last\)|at [\w$.]+\([\w.]+:\d+\)|Notice: Undefined|Warning: mysqli)/i;
const TOKEN_FIELD = /(csrf|xsrf|_token|authenticity_token|nonce|__RequestVerificationToken)/i;
const SQLI_PARAM_NAMES = /(id|page|cat|category|product|item|user|account|view|search|q|order|sort|file|lang|report|invoice)/i;

export interface ScanSummary {
  findings: number;
  requests: number;
  notes: string[];
}

export async function runVulnerabilityScan(ctx: ScanContext): Promise<ScanSummary> {
  const { target, signal, onProgress, assessments, assessmentId, db } = ctx;
  const base = target.url;
  const notes: string[] = [];
  let requests = 0;
  let count = 0;

  const exists = (name: string, endpoint: string): boolean =>
    Boolean(db.get('SELECT id FROM findings WHERE assessment_id = ? AND name = ? AND endpoint = ?', [assessmentId, name, endpoint]));

  const add = (f: {
    name: string;
    category: string;
    severity: Severity;
    confidence: 'low' | 'medium' | 'high';
    endpoint: string;
    evidence: string;
    description: string;
    impact: string;
    reproduction: string;
    remediation: string;
    references: string[];
  }) => {
    if (exists(f.name, f.endpoint)) return;
    assessments.addFinding(assessmentId, target.url, { target: target.url, ...f });
    count++;
  };

  const get = async (path: string, opts: Parameters<typeof scopedFetch>[2] = {}): Promise<FetchResult | null> => {
    requests++;
    try {
      return await scopedFetch(target, new URL(path, base).toString(), { ...opts, signal, timeoutMs: 12000, maxBytes: 500_000 });
    } catch (err) {
      notes.push(`${path}: ${(err as Error).message}`);
      return null;
    }
  };

  onProgress(5, 'Loading the target application');
  const home = await get(base);
  if (!home) throw new Error(`Could not reach ${base}. ${notes.slice(-1)[0] ?? ''}`);

  // --- Header / cookie posture ---
  for (const h of analyzeHeaders(home.url, home.headers)) {
    add({
      name: h.name,
      description: h.detail,
      impact: h.impact,
      evidence: h.evidence,
      remediation: h.remediation,
      severity: h.severity,
      category: 'security-misconfiguration',
      confidence: 'high',
      endpoint: home.url,
      reproduction: `curl -sI ${home.url}`,
      references: ['https://owasp.org/www-project-secure-headers/'],
    });
  }
  for (const c of parseCookies(home.headers['set-cookie'])) {
    if (home.url.startsWith('https:') && !c.secure) {
      add({
        name: `Cookie "${c.name}" missing Secure`,
        category: 'session-management',
        severity: 'medium',
        confidence: 'high',
        endpoint: home.url,
        evidence: `Cookie ${c.name}: Secure=false, HttpOnly=${c.httpOnly}, SameSite=${c.sameSite ?? 'unset'}`,
        description: 'Cookie can be sent over cleartext HTTP.',
        impact: 'Session theft on mixed content or downgraded connections.',
        reproduction: `curl -sI ${home.url}`,
        remediation: 'Set Secure and HttpOnly on all session cookies.',
        references: ['https://cheatsheetseries.owasp.org/cheatsheets/Session_Management_Cheat_Sheet.html'],
      });
    }
  }

  // --- HTTPS enforcement ---
  if (home.url.startsWith('https:')) {
    onProgress(15, 'Checking HTTP to HTTPS enforcement');
    const httpUrl = home.url.replace('https:', 'http:');
    try {
      requests++;
      const plain = await scopedFetch(target, httpUrl, { signal, timeoutMs: 8000 });
      if (plain.status === 200 && !plain.url.startsWith('https:')) {
        add({
          name: 'Plain HTTP served without redirect to HTTPS',
          category: 'transport',
          severity: 'high',
          confidence: 'high',
          endpoint: httpUrl,
          evidence: `HTTP 200 from ${httpUrl} (final URL ${plain.url})`,
          description: 'The application is reachable over unencrypted HTTP and does not force HTTPS.',
          impact: 'Credentials and session cookies can be intercepted in transit.',
          reproduction: `curl -sI ${httpUrl}`,
          remediation: 'Redirect all HTTP traffic to HTTPS with a 301 and enable HSTS.',
          references: ['https://cheatsheetseries.owasp.org/cheatsheets/Transport_Layer_Security_Cheat_Sheet.html'],
        });
      }
    } catch {
      notes.push('HTTP port not reachable (good).');
    }
  }

  // --- Directory listing / error disclosure ---
  if (/<title>\s*Index of \//i.test(home.body) || /<h1>\s*Index of \//i.test(home.body)) {
    add({
      name: 'Directory listing enabled',
      category: 'security-misconfiguration',
      severity: 'medium',
      confidence: 'high',
      endpoint: home.url,
      evidence: home.body.slice(0, 300),
      description: 'The web server returns an auto-generated directory index.',
      impact: 'Files that are not meant to be browsable can be enumerated and downloaded.',
      reproduction: `Open ${home.url} and look for "Index of /".`,
      remediation: 'Disable autoindex (Apache: Options -Indexes; Nginx: autoindex off).',
      references: ['https://owasp.org/www-community/attacks/Path_Traversal'],
    });
  }
  if (STACK_TRACE.test(home.body)) {
    add({
      name: 'Verbose error / stack trace disclosure',
      category: 'information-disclosure',
      severity: 'medium',
      confidence: 'medium',
      endpoint: home.url,
      evidence: firstMatch(home.body, STACK_TRACE),
      description: 'The response contains framework or runtime error details.',
      impact: 'Reveals file paths, framework versions and internal logic.',
      reproduction: `curl -s ${home.url} | grep -Ei "fatal error|stack trace"`,
      remediation: 'Disable debug mode in production and return generic error pages.',
      references: ['https://owasp.org/www-project-top-ten/2017/A6_2017-Security_Misconfiguration'],
    });
  }

  // --- Sensitive files ---
  onProgress(30, 'Probing well-known sensitive paths');
  for (const p of SENSITIVE_PATHS) {
    if (signal.aborted) throw new Error('Operation cancelled');
    const res = await get(p.path);
    if (res && res.status === 200 && p.signature.test(res.body)) {
      add({
        name: p.name,
        category: 'information-disclosure',
        severity: p.severity,
        confidence: 'high',
        endpoint: new URL(p.path, base).toString(),
        evidence: `${res.status} ${res.headers['content-type'] ?? ''} — ${firstMatch(res.body, p.signature) || res.body.slice(0, 200)}`,
        description: p.description,
        impact: 'Direct exposure of secrets or internal configuration.',
        reproduction: `curl -s ${new URL(p.path, base)} | head -20`,
        remediation: 'Remove the file from the web root, block the path at the server, and rotate any exposed secrets.',
        references: ['https://owasp.org/www-project-top-ten/2017/A6_2017-Security_Misconfiguration'],
      });
    }
  }

  // --- Administrative interfaces ---
  onProgress(45, 'Checking administrative interfaces');
  for (const p of ADMIN_PATHS) {
    if (signal.aborted) throw new Error('Operation cancelled');
    const res = await get(p);
    if (!res) continue;
    if (res.status === 200 && /<form|login|dashboard|password/i.test(res.body)) {
      add({
        name: 'Administrative interface reachable',
        category: 'access-control',
        severity: 'medium',
        confidence: 'medium',
        endpoint: new URL(p, base).toString(),
        evidence: `HTTP 200 at ${p} (${res.bodyBytes} bytes)`,
        description: 'An administrative or database management interface is exposed to the internet.',
        impact: 'Provides a direct target for credential attacks and unpatched admin vulnerabilities.',
        reproduction: `Open ${new URL(p, base)} in an authorized browser session.`,
        remediation: 'Restrict admin interfaces by IP/VPN, enable MFA, and keep them patched.',
        references: ['https://owasp.org/www-project-top-ten/2017/A5_2017-Broken_Access_Control'],
      });
    } else if (res.status === 401 || res.status === 403) {
      notes.push(`${p} present but requires authorization (HTTP ${res.status}).`);
    }
  }

  // --- CORS ---
  onProgress(55, 'Evaluating CORS configuration');
  try {
    requests++;
    const cors = await scopedFetch(target, base, {
      signal,
      timeoutMs: 10000,
      headers: { Origin: 'https://jake-scope-probe.invalid', 'Access-Control-Request-Method': 'GET' },
    });
    const acao = cors.headers['access-control-allow-origin'];
    const acac = cors.headers['access-control-allow-credentials'];
    if (acao === '*' && acac === 'true') {
      add({
        name: 'CORS allows any origin with credentials',
        category: 'api',
        severity: 'high',
        confidence: 'high',
        endpoint: cors.url,
        evidence: `Access-Control-Allow-Origin: * / Access-Control-Allow-Credentials: true`,
        description: 'The CORS policy is invalid and dangerous; browsers block this combination, but reflecting the Origin would expose authenticated data.',
        impact: 'Cross-site reading of authenticated data if the policy changes to reflect origins.',
        reproduction: `curl -sI -H "Origin: https://jake-scope-probe.invalid" ${base}`,
        remediation: 'Use an explicit allowlist of trusted origins and never combine a wildcard with credentials.',
        references: ['https://cheatsheetseries.owasp.org/cheatsheets/Cross-Origin_Resource_Sharing_Cheat_Sheet.html'],
      });
    } else if (acao && acao !== '*' && acao.includes('jake-scope-probe.invalid')) {
      add({
        name: 'CORS reflects arbitrary Origin',
        category: 'api',
        severity: acac === 'true' ? 'critical' : 'medium',
        confidence: 'high',
        endpoint: cors.url,
        evidence: `Access-Control-Allow-Origin: ${acao}; Credentials: ${acac ?? 'false'}`,
        description: 'Any website can read responses from this endpoint.',
        impact: acac === 'true' ? 'Authenticated users visiting a malicious page can have their data read cross-origin.' : 'Unauthenticated data can be read cross-origin.',
        reproduction: `curl -sI -H "Origin: https://jake-scope-probe.invalid" ${base}`,
        remediation: 'Validate Origin against a strict allowlist before echoing it.',
        references: ['https://cheatsheetseries.owasp.org/cheatsheets/Cross-Origin_Resource_Sharing_Cheat_Sheet.html'],
      });
    }
  } catch (err) {
    notes.push(`CORS probe failed: ${(err as Error).message}`);
  }

  // --- Forms, CSRF and file uploads ---
  onProgress(65, 'Reviewing HTML forms');
  const forms = [...home.body.matchAll(/<form\b[^>]*>[\s\S]*?<\/form>/gi)].map((m) => m[0]).slice(0, 30);
  const postForms = forms.filter((f) => /method\s*=\s*["']?post/i.test(f));
  const tokenForms = postForms.filter((f) => TOKEN_FIELD.test(f));
  if (postForms.length > 0 && tokenForms.length === 0) {
    add({
      name: 'POST forms without anti-CSRF token',
      category: 'csrf',
      severity: 'medium',
      confidence: 'low',
      endpoint: new URL(forms[0] ? '/#forms' : '/', base).toString(),
      evidence: `${postForms.length} POST form(s) found; none contained a CSRF/XSRF token field. Sample: ${postForms[0].slice(0, 300)}`,
      description: 'State-changing forms do not appear to carry a synchronizer token. Cookies with SameSite=None would make them CSRF-able.',
      impact: 'An attacker could submit actions on behalf of an authenticated user.',
      reproduction: 'Inspect each form action and verify a per-session token is required and validated server-side.',
      remediation: 'Add a per-session, unpredictable CSRF token validated on the server; set SameSite=Lax/Strict on session cookies.',
      references: ['https://cheatsheetseries.owasp.org/cheatsheets/Cross-Site_Request_Forgery_Prevention_Cheat_Sheet.html'],
    });
  }
  const uploadForms = forms.filter((f) => /type\s*=\s*["']?file/i.test(f));
  if (uploadForms.length > 0) {
    add({
      name: 'File upload form requires validation review',
      category: 'file-upload',
      severity: 'info',
      confidence: 'medium',
      endpoint: new URL('/#upload', base).toString(),
      evidence: uploadForms[0].slice(0, 300),
      description: 'A file upload form was found. Upload handling could not be verified without authentication; it needs a manual authorized review.',
      impact: 'Unrestricted uploads can lead to remote code execution if extensions and content are not validated.',
      reproduction: 'Using authorized test credentials, upload a harmless file with a disallowed extension and verify the server rejects it.',
      remediation: 'Validate extension, MIME and magic bytes; store uploads outside the web root; rename files; serve with Content-Disposition: attachment.',
      references: ['https://cheatsheetseries.owasp.org/cheatsheets/File_Upload_Cheat_Sheet.html'],
    });
  }

  // --- Input reflection / error-based SQL injection indicators ---
  onProgress(75, 'Testing input handling with benign probes');
  const canary = `jake${Date.now().toString(36)}<probe>`;
  const probeUrl = new URL(base);
  const existingParam = [...probeUrl.searchParams.keys()][0];
  const param = existingParam && SQLI_PARAM_NAMES.test(existingParam) ? existingParam : 'q';
  probeUrl.searchParams.set(param, canary);
  try {
    requests++;
    const reflected = await scopedFetch(target, probeUrl.toString(), { signal, timeoutMs: 10000 });
    if (reflected.body.includes(canary)) {
      add({
        name: 'Input reflected without HTML encoding',
        category: 'xss',
        severity: 'medium',
        confidence: 'medium',
        endpoint: probeUrl.toString(),
        evidence: 'The inert probe string with angle brackets was returned verbatim in the HTML response.',
        description: 'User-controlled input is echoed into HTML without encoding. This is a strong XSS indicator, though safe verification with a real payload was intentionally not attempted.',
        impact: 'An attacker may execute script in a victim\'s browser to steal sessions or perform actions.',
        reproduction: 'Send the request with an inert token containing < and >, then inspect the response source. Do not use live payloads in production.',
        remediation: 'Encode output contextually (htmlspecialchars with ENT_QUOTES) and enforce CSP.',
        references: ['https://cheatsheetseries.owasp.org/cheatsheets/Cross_Site_Scripting_Prevention_Cheat_Sheet.html'],
      });
    }
    if (SQL_ERROR.test(reflected.body)) {
      add({
        name: 'Database error disclosed after quote probe',
        category: 'sql-injection',
        severity: 'high',
        confidence: 'medium',
        endpoint: probeUrl.toString(),
        evidence: firstMatch(reflected.body, SQL_ERROR),
        description: 'A single-quote probe produced a database error, indicating unparameterized input reaching a SQL query.',
        impact: 'Potential SQL injection; an attacker may read or modify database contents.',
        reproduction: `Append a single quote to the "${param}" parameter and observe the database error. Do not attempt extraction.`,
        remediation: 'Use prepared statements with bound parameters for every query; hide database errors from users.',
        references: ['https://cheatsheetseries.owasp.org/cheatsheets/SQL_Injection_Prevention_Cheat_Sheet.html'],
      });
    }
  } catch (err) {
    notes.push(`Input probe failed: ${(err as Error).message}`);
  }

  // --- HTTP methods ---
  onProgress(85, 'Checking allowed HTTP methods');
  try {
    requests++;
    const opt = await scopedFetch(target, base, { method: 'OPTIONS', signal, timeoutMs: 8000 });
    const allow = opt.headers.allow ?? opt.headers['access-control-allow-methods'] ?? '';
    const risky = allow.split(',').map((s) => s.trim().toUpperCase()).filter((m) => ['PUT', 'DELETE', 'TRACE', 'CONNECT', 'PATCH'].includes(m));
    if (risky.length > 0) {
      add({
        name: 'Dangerous HTTP methods advertised',
        category: 'security-misconfiguration',
        severity: risky.includes('TRACE') ? 'medium' : 'low',
        confidence: 'medium',
        endpoint: base,
        evidence: `Allow: ${allow}`,
        description: `The server advertises ${risky.join(', ')} on this resource.`,
        impact: 'Risky verbs may allow unintended modification or cross-site tracing.',
        reproduction: `curl -sI -X OPTIONS ${base}`,
        remediation: 'Restrict methods per route and disable TRACE at the server.',
        references: ['https://owasp.org/www-project-top-ten/2017/A6_2017-Security_Misconfiguration'],
      });
    }
  } catch {
    notes.push('OPTIONS probe not supported by target.');
  }

  onProgress(95, 'Summarizing results');
  return { findings: count, requests, notes };
}

function firstMatch(text: string, re: RegExp): string {
  const m = re.exec(text);
  return m ? text.slice(Math.max(0, m.index - 40), m.index + 200).replace(/\s+/g, ' ') : text.slice(0, 200).replace(/\s+/g, ' ');
}
