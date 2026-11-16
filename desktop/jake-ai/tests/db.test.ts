import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { Database } from '../electron/services/db';

test('SQLite schema, foreign keys and cascade deletes work', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'jake-db-'));
  const file = path.join(dir, 'test.sqlite');
  const db = await Database.open(file);
  try {
    const now = new Date().toISOString();
    db.run('INSERT INTO targets (id, label, url, kind, authorization_note, scope_patterns, rate_limit, allow_private, created_at) VALUES (?,?,?,?,?,?,?,?,?)', [
      't1',
      'Test',
      'https://example.com',
      'web',
      'Test authorization note',
      '[]',
      2,
      0,
      now,
    ]);
    db.run('INSERT INTO assessments (id, target_id, target_label, type, status, progress, started_at) VALUES (?,?,?,?,?,?,?)', ['a1', 't1', 'Test', 'vulnerability', 'running', 10, now]);
    db.run(
      `INSERT INTO findings (id, assessment_id, target, name, category, severity, confidence, endpoint, evidence, description, impact, reproduction, remediation, "references", status, created_at)
       VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)`,
      ['f1', 'a1', 'https://example.com', 'Test finding', 'xss', 'high', 'high', 'https://example.com', 'e', 'd', 'i', 'r', 'fix', '[]', 'open', now],
    );
    assert.equal(db.all('SELECT * FROM findings').length, 1);

    db.run('DELETE FROM assessments WHERE id = ?', ['a1']);
    assert.equal(db.all('SELECT * FROM findings').length, 0, 'findings should cascade with their assessment');

    // transaction rollback leaves no partial data
    assert.throws(() =>
      db.tx(() => {
        db.run('INSERT INTO targets (id, label, url, kind, authorization_note, scope_patterns, rate_limit, allow_private, created_at) VALUES (?,?,?,?,?,?,?,?,?)', [
          't2',
          'Rollback',
          'https://rollback.example',
          'web',
          'note',
          '[]',
          2,
          0,
          now,
        ]);
        throw new Error('boom');
      }),
    );
    assert.equal(db.all('SELECT * FROM targets WHERE id = ?', ['t2']).length, 0);

    // persisted file is a real SQLite database
    db.close();
    assert.ok(fs.statSync(file).size > 0);
    const reopened = await Database.open(file);
    try {
      assert.equal(reopened.all('SELECT * FROM targets').length, 1);
    } finally {
      reopened.close();
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
