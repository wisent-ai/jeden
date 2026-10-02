import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { get } from 'node:http';
import { createServer } from 'node:net';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = process.env.JEDEN_BIN || 'jeden';
const runs = join(root, 'build/real-tests/stats');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const report = {
  started_at: new Date().toISOString(), binary, commands: [], requests: [], cases: [], verdict: 'failed',
  scope: 'Real native snapshot, loopback HTTP routes and occupied-port refusal. Does not qualify graphical refresh, retained errors, or account-specific quota outcomes. Reads existing usage and quota without changing them.',
};
let reservation;
let server;

function run(program, args) {
  const result = spawnSync(program, args, { cwd: root, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 });
  report.commands.push({
    program, args, exit_status: result.status, signal: result.signal,
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
  const args = ['stats', '--serve', '--port', String(port)];
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
      if (observation.stdout.includes(`jeden stats dashboard: http://127.0.0.1:${port} `)) resolve();
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
