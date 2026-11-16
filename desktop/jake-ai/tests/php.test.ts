import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { auditPhpDirectory } from '../electron/services/php';

test('PHP auditor detects injection, weak hashing, XSS and session issues', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'jake-php-'));
  fs.writeFileSync(
    path.join(dir, 'login.php'),
    `<?php
session_start();
$id = $_GET['id'];
$q = "SELECT * FROM users WHERE id = $id";
mysqli_query($conn, $q);
$hash = md5($password);
echo $_GET['name'];
$stmt = $pdo->prepare('SELECT * FROM users WHERE id = ?');
$stmt->execute([$id]);
`,
  );
  fs.writeFileSync(
    path.join(dir, 'safe.php'),
    `<?php
$stmt = $pdo->prepare('SELECT * FROM users WHERE email = ?');
$stmt->execute([$_POST['email']]);
echo htmlspecialchars($_POST['name'], ENT_QUOTES, 'UTF-8');
`,
  );
  fs.writeFileSync(path.join(dir, '.env'), 'DB_PASSWORD=not-a-real-secret\n');

  try {
    const result = auditPhpDirectory(dir);
    const rules = new Set(result.issues.map((i) => i.ruleId));
    assert.ok(result.filesScanned >= 2);
    assert.ok(rules.has('sql-injection'), 'expected SQL injection finding');
    assert.ok(rules.has('weak-password-hash'), 'expected weak hashing finding');
    assert.ok(rules.has('unencoded-output'), 'expected XSS finding');
    assert.ok(rules.has('session-hardening'), 'expected session hardening finding');
    assert.ok(rules.has('sensitive-file-in-project'), 'expected sensitive file finding');

    const xss = result.issues.find((i) => i.ruleId === 'unencoded-output');
    assert.ok(xss?.proposedFix?.includes('htmlspecialchars'), 'expected an output-encoding patch proposal');

    // The prepared statement must not be reported as SQL injection.
    const safe = result.issues.filter((i) => i.ruleId === 'sql-injection' && i.file === 'safe.php');
    assert.equal(safe.length, 0);
    assert.ok(result.issues.every((i) => !i.file.includes('..')));
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('PHP auditor rejects a missing directory', () => {
  assert.throws(() => auditPhpDirectory(path.join(os.tmpdir(), 'jake-does-not-exist-xyz')), /Directory not found/);
});
