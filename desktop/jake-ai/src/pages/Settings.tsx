import { useEffect, useState } from 'react';
import { Cpu, KeyRound, RefreshCw, Save, ShieldCheck, TestTube2, TriangleAlert, Wifi } from 'lucide-react';
import { call, useLoad, type SettingsRow } from '../lib/api';
import { Badge, Button, Card, ErrorBox, Field, Select, Spinner } from '../components/ui';
import { AI_PROVIDERS, getProvider } from '../../shared/providers';

export default function SettingsPage() {
  const settings = useLoad(() => call<SettingsRow>('settings:get'), []);
  const [form, setForm] = useState({
    aiProvider: 'ollama',
    aiModel: 'qwen2.5:7b',
    aiBaseUrl: 'http://127.0.0.1:11434/v1',
    temperature: '0.2',
    maxTokens: '2048',
    autoApproveLevel: '1',
    defaultRateLimit: '2',
    requestTimeoutMs: '15000',
    apiKey: '',
  });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<SettingsRow | null>(null);
  const [testResult, setTestResult] = useState<string | null>(null);
  const [detected, setDetected] = useState<string[]>([]);
  const [detectMsg, setDetectMsg] = useState<string | null>(null);

  useEffect(() => {
    if (!settings.data) return;
    setForm((f) => ({
      ...f,
      aiProvider: settings.data!.aiProvider,
      aiModel: settings.data!.aiModel,
      aiBaseUrl: settings.data!.aiBaseUrl,
      temperature: String(settings.data!.temperature),
      maxTokens: String(settings.data!.maxTokens),
      autoApproveLevel: String(settings.data!.autoApproveLevel),
      defaultRateLimit: String(settings.data!.defaultRateLimit),
      requestTimeoutMs: String(settings.data!.requestTimeoutMs),
    }));
    setSaved(settings.data);
  }, [settings.data]);

  const current = saved ?? settings.data;
  const preset = getProvider(form.aiProvider);
  const savedProvider = getProvider(current?.aiProvider);
  const needsKey = preset.requiresKey;
  const showKey = needsKey || preset.id === 'custom';
  const configChanged = Boolean(current) && (form.aiProvider !== current!.aiProvider || form.aiBaseUrl.replace(/\/+$/, '') !== current!.aiBaseUrl.replace(/\/+$/, ''));

  // For local providers, quietly list installed models so a missing saved
  // default is visible immediately instead of only when chatting.
  useEffect(() => {
    if (!current || !getProvider(current.aiProvider).local) return;
    let cancelled = false;
    call<string[]>('ai:models')
      .then((models) => {
        if (cancelled) return;
        setDetected(models);
        if (models.length && !models.includes(current.aiModel)) {
          setDetectMsg(`Saved model “${current.aiModel}” is not installed. Installed: ${models.join(', ')}. Select one below or pull the saved model.`);
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [current?.aiProvider, current?.aiBaseUrl, current?.aiModel]);

  const changeProvider = (id: string) => {
    const next = getProvider(id);
    setForm((f) => ({ ...f, aiProvider: next.id, aiBaseUrl: next.baseUrl, aiModel: next.defaultModel }));
    setDetected([]);
    setTestResult(null);
    setDetectMsg(null);
  };

  const save = async () => {
    setBusy(true);
    setError(null);
    setTestResult(null);
    try {
      const res = await call<SettingsRow>('settings:save', {
        aiProvider: form.aiProvider,
        aiModel: form.aiModel,
        aiBaseUrl: form.aiBaseUrl,
        temperature: Number(form.temperature),
        maxTokens: Number(form.maxTokens),
        autoApproveLevel: Number(form.autoApproveLevel),
        defaultRateLimit: Number(form.defaultRateLimit),
        requestTimeoutMs: Number(form.requestTimeoutMs),
        apiKey: form.apiKey || undefined,
      });
      setSaved(res);
      setForm((f) => ({ ...f, apiKey: '' }));
      settings.reload();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const test = async () => {
    setBusy(true);
    setTestResult(null);
    try {
      const res = await call<{ ok: boolean; message: string; latencyMs: number; model: string; provider: string; models?: string[] }>('settings:test-ai');
      setTestResult(`${res.ok ? '✓' : '✕'} ${res.message}${res.ok ? `\n\nLatency: ${res.latencyMs} ms · provider: ${getProvider(res.provider).label} · model: ${res.model}` : ''}`);
      if (res.models?.length) setDetected(res.models);
    } catch (e) {
      setTestResult(`✕ ${(e as Error).message}`);
    } finally {
      setBusy(false);
    }
  };

  const detect = async () => {
    setBusy(true);
    setDetectMsg(null);
    try {
      if (configChanged) {
        setDetectMsg('Save the provider and base URL first, then detect models.');
        return;
      }
      const models = await call<string[]>('ai:models');
      setDetected(models);
      setDetectMsg(models.length ? `Found ${models.length} model(s). Click a model to select it.` : 'The server responded but has no models installed.');
    } catch (e) {
      setDetectMsg((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  if (settings.loading) return <Spinner label="Loading settings" />;

  return (
    <div className="grid grid-cols-1 gap-4 xl:grid-cols-3">
      <div className="space-y-4 xl:col-span-2">
        <Card title="AI provider" actions={<span className="text-[10px] text-slate-500">Free local AI by default · OpenAI-compatible</span>}>
          <div className="space-y-3">
            <ErrorBox message={error} />
            <div className="flex flex-wrap items-center gap-2">
              <Badge tone={preset.local ? 'ok' : 'warn'}>
                {preset.local ? <Cpu size={11} /> : <Wifi size={11} />} {preset.local ? 'Runs on this PC' : 'Cloud provider'}
              </Badge>
              {needsKey ? (
                <Badge tone={current?.aiProvider === preset.id && current?.keyConfigured ? 'ok' : 'warn'}>
                  <KeyRound size={11} /> {current?.aiProvider === preset.id && current?.keyConfigured ? 'API key configured' : 'API key required'}
                </Badge>
              ) : (
                <Badge tone="ok">
                  <KeyRound size={11} /> {preset.id === 'custom' ? 'No API key required by default' : 'No API key needed'}
                </Badge>
              )}
              <Badge tone={current?.secureStorage ? 'ok' : 'bad'}>
                <ShieldCheck size={11} /> {current?.secureStorage ? 'OS secure storage available' : 'secure storage unavailable'}
              </Badge>
            </div>

            <Field label="Provider">
              <Select value={form.aiProvider} onChange={changeProvider}>
                {AI_PROVIDERS.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.label}
                  </option>
                ))}
              </Select>
            </Field>
            <p className="rounded border border-edge bg-[#0b1219] px-3 py-2 text-[11px] leading-relaxed text-slate-400">
              {preset.description} <span className="text-slate-500">{preset.setup}</span>
            </p>

            {showKey && (
              <Field
                label={`${preset.label} API key`}
                hint={
                  current?.aiProvider === preset.id && current?.keyConfigured
                    ? 'A key is stored encrypted for this provider. Enter a new key to replace it.'
                    : preset.id === 'custom'
                      ? 'Optional. Only needed if your endpoint requires a bearer token. Stored with Electron safeStorage (Windows DPAPI).'
                      : 'Stored with Electron safeStorage (Windows DPAPI). It never reaches the React renderer.'
                }
              >
                <input
                  className="input"
                  type="password"
                  value={form.apiKey}
                  onChange={(e) => setForm({ ...form, apiKey: e.target.value })}
                  placeholder={current?.aiProvider === preset.id && current?.keyConfigured ? '••••••••••••••••••••••••' : 'Paste your API key'}
                  autoComplete="new-password"
                />
              </Field>
            )}

            <div className="grid grid-cols-2 gap-3">
              <Field label="Model" hint="Models with tool-calling support work best (Qwen 2.5/3, Llama 3.1+, Mistral).">
                <input className="input" list="model-options" value={form.aiModel} onChange={(e) => setForm({ ...form, aiModel: e.target.value })} />
                <datalist id="model-options">
                  {[...new Set([...preset.models, ...detected])].map((m) => (
                    <option key={m} value={m} />
                  ))}
                </datalist>
              </Field>
              <Field label="Base URL" hint="Must include /v1 (OpenAI-compatible endpoint).">
                <input className="input" value={form.aiBaseUrl} onChange={(e) => setForm({ ...form, aiBaseUrl: e.target.value })} />
              </Field>
              <Field label="Temperature" hint="0 = deterministic, 2 = most random.">
                <input className="input" type="number" min={0} max={2} step={0.1} value={form.temperature} onChange={(e) => setForm({ ...form, temperature: e.target.value })} />
              </Field>
              <Field label="Max tokens">
                <input className="input" type="number" min={128} max={8192} step={128} value={form.maxTokens} onChange={(e) => setForm({ ...form, maxTokens: e.target.value })} />
              </Field>
            </div>

            <div className="flex flex-wrap gap-2">
              <Button variant="primary" busy={busy} onClick={() => void save()}>
                <Save size={13} /> Save settings
              </Button>
              <Button busy={busy} onClick={() => void detect()}>
                <RefreshCw size={13} /> Detect models
              </Button>
              <Button busy={busy} disabled={needsKey && !(current?.aiProvider === preset.id && current?.keyConfigured)} onClick={() => void test()}>
                <TestTube2 size={13} /> Test connection
              </Button>
            </div>
            {configChanged && <p className="text-[10.5px] text-amber-400">You have unsaved provider changes. Save before detecting models or testing.</p>}
            {detectMsg && <pre className="code whitespace-pre-wrap">{detectMsg}</pre>}
            {detected.length > 0 && (
              <div className="flex flex-wrap gap-1.5">
                {detected.map((m) => (
                  <button key={m} className={`chip ${form.aiModel === m ? 'border-cyan-700 text-cyan-300' : 'hover:border-slate-500'}`} onClick={() => setForm({ ...form, aiModel: m })}>
                    {m}
                  </button>
                ))}
              </div>
            )}
            {testResult && <pre className="code whitespace-pre-wrap">{testResult}</pre>}
            <p className="text-[10.5px] text-slate-500">
              Connected to <span className="text-slate-400">{savedProvider.label}</span> · model <span className="text-slate-400">{current?.aiModel}</span>. Test and Detect use the saved configuration.
            </p>
          </div>
        </Card>

        <Card title="Assessment behaviour">
          <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
            <Field label="Auto-approve up to level" hint="Level 1 = every active test asks first. Higher levels reduce prompts.">
              <Select value={form.autoApproveLevel} onChange={(v) => setForm({ ...form, autoApproveLevel: v })}>
                <option value="1">1 — passive only (recommended)</option>
                <option value="2">2 — safe active scans run without asking</option>
                <option value="3">3 — authenticated tests also auto-run</option>
                <option value="4">4 — administrative changes auto-run (not recommended)</option>
              </Select>
            </Field>
            <Field label="Default request rate limit (req/s)" hint="Applied to newly registered targets.">
              <input className="input" type="number" min={0.2} max={20} step={0.2} value={form.defaultRateLimit} onChange={(e) => setForm({ ...form, defaultRateLimit: e.target.value })} />
            </Field>
            <Field label="Request timeout (ms)">
              <input className="input" type="number" min={2000} max={120000} step={1000} value={form.requestTimeoutMs} onChange={(e) => setForm({ ...form, requestTimeoutMs: e.target.value })} />
            </Field>
          </div>
          <p className="mt-2 text-[10.5px] text-slate-500">
            Level 5 (destructive operations) is not implemented and cannot be enabled. Emergency Stop is always available from the header and Ctrl+Shift+X.
          </p>
        </Card>
      </div>

      <div className="space-y-4">
        <Card title="Data sharing and privacy">
          {preset.local ? (
            <div className="flex items-start gap-2 rounded border border-emerald-900/50 bg-emerald-950/20 px-3 py-2 text-[11px] leading-relaxed text-emerald-200/80">
              <Cpu size={14} className="mt-0.5 shrink-0" />
              <div>
                <p className="font-medium">Local AI — nothing leaves this PC.</p>
                <p className="mt-1">Prompts and tool summaries are processed by {preset.label} on this computer. No cloud account is used.</p>
              </div>
            </div>
          ) : (
            <div className="flex items-start gap-2 rounded border border-amber-900/50 bg-amber-950/20 px-3 py-2 text-[11px] leading-relaxed text-amber-200/80">
              <TriangleAlert size={14} className="mt-0.5 shrink-0" />
              <div>
                <p className="font-medium">{preset.label} is an external cloud AI service.</p>
                <p className="mt-1">Chat messages and tool summaries are sent to this provider for reasoning. Do not send confidential production data, credentials or personal data without appropriate authorization and a lawful basis.</p>
              </div>
            </div>
          )}
          <ul className="mt-2 list-disc space-y-1 pl-4 text-[10.5px] leading-relaxed text-slate-400">
            <li>API keys stay in the main process and OS secure storage, per provider.</li>
            <li>Passwords, cookies, tokens and hashes are redacted before logging or AI transmission.</li>
            <li>Account audits send only aggregates and masked identifiers to the AI.</li>
            <li>Connection credentials are never included in prompts or reports.</li>
          </ul>
        </Card>

        <Card title="Free provider quick start">
          <div className="space-y-2 text-[11px] leading-relaxed text-slate-400">
            <p><span className="text-slate-200">Ollama (recommended, fully free):</span></p>
            <pre className="code whitespace-pre-wrap text-[10px]">{`ollama pull qwen2.5:7b
# or a smaller model:
ollama pull llama3.2:3b`}</pre>
            <p className="text-slate-500">Ollama usually runs at http://127.0.0.1:11434. Use “Detect models” to list what is installed.</p>
            <p><span className="text-slate-200">Qwen:</span> free trial quota at DashScope, OpenAI-compatible endpoint.</p>
            <p><span className="text-slate-200">OpenRouter:</span> use models ending in <code className="text-cyan-300">:free</code>; availability changes, check their model list.</p>
            <p><span className="text-slate-200">Groq:</span> free tier for Qwen/Llama, very fast.</p>
            <p className="text-slate-600">No provider is required at build time; JAKE starts with Ollama and works offline once a model is pulled.</p>
          </div>
        </Card>

        <Card title="Local data">
          <div className="space-y-1.5 text-[11px]">
            <div className="flex justify-between gap-2"><span className="text-slate-500">Reports directory</span><span className="break-all text-right font-mono text-[10px] text-slate-300">{current?.reportsDir}</span></div>
            <div className="flex justify-between"><span className="text-slate-500">Database</span><span className="font-mono text-[10px] text-slate-300">jake-ai.sqlite (userData)</span></div>
            <div className="flex justify-between"><span className="text-slate-500">Storage</span><span className="text-slate-300">SQLite with foreign keys and transactions</span></div>
          </div>
        </Card>

        <Card title="Security model">
          <ul className="list-disc space-y-1 pl-4 text-[10.5px] leading-relaxed text-slate-400">
            <li>Renderer: context isolation on, Node integration off, sandboxed, strict CSP.</li>
            <li>IPC: explicit allowlist, sender validation, typed inputs.</li>
            <li>Tools: fixed argument builders, scope checks, timeouts, audit logging.</li>
            <li>AI: no OS access; every action passes through the tool registry and approval manager.</li>
          </ul>
        </Card>
      </div>
    </div>
  );
}
