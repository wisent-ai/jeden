// Real test that `jeden contracts status --omp` names the Omp sessions still
// answering under the contracts the file held before the last install: Omp
// reads APPEND_SYSTEM.md only when a session starts. Runs the built jeden in an
// isolated HOME with Omp's real layout (~/.omp/agent/sessions/<dir>/*.jsonl):
// a transcript created before the install and written after it is listed and
// makes status fail; one created after the install is not; a `--file` target
// has no sessions to check. Every command, its exit status and output go to
// the run's report.json.
//
// Usage: JEDEN_BIN=<built jeden> node tests/cli/contracts_sessions.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { appendFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const binary = process.env.JEDEN_BIN;
if (!binary) {
  throw new Error('JEDEN_BIN must name the built jeden this test runs; the test proves that binary, not whichever is on PATH');
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const runs = join(root, 'build/real-tests/contracts-sessions');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const home = join(output, 'home');
const sessions = join(home, '.omp/agent/sessions/-');
mkdirSync(sessions, { recursive: true });
const older = join(sessions, 'older-session.jsonl');
const newer = join(sessions, 'newer-session.jsonl');
const other = join(output, 'other.md');
const report = {
  started_at: new Date().toISOString(), binary, commands: [], verdict: 'failed',
  scope: 'Real jeden contracts install and status against an isolated Omp layout and file times; does not start Omp itself.',
};

function run(args) {
  const result = spawnSync(binary, args, { cwd: root, encoding: 'utf8', env: { PATH: process.env.PATH, HOME: home } });
  report.commands.push({ args, exit_status: result.status, signal: result.signal, stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  return result;
}
function success(args) {
  const result = run(args);
  assert.ok(!result.status && !result.error, `jeden ${args.join(' ')} failed: ${result.stderr || result.error}`);
  return result.stdout;
}
function failure(args) {
  const result = run(args);
  assert.ok(result.status && !result.error, `jeden ${args.join(' ')} should have failed: ${result.stdout}`);
  return result.stderr;
}
const listed = (args) => JSON.parse(success(args)).sessions_on_older_text;

try {
  report.revision = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim();
  report.test_sha256 = createHash('sha256').update(readFileSync(fileURLToPath(import.meta.url))).digest('hex');

  // A session that started before the contracts were installed.
  writeFileSync(older, '{"type":"session"}\n');
  success(['contracts', 'install', '--omp']);
  assert.deepEqual(listed(['contracts', 'status', '--omp', '--json']), []);

  // It answers again after the install, under the text it started with; a
  // session started after the install reads the new text.
  appendFileSync(older, '{"type":"message"}\n');
  writeFileSync(newer, '{"type":"session"}\n');
  const refused = failure(['contracts', 'status', '--omp']);
  assert.match(refused, /Omp session\(s\) started before it was written/);
  assert.ok(refused.includes(older), refused);
  assert.ok(!refused.includes(newer), refused);
  assert.deepEqual(listed(['contracts', 'status', '--omp', '--json']), [older]);

  // Another target has no sessions behind it.
  success(['contracts', 'install', '--file', other]);
  assert.deepEqual(listed(['contracts', 'status', '--file', other, '--json']), []);

  report.verdict = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
} finally {
  report.finished_at = new Date().toISOString();
  rmSync(home, { recursive: true, force: true });
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, '  ')}\n`);
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
if (report.verdict !== 'passed') {
  throw new Error(`contracts sessions journey failed: ${report.error}`);
}
