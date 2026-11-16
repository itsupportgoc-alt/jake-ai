import type { Store } from './store';
import { redact } from './redact';
import { getProvider, pickInstalledModel, type AiProvider } from '../../shared/providers';

export interface ToolSchema {
  type: 'function';
  function: { name: string; description: string; parameters: Record<string, unknown> };
}

export interface ChatMessage {
  role: 'system' | 'user' | 'assistant' | 'tool';
  content: string | null;
  tool_calls?: Array<{ id: string; type: 'function'; function: { name: string; arguments: string } }>;
  tool_call_id?: string;
  name?: string;
}

export interface StreamEvent {
  delta?: string;
  reasoning?: string;
  toolCalls?: ChatMessage['tool_calls'];
  finishReason?: string;
  usage?: { prompt_tokens?: number; completion_tokens?: number; total_tokens?: number };
}

export interface ConnectionTest {
  ok: boolean;
  message: string;
  latencyMs: number;
  model: string;
  provider: string;
  models?: string[];
}

export class AiError extends Error {
  constructor(message: string, public status?: number) {
    super(message);
    this.name = 'AiError';
  }
}

/**
 * OpenAI-compatible AI client for free/local and optional cloud providers.
 * Runs only in the main process. API keys (only needed by cloud providers) are
 * read from OS secure storage and never leave this process.
 * Prompt content is redacted before transmission.
 */
export class AiClient {
  private resolved: { key: string; model: string; models: string[]; at: number } | null = null;

  constructor(private store: Store) {}

  provider(): AiProvider {
    return getProvider(this.store.getSetting('ai.provider'));
  }

  baseUrl(): string {
    return (this.store.getSetting('ai.baseUrl') ?? this.provider().baseUrl).replace(/\/+$/, '');
  }

  model(): string {
    return this.store.getSetting('ai.model') ?? this.provider().defaultModel;
  }

  requiresKey(): boolean {
    return this.provider().requiresKey;
  }

  hasKey(): boolean {
    const provider = this.provider();
    if (this.store.getSecret(`ai.apiKey.${provider.id}`)) return true;
    // Backwards compatibility with the single legacy key slot.
    return provider.id === 'custom' && Boolean(this.store.getSecret('ai.apiKey'));
  }

  private key(): string | null {
    const provider = this.provider();
    const stored = this.store.getSecret(`ai.apiKey.${provider.id}`) ?? (provider.id === 'custom' ? this.store.getSecret('ai.apiKey') : null);
    if (stored) return stored;
    if (provider.requiresKey) {
      throw new AiError(`${provider.label} requires an API key. Open Settings → AI provider and add one.`);
    }
    return null;
  }

  private headers(): Record<string, string> {
    const key = this.key();
    return {
      'Content-Type': 'application/json',
      ...(key ? { Authorization: `Bearer ${key}` } : {}),
    };
  }

