// Real journey: a Jeden process holds the transcript of the session it
// records open for writing, for exactly as long as it records it — what a
// fleet supervisor (Oko) reads from the kernel to tell which session a
// running Jeden is. A real `jeden rpc` in an isolated session root opens a
// session; the kernel's open files of that process (lsof) must name the
// session's transcript.jsonl opened for writing, and once the session is
// disposed the process must hold it no longer. No model is called.
// Required: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startRpc } from '../../hooks/rpc.mjs';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const binary = resolve(required('JEDEN_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const runs = join(root, '.build/held-transcript');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const workspace = join(output, 'workspace');
const sessions = join(output, 'sessions');
mkdirSync(workspace, { recursive: true });
mkdirSync(sessions, { recursive: true });
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  commands: [], frames: [], observations: {}, verdict: 'failed' };

// The files `pid` holds open for writing, by their real paths, as the
// kernel names them through lsof (`a` is the access mode, `n` the name).
function writing(pid) {
  const args = ['-a', '-p', String(pid), '-Fan'];
  const result = spawnSync('/usr/sbin/lsof', args, { encoding: 'utf8' });
  report.commands.push({ program: '/usr/sbin/lsof', args, status: result.status, stdout: result.stdout,
    stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  const held = [];
  let mode;
  for (const line of result.stdout.split('\n')) {
    if (line.startsWith('a')) mode = line.slice('a'.length);
    if (line.startsWith('n') && (mode === 'w' || mode === 'u')) held.push(line.slice('n'.length));
  }
  return held;
}

const env = { ...process.env, JEDEN_SESSION_ROOT: sessions };
const client = startRpc({ binary, cwd: workspace, env, report });
try {
  assert.equal(spawnSync('git', ['-C', root, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).stdout.trim(), revision);
  const { pid } = client.command;
  assert.ok(pid, 'jeden rpc started with a process id');
  const session = await client.rpc('session/new', { options: { cwd: workspace } });
  report.observations.session = session;
  const transcript = realpathSync(join(session.sessionPath, 'transcript.jsonl'));
  const recording = writing(pid).filter((name) => name === transcript);
  report.observations.whileRecording = recording;
  assert.ok(recording.length, `jeden rpc (pid ${pid}) does not hold ${transcript} open for writing`);

  await client.rpc('session/dispose', { sessionId: session.sessionId });
  const disposed = writing(pid).filter((name) => name === transcript);
  report.observations.afterDispose = disposed;
  assert.ok(!disposed.length, `jeden rpc still holds ${transcript} after the session was disposed`);
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  await client.end();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
