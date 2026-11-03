import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { get } from 'node:http';
import { createServer } from 'node:net';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = process.env.JEDEN_BIN || 'jeden';
const runs = join(root, 'build/real-tests/stats');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const report = {
  started_at: new Date().toISOString(), binary, commands: [], requests: [], cases: [], verdict: 'failed',
  scope: 'Real native snapshot, loopback HTTP routes, an operating-system-assigned port, and occupied-port and malformed-port refusals. Does not qualify graphical refresh, retained errors, or account-specific quota outcomes. Reads existing usage and quota without changing them.',
};
let reservation;
let server;

function run(program, args, cwd = root) {
  const result = spawnSync(program, args, { cwd, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 });
  report.commands.push({
    program, args, cwd, exit_status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message,
  });
  return result;
}

function success(result) {
  assert.equal(result.status, 0, String(result.stderr || result.error || result.signal));
  return result.stdout.trim();
}

function blocked(code, message) {
  report.verdict = 'blocked';
  throw Object.assign(new Error(message), { code });
}

function launch(port) {
  const args = port === undefined ? ['stats', '--serve'] : ['stats', '--serve', '--port', String(port)];
  const observation = { program: binary, args, stdout: '', stderr: '', exit_status: null, signal: null };
  report.commands.push(observation);
  const child = spawn(binary, args, { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  const closed = new Promise(resolve => {
    child.once('error', error => { observation.error = error.message; });
    child.once('close', (code, signal) => {
      observation.exit_status = code;
      observation.signal = signal;
      resolve();
    });
  });
  const ready = new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('close', () => reject(new Error(`Statistics server exited before readiness: ${observation.stderr}`)));
    child.stdout.on('data', chunk => {
      observation.stdout += chunk;
      const bound = observation.stdout.match(/jeden stats dashboard: http:\/\/127\.0\.0\.1:(\d+) /);
      if (bound && (port === undefined || Number(bound[1]) === port)) resolve(Number(bound[1]));
    });
    child.stderr.on('data', chunk => { observation.stderr += chunk; });
  });
  return { child, closed, ready };
}

function request(url) {
  const observation = { method: 'GET', url, body: '' };
  report.requests.push(observation);
  return new Promise((resolve, reject) => {
    const fail = error => { observation.error = error.message; reject(error); };
    const operation = get(url, { agent: false }, response => {
      observation.status = response.statusCode;
      observation.headers = response.headers;
      response.setEncoding('utf8');
      response.on('data', chunk => { observation.body += chunk; });
      response.once('error', fail);
      response.once('end', () => resolve(observation));
    });
    operation.once('error', fail);
  });
}

async function releasePort() {
  if (!reservation) return;
  const owned = reservation;
  reservation = undefined;
  if (owned.listening) await new Promise((resolve, reject) => owned.close(error => error ? reject(error) : resolve()));
}

