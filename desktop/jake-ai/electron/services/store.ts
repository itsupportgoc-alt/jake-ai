import { safeStorage } from 'electron';
import fs from 'node:fs';
import path from 'node:path';
import type { Database } from './db';
import { isoNow } from './db';
import type { Settings } from '../../shared/types';
import { redact } from './redact';
import { getProvider, DEFAULT_PROVIDER_ID } from '../../shared/providers';

const DEFAULTS = {
  aiModel: 'qwen2.5:7b',
  temperature: 0.2,
  maxTokens: 2048,
  defaultRateLimit: 2,
  requestTimeoutMs: 15000,
  autoApproveLevel: 1,
};

function secretKey(providerId: string): string {
  return `ai.apiKey.${providerId}`;
}

export class Store {
  constructor(private db: Database, private dir: string) {}

  getSetting(key: string): string | null {
    const row = this.db.get<{ value: string }>('SELECT value FROM settings WHERE key = ?', [key]);
    return row?.value ?? null;
  }

  setSetting(key: string, value: string): void {
    this.db.run('INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)', [key, value]);
  }

  secureAvailable(): boolean {
    try {
      return safeStorage.isEncryptionAvailable();
    } catch {
      return false;
    }
  }

  /** Encrypts with the OS credential store (DPAPI-backed on Windows). */
  putSecret(key: string, plaintext: string): void {
    if (!this.secureAvailable()) throw new Error('OS secure storage is unavailable; refusing to store a secret in plaintext.');
    const enc = safeStorage.encryptString(plaintext).toString('base64');
    this.setSetting(key, enc);
  }

  getSecret(key: string): string | null {
    const enc = this.getSetting(key);
    if (!enc) return null;
    if (!this.secureAvailable()) return null;
    try {
      return safeStorage.decryptString(Buffer.from(enc, 'base64'));
    } catch {
      console.warn(`Could not decrypt secret ${key}`);
      return null;
    }
  }

  clearSecret(key: string): void {
    this.db.run('DELETE FROM settings WHERE key = ?', [key]);
  }

  hasSecret(key: string): boolean {
    return Boolean(this.getSetting(key));
  }

  /** Active provider id, migrating older installs that only had a base URL. */
  providerId(): string {
    const stored = this.getSetting('ai.provider');
    if (stored) return getProvider(stored).id;
    return this.getSetting('ai.baseUrl') ? 'custom' : DEFAULT_PROVIDER_ID;
  }

  getSettings(): Settings {
    const num = (k: string, d: number) => {
      const v = this.getSetting(k);
      return v === null ? d : Number(v);
    };
    const providerId = this.providerId();
    const provider = getProvider(providerId);
    const legacyKey = providerId === 'custom' && this.hasSecret('ai.apiKey');
    return {
      aiProvider: provider.id,
      aiModel: this.getSetting('ai.model') ?? provider.defaultModel ?? DEFAULTS.aiModel,
      temperature: num('ai.temperature', DEFAULTS.temperature),
      maxTokens: num('ai.maxTokens', DEFAULTS.maxTokens),
      aiBaseUrl: this.getSetting('ai.baseUrl') ?? provider.baseUrl,
      keyConfigured: this.hasSecret(secretKey(provider.id)) || legacyKey,
      requiresKey: provider.requiresKey,
      secureStorage: this.secureAvailable(),
      defaultRateLimit: num('scan.rateLimit', DEFAULTS.defaultRateLimit),
      requestTimeoutMs: num('scan.timeoutMs', DEFAULTS.requestTimeoutMs),
      autoApproveLevel: num('security.autoApproveLevel', DEFAULTS.autoApproveLevel),
      reportsDir: this.reportsDir(),
    };
  }

  saveSettings(patch: Partial<Settings> & { apiKey?: string; clearApiKey?: boolean }): Settings {
    const providerId = patch.aiProvider ? getProvider(patch.aiProvider).id : this.providerId();
    if (patch.aiProvider) this.setSetting('ai.provider', providerId);

    if (patch.apiKey !== undefined && patch.apiKey !== '') this.putSecret(secretKey(providerId), patch.apiKey.trim());
    if (patch.clearApiKey) {
      this.clearSecret(secretKey(providerId));
      this.clearSecret('ai.apiKey');
    }
    if (patch.aiModel) this.setSetting('ai.model', String(patch.aiModel).slice(0, 160));

    if (patch.aiBaseUrl) {
      const url = String(patch.aiBaseUrl).trim().replace(/\/+$/, '');
      let parsed: URL;
      try {
        parsed = new URL(url);
      } catch {
        throw new Error('The AI base URL must be a valid http(s) URL (for example http://127.0.0.1:11434/v1).');
      }
      if (!['http:', 'https:'].includes(parsed.protocol)) throw new Error('The AI base URL must use http or https.');
      this.setSetting('ai.baseUrl', url.slice(0, 300));
    }
    if (patch.temperature !== undefined) this.setSetting('ai.temperature', String(clamp(Number(patch.temperature), 0, 2)));
    if (patch.maxTokens !== undefined) this.setSetting('ai.maxTokens', String(Math.round(clamp(Number(patch.maxTokens), 128, 8192))));
    if (patch.defaultRateLimit !== undefined) this.setSetting('scan.rateLimit', String(clamp(Number(patch.defaultRateLimit), 0.2, 20)));
    if (patch.requestTimeoutMs !== undefined) this.setSetting('scan.timeoutMs', String(Math.round(clamp(Number(patch.requestTimeoutMs), 2000, 120000))));
    if (patch.autoApproveLevel !== undefined) this.setSetting('security.autoApproveLevel', String(Math.round(clamp(Number(patch.autoApproveLevel), 1, 4))));
    this.db.run('INSERT OR REPLACE INTO audit_logs (actor, action, target, level, detail, created_at) VALUES (?,?,?,?,?,?)', [
      'user',
      'settings.update',
      null,
      1,
      `Updated settings: ${Object.keys(patch).filter((k) => k !== 'apiKey').join(', ')} (provider ${providerId})`,
      isoNow(),
    ]);
    return this.getSettings();
  }

  reportsDir(): string {
    const dir = path.join(this.dir, 'reports');
    fs.mkdirSync(dir, { recursive: true });
    return dir;
  }
}

function clamp(n: number, min: number, max: number): number {
  if (!Number.isFinite(n)) return min;
  return Math.min(max, Math.max(min, n));
}

export function logSafe(scope: string, message: string, detail?: unknown): void {
  const line = detail === undefined ? message : `${message} ${redact(detail)}`;
  console.log(`[${scope}] ${line.slice(0, 4000)}`);
}
