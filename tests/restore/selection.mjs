import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, openSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real `jeden restore` against an isolated OMP session root: selection by
// operator message time, the running check through a real process holding a
// session's owner lock, unreadable transcripts, and the usage refusals.
// Opening Terminal windows is not exercised here: it would start real agent
// sessions on the operator's desktop.

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = process.env.JEDEN_BIN || 'jeden';
const runs = join(root, 'build/real-tests/restore');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const sessions = join(output, 'sessions');
const workspace = join(sessions, '-workspace');
mkdirSync(workspace, { recursive: true });
const report = {
  started_at: new Date().toISOString(),
  binary,
  revision: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim(),
  commands: [],
  verdict: 'failed',
  scope: 'Selection, running check and refusals of jeden restore on an isolated session root, with --dry-run. Does not open Terminal windows.',
};
let holder;

function run(args) {
  const result = spawnSync(binary, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ args, status: result.status, stdout: result.stdout, stderr: result.stderr });
  return result;
}

function transcript(name, id, records) {
  const header = { type: 'session', id, cwd: output, title: name };
  const path = join(workspace, `${name}.jsonl`);
  writeFileSync(path, [header, ...records].map((record) => JSON.stringify(record)).join('\n') + '\n');
  return path;
}

function operator(at) {
  return { type: 'message', timestamp: at, message: { role: 'user', attribution: 'user', content: 'work' } };
}

const since = '2026-10-02T07:00:00Z';
try {
  transcript('running', 'session-running', [operator('2026-10-02T16:00:00.000Z')]);
  transcript('stopped', 'session-stopped', [operator('2026-10-02T17:00:00.000Z')]);
  transcript('older', 'session-older', [operator('2026-10-01T12:00:00.000Z')]);
  transcript('injected', 'session-injected', [
    { type: 'message', timestamp: '2026-10-02T18:00:00.000Z', message: { role: 'user', attribution: 'agent', content: 'x' } },
  ]);
  const broken = join(workspace, 'broken.jsonl');
  writeFileSync(broken, JSON.stringify({ type: 'session', id: 'session-broken', cwd: output }) + '\nnot json\n');

  // A real process holding the running session's owner lock open.
  const lock = join(workspace, '.running.jsonl.owner.lock');
  writeFileSync(lock, '');
  const descriptor = openSync(lock, 'r');
  holder = spawn(process.execPath, ['-e', 'process.stdin.resume()'], { stdio: ['pipe', 'ignore', 'ignore', descriptor] });

  const dry = run(['restore', '--since', since, '--dry-run', '--sessions', sessions, '--json']);
  assert.equal(dry.status, 1, 'an unreadable transcript makes the run incomplete');
  const answer = JSON.parse(dry.stdout);
  const states = Object.fromEntries(answer.sessions.map((row) => [row.sessionId, row.state]));
  assert.deepEqual(states, { 'session-running': 'already_running', 'session-stopped': 'would_reopen' });
  assert.ok(answer.sessions.find((row) => row.sessionId === 'session-running').pids.includes(holder.pid));
  assert.equal(answer.unreadable.length, 1);
  assert.match(answer.unreadable[0].error, /^line 2 is not a JSON record/);
  assert.equal(answer.counts.selected, 2);
  assert.equal(answer.sinceUtc, '2026-10-02T07:00:00.000Z');

  const missing = run(['restore', '--dry-run', '--sessions', sessions]);
  assert.equal(missing.status, 2);
  assert.match(missing.stderr, /restore requires --since/);

  const malformed = run(['restore', '--since', 'yesterday', '--sessions', sessions]);
  assert.equal(malformed.status, 2);
  assert.match(malformed.stderr, /--since takes today/);

  const absent = run(['restore', '--since', since, '--dry-run', '--sessions', join(output, 'absent')]);
  assert.equal(absent.status, 1);
  assert.match(absent.stderr, /cannot read the OMP session root/);

  const headerless = join(output, 'headerless.jsonl');
  writeFileSync(headerless, JSON.stringify(operator('2026-10-02T17:00:00.000Z')) + '\n');
  const open = run(['restore', 'open', headerless]);
  assert.equal(open.status, 1);
  assert.match(open.stderr, /not an OMP transcript \(no session header\)/);

  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  process.exitCode = 1;
} finally {
  holder?.kill();
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, 2));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
