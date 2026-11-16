import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import type { Target } from '../shared/types';
import { assertUrlInScope, hostMatches, isPrivateAddress, scopedFetch, ScopeError, targetHosts } from '../electron/services/scope';

function localTarget(port: number, extra: string[] = []): Target {
  return {
    id: 'target-local',
    label: 'Local test server',
    url: `http://127.0.0.1:${port}`,
    kind: 'localhost',
    authorization_note: 'Isolated test server created by the test suite.',
    scope_patterns: JSON.stringify(extra),
    rate_limit: 50,
    allow_private: 1,
    created_at: new Date().toISOString(),
  };
}

test('host matching supports exact and wildcard allowlists', () => {
  assert.equal(hostMatches('app.example.com', 'app.example.com'), true);
  assert.equal(hostMatches('app.example.com', '*.example.com'), true);
  assert.equal(hostMatches('example.com', '*.example.com'), false);
  assert.equal(hostMatches('evil-example.com', '*.example.com'), false);
  assert.equal(hostMatches('APP.Example.com', 'app.example.com'), true);
});

test('private address classification covers RFC1918 and loopback', () => {
  for (const ip of ['127.0.0.1', '10.1.2.3', '192.168.1.10', '172.16.5.5', '169.254.1.1', '::1', 'fd00::1', 'fe80::1']) {
    assert.equal(isPrivateAddress(ip), true, `${ip} should be private`);
  }
  for (const ip of ['8.8.8.8', '1.1.1.1', '2606:4700:4700::1111']) {
    assert.equal(isPrivateAddress(ip), false, `${ip} should be public`);
  }
});

test('scope rejects hosts outside the allowlist before any connection', async () => {
  const target = localTarget(1);
  await assert.rejects(() => assertUrlInScope(target, 'https://not-registered.example/'), ScopeError);
  assert.deepEqual(targetHosts(target), ['127.0.0.1']);
});

test('scope blocks private ranges unless explicitly allowed', async () => {
  const target = { ...localTarget(1), allow_private: 0 };
  await assert.rejects(() => assertUrlInScope(target, 'http://127.0.0.1/'), /private\/loopback/);
});

test('scoped fetch follows in-scope redirects but refuses to leave scope', async () => {
  const server = http.createServer((req, res) => {
    if (req.url === '/redirect') {
      res.writeHead(302, { Location: '/final' });
      res.end();
    } else if (req.url === '/escape') {
      res.writeHead(302, { Location: 'https://example.com/' });
      res.end();
    } else if (req.url === '/final') {
      res.writeHead(200, { 'Content-Type': 'text/plain', 'X-Jake-Test': 'ok' });
      res.end('final page');
    } else {
      res.writeHead(200, { 'Content-Type': 'text/plain' });
      res.end('root');
    }
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const port = (server.address() as { port: number }).port;
  const target = localTarget(port);
  try {
    const root = await scopedFetch(target, `http://127.0.0.1:${port}/`);
    assert.equal(root.status, 200);
    assert.equal(root.body, 'root');

    const redirect = await scopedFetch(target, `http://127.0.0.1:${port}/redirect`);
    assert.equal(redirect.status, 200);
    assert.equal(redirect.body, 'final page');
    assert.equal(redirect.redirectChain.length, 1);
    assert.equal(redirect.headers['x-jake-test'], 'ok');

    await assert.rejects(() => scopedFetch(target, `http://127.0.0.1:${port}/escape`), ScopeError);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});
