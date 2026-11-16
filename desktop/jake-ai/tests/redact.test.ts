import test from 'node:test';
import assert from 'node:assert/strict';
import { redact, maskEmail, redactObject } from '../electron/services/redact';

test('redacts bearer tokens and API keys', () => {
  const input = 'Authorization: Bearer abcdef1234567890 and api_key=supersecretvalue';
  const out = redact(input);
  assert.ok(!out.includes('abcdef1234567890'));
  assert.ok(!out.includes('supersecretvalue'));
  assert.ok(out.includes('[REDACTED]'));
});

test('redacts cookies and session ids', () => {
  const out = redact('Cookie: session_id=abcdef0123456789; csrf_token=deadbeefdeadbeef');
  assert.ok(!out.includes('abcdef0123456789'));
  assert.ok(!out.includes('deadbeefdeadbeef'));
});

test('redacts structured password fields without dropping other data', () => {
  const out = redactObject({ username: 'alice', password: 'hunter2', nested: { password_hash: '$2y$10$abcdefghijklmnopqrstuv' } });
  assert.deepEqual(out, {
    username: 'alice',
    password: '[REDACTED]',
    nested: { password_hash: '[REDACTED]' },
  });
});

test('redacts long opaque secrets', () => {
  const secret = 'A'.repeat(64);
  assert.ok(!redact({ value: secret }).includes(secret));
});

test('masks account identifiers for reports', () => {
  assert.equal(maskEmail('alice@example.com'), 'a***@example.com');
  assert.equal(maskEmail('bob'), 'b***b');
});