  private async request(body: unknown, signal?: AbortSignal): Promise<Response> {
    const provider = this.provider();
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 180_000);
    const onAbort = () => controller.abort();
    signal?.addEventListener('abort', onAbort, { once: true });
    try {
      const res = await fetch(`${this.baseUrl()}/chat/completions`, {
        method: 'POST',
        headers: this.headers(),
        body: JSON.stringify(body),
        signal: controller.signal,
      });
      if (!res.ok) {
        let detail = '';
        try {
          const j = (await res.json()) as { error?: { message?: string } | string; message?: string };
          detail = typeof j.error === 'string' ? j.error : (j.error?.message ?? j.message ?? '');
        } catch {
          detail = await res.text().catch(() => '');
        }
        const hint: Record<number, string> = {
          400: `The request was rejected by ${provider.label} (check the model name and that it supports chat + tool calling).`,
          401: `${provider.label} rejected the API key. Verify it in Settings.`,
          402: `${provider.label} reports insufficient balance/quota.`,
          403: `${provider.label} refused access for this key, region or model.`,
          404: `Model or endpoint not found at ${this.baseUrl()}. Check the base URL and model name.`,
          429: `${provider.label} rate limit reached (free tiers are limited). Wait and retry, or switch models.`,
          500: `${provider.label} reported an internal error. Retry shortly.`,
          502: `${provider.label} gateway error. Retry shortly.`,
          503: `${provider.label} is overloaded. Retry shortly or switch models.`,
        };
        throw new AiError(`${hint[res.status] ?? `${provider.label} returned HTTP ${res.status}.`}${detail ? ` (${redact(detail).slice(0, 300)})` : ''}`, res.status);
      }
      return res;
    } catch (err) {
      if ((err as Error).name === 'AbortError') throw new AiError('The AI request was cancelled or timed out.');
      if (err instanceof AiError) throw err;
      const localHint = provider.local ? ` Make sure ${provider.label} is running. ${provider.setup}` : '';
      throw new AiError(`Could not reach ${provider.label} at ${this.baseUrl()}: ${(err as Error).message}.${localHint}`);
    } finally {
      clearTimeout(timeout);
      signal?.removeEventListener('abort', onAbort);
    }
  }

  /** List models exposed by the provider (works for Ollama and OpenAI-compatible servers). */
  async listModels(): Promise<string[]> {
    const provider = this.provider();
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 12000);
    try {
      const res = await fetch(`${this.baseUrl()}/models`, { headers: this.headers(), signal: controller.signal });
      if (res.ok) {
        const json = (await res.json()) as { data?: Array<{ id?: string }> } | Array<{ id?: string }>;
        const rows = Array.isArray(json) ? json : (json.data ?? []);
        return rows.map((m) => String(m.id ?? '')).filter(Boolean).sort();
      }
      throw new AiError(`${provider.label} returned HTTP ${res.status} when listing models.`, res.status);
    } catch (err) {
      // Ollama also exposes a native tags endpoint; fall back to it when the
      // OpenAI-compatible route is unavailable on older versions.
      if (provider.id === 'ollama') {
        try {
          const root = this.baseUrl().replace(/\/v1$/, '');
          const res = await fetch(`${root}/api/tags`, { signal: controller.signal });
          if (res.ok) {
            const json = (await res.json()) as { models?: Array<{ name?: string }> };
            return (json.models ?? []).map((m) => String(m.name ?? '')).filter(Boolean).sort();
          }
        } catch {
          /* fall through to the original error */
        }
      }
      if (err instanceof AiError) throw err;
      const localHint = provider.local ? ` Start ${provider.label} first. ${provider.setup}` : '';
      throw new AiError(`Could not reach ${provider.label} at ${this.baseUrl()}: ${(err as Error).message}.${localHint}`);
    } finally {
      clearTimeout(timer);
    }
  }

  /**
   * Resolve the model to actually use. For local servers, if the configured
   * model is missing, an installed model is selected and persisted so the
   * Settings screen and Chat badge stay accurate.
   */
  async effectiveModel(): Promise<{ model: string; repairedFrom?: string; models?: string[]; listError?: string }> {
    const provider = this.provider();
    const configured = this.model();
    if (!provider.local) return { model: configured };
    const cacheKey = `${provider.id}|${this.baseUrl()}|${configured}`;
    if (this.resolved && this.resolved.key === cacheKey && Date.now() - this.resolved.at < 30_000) {
      return { model: this.resolved.model, models: this.resolved.models };
    }
    let models: string[];
    try {
      models = await this.listModels();
    } catch (err) {
      return { model: configured, listError: (err as Error).message };
    }
    const picked = pickInstalledModel(configured, models);
    if (picked.repaired) this.store.setSetting('ai.model', picked.model);
    this.resolved = { key: `${provider.id}|${this.baseUrl()}|${picked.model}`, model: picked.model, models, at: Date.now() };
    return { model: picked.model, repairedFrom: picked.repaired ? configured : undefined, models };
  }

  async *stream(opts: { messages: ChatMessage[]; tools?: ToolSchema[]; temperature?: number; maxTokens?: number; signal?: AbortSignal }): AsyncGenerator<StreamEvent> {
    const { model } = await this.effectiveModel();
    const body = {
      model,
      messages: opts.messages.map((m) => ({ ...m, content: m.content === null ? null : redact(m.content).slice(0, 30_000) })),
      stream: true,
      temperature: opts.temperature ?? Number(this.store.getSetting('ai.temperature') ?? 0.2),
      max_tokens: opts.maxTokens ?? Number(this.store.getSetting('ai.maxTokens') ?? 2048),
      ...(opts.tools && opts.tools.length ? { tools: opts.tools, tool_choice: 'auto' } : {}),
    };
    const res = await this.request(body, opts.signal);
    if (!res.body) throw new AiError('The AI provider returned an empty stream.');
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    const toolAcc = new Map<number, { id: string; type: 'function'; function: { name: string; arguments: string } }>();
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        const parts = buffer.split('\n');
        buffer = parts.pop() ?? '';
        for (const raw of parts) {
          const line = raw.trim();
          if (!line.startsWith('data:')) continue;
          const data = line.slice(5).trim();
          if (data === '[DONE]') continue;
          let json: {
            choices?: Array<{
              delta?: { content?: string; reasoning_content?: string; reasoning?: string; tool_calls?: Array<{ index: number; id?: string; function?: { name?: string; arguments?: string } }> };
              finish_reason?: string;
            }>;
            usage?: StreamEvent['usage'];
          };
          try {
            json = JSON.parse(data);
          } catch {
            continue;
          }
          const choice = json.choices?.[0];
          if (choice?.delta?.content) yield { delta: choice.delta.content };
          const reasoning = choice?.delta?.reasoning_content ?? choice?.delta?.reasoning;
          if (reasoning) yield { reasoning };
          if (choice?.delta?.tool_calls) {
            for (const tc of choice.delta.tool_calls) {
              const cur = toolAcc.get(tc.index) ?? { id: tc.id ?? `call_${tc.index}`, type: 'function' as const, function: { name: '', arguments: '' } };
              if (tc.id) cur.id = tc.id;
              if (tc.function?.name) cur.function.name += tc.function.name;
              if (tc.function?.arguments) cur.function.arguments += tc.function.arguments;
              toolAcc.set(tc.index, cur);
            }
          }
          if (choice?.finish_reason) yield { finishReason: choice.finish_reason, toolCalls: [...toolAcc.values()] };
          if (json.usage) yield { usage: json.usage };
        }
      }
      if (toolAcc.size > 0) yield { toolCalls: [...toolAcc.values()] };
    } finally {
      reader.releaseLock();
    }
  }

  async testConnection(): Promise<ConnectionTest> {
    const provider = this.provider();
    const started = Date.now();
    if (provider.requiresKey && !this.hasKey()) {
      return {
        model: this.model(),
        provider: provider.id,
        ok: false,
        latencyMs: 0,
        message: `${provider.label} requires an API key. Add one in Settings, or use the free local Ollama provider.`,
      };
    }
    const effective = await this.effectiveModel();
    const model = effective.model;
    const base = { model, provider: provider.id };
    if (provider.local && effective.listError) {
      return { ...base, ok: false, latencyMs: Date.now() - started, message: effective.listError };
    }
    let models = effective.models;
    if (!models) {
      try {
        models = await this.listModels();
      } catch (err) {
        // A provider may not implement /models; the chat call below is the real test.
        if ((err as AiError).status === 401 || (err as AiError).status === 403) {
          return { ...base, ok: false, latencyMs: Date.now() - started, message: (err as Error).message };
        }
      }
    }
    if (provider.local && models && models.length === 0) {
      return {
        ...base,
        ok: false,
        latencyMs: Date.now() - started,
        message: `${provider.label} is running but has no models installed. ${provider.setup}`,
        models,
      };
    }
    try {
      const res = await this.request(
        {
          model,
          messages: [
            { role: 'system', content: 'You are a connectivity probe. Reply with exactly OK.' },
            { role: 'user', content: 'Reply with OK.' },
          ],
          max_tokens: 16,
          temperature: 0,
          stream: false,
        },
        undefined,
      );
      const json = (await res.json()) as { choices?: Array<{ message?: { content?: string } }> };
      const reply = (json.choices?.[0]?.message?.content ?? '').trim();
      const repairNote = effective.repairedFrom
        ? `Selected installed model "${model}" because "${effective.repairedFrom}" is not installed. `
        : '';
      return {
        ...base,
        ok: true,
        latencyMs: Date.now() - started,
        message: `${repairNote}Connected to ${provider.label}. Model replied: ${reply.slice(0, 60) || '(empty)'}`,
        models,
      };
    } catch (err) {
      return { ...base, ok: false, latencyMs: Date.now() - started, message: (err as Error).message, models };
    }
  }
}
