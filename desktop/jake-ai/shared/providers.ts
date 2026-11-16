/**
 * OpenAI-compatible AI providers. Free/local options come first; Ollama is the
 * default so JAKE works without any paid API or cloud account.
 */
export type ProviderId = 'ollama' | 'lmstudio' | 'qwen' | 'openrouter' | 'groq' | 'openai' | 'custom';

export interface AiProvider {
  id: ProviderId;
  label: string;
  description: string;
  baseUrl: string;
  defaultModel: string;
  models: string[];
  requiresKey: boolean;
  local: boolean;
  setup: string;
}

export const AI_PROVIDERS: AiProvider[] = [
  {
    id: 'ollama',
    label: 'Ollama (local, free)',
    description: 'Runs Qwen, Llama and other open models entirely on this PC. No account, no API key, no data leaves the machine.',
    baseUrl: 'http://127.0.0.1:11434/v1',
    defaultModel: 'qwen2.5:7b',
    models: ['qwen2.5:7b', 'qwen2.5:3b', 'qwen2.5-coder:7b', 'qwen3:8b', 'llama3.1:8b', 'mistral:7b'],
    requiresKey: false,
    local: true,
    setup: 'Install from https://ollama.com, then run: ollama pull qwen2.5:7b',
  },
  {
    id: 'lmstudio',
    label: 'LM Studio (local, free)',
    description: 'Local OpenAI-compatible server from LM Studio. No account or key; models run on this PC.',
    baseUrl: 'http://127.0.0.1:1234/v1',
    defaultModel: 'qwen2.5-7b-instruct',
    models: ['qwen2.5-7b-instruct', 'qwen2.5-coder-7b-instruct', 'llama-3.1-8b-instruct'],
    requiresKey: false,
    local: true,
    setup: 'In LM Studio, load a model and start the local server on port 1234.',
  },
  {
    id: 'qwen',
    label: 'Qwen / DashScope (free quota)',
    description: "Alibaba's Qwen models through the OpenAI-compatible DashScope endpoint. Free trial quota, then paid.",
    baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    defaultModel: 'qwen-plus',
    models: ['qwen-plus', 'qwen-turbo', 'qwen2.5-72b-instruct', 'qwen2.5-coder-32b-instruct', 'qwen3-235b-a22b'],
    requiresKey: true,
    local: false,
    setup: 'Create an API key at https://bailian.console.alibabacloud.com and paste it here.',
  },
  {
    id: 'openrouter',
    label: 'OpenRouter (free models)',
    description: 'One key for hundreds of models, including Qwen and Llama variants tagged :free. Free models are rate-limited.',
    baseUrl: 'https://openrouter.ai/api/v1',
    defaultModel: 'qwen/qwen-2.5-72b-instruct:free',
    models: ['qwen/qwen-2.5-72b-instruct:free', 'qwen/qwen3-32b:free', 'meta-llama/llama-3.3-70b-instruct:free', 'deepseek/deepseek-chat-v3-0324:free'],
    requiresKey: true,
    local: false,
    setup: 'Create a free key at https://openrouter.ai/keys and confirm the model still has a :free variant.',
  },
  {
    id: 'groq',
    label: 'Groq (free tier)',
    description: 'Very fast inference with a free tier, including Qwen and Llama models.',
    baseUrl: 'https://api.groq.com/openai/v1',
    defaultModel: 'qwen/qwen3-32b',
    models: ['qwen/qwen3-32b', 'llama-3.3-70b-versatile', 'llama-3.1-8b-instant'],
    requiresKey: true,
    local: false,
    setup: 'Create a free key at https://console.groq.com/keys.',
  },
  {
    id: 'openai',
    label: 'OpenAI (paid, optional)',
    description: 'OpenAI models through the standard API. Requires a funded account.',
    baseUrl: 'https://api.openai.com/v1',
    defaultModel: 'gpt-4o-mini',
    models: ['gpt-4o-mini', 'gpt-4.1-mini'],
    requiresKey: true,
    local: false,
    setup: 'Create a key at https://platform.openai.com/api-keys.',
  },
  {
    id: 'custom',
    label: 'Custom OpenAI-compatible endpoint',
    description: 'Any server that speaks /v1/chat/completions (vLLM, llama.cpp, LocalAI, a gateway, or a company endpoint).',
    baseUrl: 'http://127.0.0.1:8000/v1',
    defaultModel: 'qwen2.5-7b-instruct',
    models: [],
    requiresKey: false,
    local: false,
    setup: 'Enter the base URL including /v1, and the model name your server exposes.',
  },
];

export const DEFAULT_PROVIDER_ID: ProviderId = 'ollama';

export function getProvider(id: string | null | undefined): AiProvider {
  return AI_PROVIDERS.find((p) => p.id === id) ?? AI_PROVIDERS[0];
}

export function providerRequiresKey(id: string | null | undefined): boolean {
  return getProvider(id).requiresKey;
}

export function providerModels(id: string | null | undefined): string[] {
  return getProvider(id).models;
}

/**
 * Pick a usable local model. If the configured model is installed it is kept;
 * otherwise prefer an installed Qwen model, then Llama, then the first one.
 */
export function pickInstalledModel(configured: string, installed: string[]): { model: string; repaired: boolean } {
  if (installed.length === 0 || installed.includes(configured)) return { model: configured, repaired: false };
  const qwen = installed.find((m) => /qwen/i.test(m));
  const llama = installed.find((m) => /llama/i.test(m));
  return { model: qwen ?? llama ?? installed[0], repaired: true };
}
