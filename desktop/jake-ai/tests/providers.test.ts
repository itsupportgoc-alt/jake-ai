import test from 'node:test';
import assert from 'node:assert/strict';
import { AI_PROVIDERS, DEFAULT_PROVIDER_ID, getProvider, pickInstalledModel, providerModels, providerRequiresKey } from '../shared/providers';

test('Ollama is the default free local provider and needs no key', () => {
  assert.equal(DEFAULT_PROVIDER_ID, 'ollama');
  const p = getProvider(DEFAULT_PROVIDER_ID);
  assert.equal(p.local, true);
  assert.equal(p.requiresKey, false);
  assert.match(p.baseUrl, /127\.0\.0\.1:11434\/v1$/);
  assert.ok(p.models.some((m) => m.startsWith('qwen')));
});

test('cloud providers declare a key requirement and a usable base URL', () => {
  for (const id of ['qwen', 'openrouter', 'groq', 'openai']) {
    const p = getProvider(id);
    assert.equal(p.requiresKey, true, `${id} should require a key`);
    assert.match(p.baseUrl, /^https:\/\//, `${id} should use https`);
    assert.ok(p.defaultModel.length > 0);
  }
  assert.equal(providerRequiresKey('ollama'), false);
  assert.equal(providerRequiresKey('lmstudio'), false);
});

test('unknown providers fall back to Ollama instead of crashing', () => {
  assert.equal(getProvider('does-not-exist').id, 'ollama');
  assert.equal(getProvider(null).id, 'ollama');
  assert.deepEqual(providerModels('ollama'), getProvider('ollama').models);
});

test('an installed local model is selected when the configured one is missing', () => {
  assert.deepEqual(pickInstalledModel('qwen2.5:7b', ['llama3.2:3b', 'qwen2.5:7b']), { model: 'qwen2.5:7b', repaired: false });
  assert.deepEqual(pickInstalledModel('qwen2.5:7b', ['llama3.2:3b']), { model: 'llama3.2:3b', repaired: true });
  assert.deepEqual(pickInstalledModel('missing:1b', ['llama3.2:3b', 'qwen2.5:7b']), { model: 'qwen2.5:7b', repaired: true });
  assert.deepEqual(pickInstalledModel('missing:1b', []), { model: 'missing:1b', repaired: false });
});

test('every provider has unique id and a complete label/baseUrl/setup', () => {
  const ids = new Set<string>();
  for (const p of AI_PROVIDERS) {
    assert.ok(!ids.has(p.id), `duplicate provider id ${p.id}`);
    ids.add(p.id);
    assert.ok(p.label.length > 3);
    assert.match(p.baseUrl, /^https?:\/\//);
    assert.ok(p.setup.length > 5);
  }
});
