// Real CLI test: `/resume` with no session lists the sessions under the
// session root; a root that exists and cannot be listed is named with its
// cause instead of being answered as "No sessions found.", which told the
// operator his sessions were gone. A missing root is still no sessions, and
// a readable one lists its session directories. JEDEN_SESSION_ROOT, HOME and
// the project are isolated under .build. Every command, its exit status and
// its output are kept in report.json.
//
// Run: JEDEN_BIN=target/debug/jeden JEDEN_TEST_SOURCE_REVISION=$(git rev-parse HEAD) \
//   node tests/sessions/unlistable_root.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = resolve(required('JEDEN_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const runs = join(root, '.build/sessions-unlistable-root');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  commands: [], verdict: 'failed' };

const home = join(output, 'home');
const project = join(output, 'project');
const sessions = join(output, 'sessions');
mkdirSync(home, { recursive: true });
mkdirSync(project, { recursive: true });

function resumeListing() {
  const env = { ...process.env, HOME: home, JEDEN_SESSION_ROOT: sessions };
  const args = ['run', '/resume', '--json', '--cwd', project];
  const result = spawnSync(binary, args, { cwd: project, env, encoding: 'utf8' });
  report.commands.push({ args, sessions, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.signal, null, '/resume must not end by signal');
  assert.ok(result.status !== null && !result.status, result.stderr);
  return JSON.parse(result.stdout).text;
}

try {
  assert.equal(resumeListing(), 'No sessions found.', 'a missing root is not no sessions');

  writeFileSync(sessions, 'not a directory');
  const refused = resumeListing();
  assert.ok(refused.startsWith(`Sessions cannot be listed: ${sessions}: `),
    `an unlistable root was not named: ${refused}`);

  rmSync(sessions);
  mkdirSync(join(sessions, 'listed-session'), { recursive: true });
  assert.ok(resumeListing().includes('listed-session'), 'a readable root did not list its session');
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