try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = createHash('sha256').update(readFileSync(fileURLToPath(import.meta.url))).digest('hex');
  report.binary_version = success(run(binary, ['--version']));
  const revision = report.binary_version.match(/[.+]([0-9a-f]{7,40})$/);
  if (!revision) blocked('JEDEN_REVISION_NOT_REPORTED', report.binary_version);
  const resolved = run('git', ['rev-parse', '--verify', `${revision[1]}^{commit}`]);
  if (resolved.status !== 0) blocked('JEDEN_REVISION_NOT_RESOLVED', resolved.stderr);
  report.binary_revision = resolved.stdout.trim();
  if (report.binary_revision !== report.source_revision) {
    blocked('JEDEN_REVISION_NOT_INSTALLED', `Installed ${report.binary_version} identifies ${report.binary_revision}, not source ${report.source_revision}`);
  }

  const snapshot = JSON.parse(success(run(binary, ['stats', '--json'])));
  assert.equal(snapshot.cwd, root, 'CLI read a different project');
  report.cases.push({ name: 'native project snapshot', snapshot });
  const sessionRoot = join(homedir(), '.jeden/sessions');
  const expectedSessions = readdirSync(sessionRoot)
    .filter(name => statSync(join(sessionRoot, name)).isDirectory()).sort();
  assert.deepEqual([...snapshot.sessions.recent].sort(), expectedSessions,
    'native snapshot must include every persisted session directory, not only a newest subset');
  assert.equal(snapshot.sessions.count, expectedSessions.length);
  report.cases.push({ name: 'complete persisted session inventory', sessionRoot, expectedSessions });

  // A ledger holding an event of another shape is reported with its path
  // and the decoder's reason; it is never counted as zero usage.
  const brokenProject = join(output, 'broken-ledger');
  mkdirSync(join(brokenProject, '.jeden'), { recursive: true });
  const brokenLedger = join(brokenProject, '.jeden/usage.json');
  writeFileSync(brokenLedger, JSON.stringify({ events: [{ model: 'recorded-model' }] }) + '\n');
  const brokenSnapshot = JSON.parse(success(run(binary, ['stats', '--json'], brokenProject)));
  assert.ok(brokenSnapshot.usage.project.error?.includes(`${brokenLedger} is not a usage ledger`),
    JSON.stringify(brokenSnapshot.usage.project));
  assert.ok(brokenSnapshot.usage.project.error.includes('totalTokens'), brokenSnapshot.usage.project.error);
  assert.equal(brokenSnapshot.usage.project.events, undefined, 'an unreadable ledger must not report an event count');
  const brokenSummary = run(binary, ['stats', '--summary'], brokenProject);
  assert.throws(() => success(brokenSummary), 'a summary over an unreadable ledger must fail');
  assert.ok(brokenSummary.stderr.includes('usage unreadable:'), brokenSummary.stderr);
  const brokenText = success(run(binary, ['stats'], brokenProject));
  assert.ok(brokenText.includes('usage (project) unreadable:'), brokenText);
  report.cases.push({ name: 'unreadable usage ledger reported', ledger: brokenLedger, error: brokenSnapshot.usage.project.error });

  const malformed = run(binary, ['stats', '--serve', '--port', 'not-a-port']);
  assert.equal(malformed.status, 1, `${malformed.stdout}\n${malformed.stderr}`);
  assert.ok(malformed.stderr.includes('jeden stats --serve --port takes a port number from 0 to 65535, not not-a-port'), malformed.stderr);
  report.cases.push({ name: 'malformed port refused', exit_status: malformed.status, stderr: malformed.stderr });

  const assigned = launch();
  const assignedPort = await assigned.ready;
  const assignedApi = await request(`http://127.0.0.1:${assignedPort}/api/stats`);
  assert.equal(assignedApi.status, 200);
  report.cases.push({ name: 'operating-system-assigned port served and reported', port: assignedPort });
  assigned.child.kill('SIGTERM');
  await assigned.closed;

  reservation = createServer();
  await new Promise((resolve, reject) => {
    reservation.once('error', reject);
    reservation.listen(0, '127.0.0.1', resolve);
  });
  const port = reservation.address().port;
  const refused = run(binary, ['stats', '--serve', '--port', String(port)]);
  assert.equal(refused.status, 1, `${refused.stdout}\n${refused.stderr}`);
  assert.ok(refused.stderr.includes(`cannot bind 127.0.0.1:${port}:`), refused.stderr);
  report.cases.push({ name: 'occupied loopback port refused', port, exit_status: refused.status, stderr: refused.stderr });
  await releasePort();

  server = launch(port);
  await server.ready;
  const origin = `http://127.0.0.1:${port}`;
  const page = await request(`${origin}/`);
  assert.equal(page.status, 200);
  assert.equal(page.headers['content-type'], 'text/html; charset=utf-8');
  const api = await request(`${origin}/api/stats`);
  assert.equal(api.status, 200);
  const observed = JSON.parse(api.body);
  assert.equal(observed.cwd, root, 'HTTP read a different project');
  report.cases.push({ name: 'loopback snapshot served', snapshot: observed });
  const missing = await request(`${origin}/not-a-statistics-route`);
  assert.equal(missing.status, 404);
  report.cases.push({ name: 'unknown route refused', status: missing.status, body: missing.body });
  report.verdict = 'passed';
} catch (error) {
  report.error = { code: error.code, message: error.message, stack: error.stack };
  process.exitCode = 1;
} finally {
  try {
    await releasePort();
    if (server) {
      if (server.child.pid && server.child.exitCode === null && server.child.signalCode === null) server.child.kill('SIGTERM');
      await server.closed;
    }
  } catch (error) {
    report.cleanup_error = error.message;
    report.verdict = 'failed';
    process.exitCode = 1;
  }
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
