/**
 * Secret redaction. Everything written to logs, audit entries, tool output or
 * sent to the AI provider passes through here first.
 */
const PATTERNS: Array<[RegExp, string]> = [
  [/(authorization\s*:\s*(?:bearer|basic|token)\s+)[A-Za-z0-9._~+/=-]{6,}/gi, '$1[REDACTED]'],
  [/((?:api[_-]?key|apikey|access[_-]?token|secret|client[_-]?secret|password|passwd|pwd)\s*["']?\s*[:=]\s*["']?)([^\s"',;]{3,})/gi, '$1[REDACTED]'],
  [/\b(sk|xai|gsk|pk|rk)-[A-Za-z0-9_-]{16,}\b/g, '[REDACTED_KEY]'],
  [/(session(?:_id)?|sid|csrf[_-]?token|xsrf[_-]?token)=([^;\s&"']{6,})/gi, '$1=[REDACTED]'],
  [/-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----/g, '[REDACTED_PRIVATE_KEY]'],
  [/\b[A-Za-z0-9+/=]{40,}\b/g, '[REDACTED_BLOB]'],
];

const SENSITIVE_KEYS = /^(password|passwd|pwd|hash|password_hash|secret|token|access_token|refresh_token|api_key|apikey|session|cookie|authorization|ssn|credit_card)$/i;

export function redact(input: unknown, depth = 0): string {
  if (input === null || input === undefined) return '';
  let text = typeof input === 'string' ? input : safeStringify(input);
  if (depth < 3 && (text.startsWith('{') || text.startsWith('['))) {
    try {
      text = safeStringify(redactObject(JSON.parse(text)));
    } catch {
      /* not JSON, fall through to regex redaction */
    }
  }
  for (const [re, replacement] of PATTERNS) text = text.replace(re, replacement);
  return text;
}

export function redactObject(value: unknown, depth = 0): unknown {
  if (depth > 6) return '[depth-limit]';
  if (Array.isArray(value)) return value.slice(0, 500).map((v) => redactObject(v, depth + 1));
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
      if (SENSITIVE_KEYS.test(k)) out[k] = '[REDACTED]';
      else if (typeof v === 'string' && /^[A-Za-z0-9+/=]{32,}$/.test(v)) out[k] = '[REDACTED]';
      else out[k] = redactObject(v, depth + 1);
    }
    return out;
  }
  return value;
}

export function safeStringify(value: unknown): string {
  try {
    return JSON.stringify(value, (_k, v) => (typeof v === 'bigint' ? v.toString() : v));
  } catch {
    return String(value);
  }
}

/** Minimal masking for account identifiers shown in reports. */
export function maskEmail(value: string): string {
  if (!value.includes('@')) return value.length > 2 ? `${value[0]}***${value.slice(-1)}` : '***';
  const [name, domain] = value.split('@');
  return `${name.slice(0, 1)}***@${domain}`;
}
