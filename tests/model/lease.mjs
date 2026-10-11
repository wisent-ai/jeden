// Real CLI and Brama journey: a session runs on an account leased from Brama's
// pool and gives the lease back when it ends.
// Required: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_LEASE_ROUTE (a
// provider/model route of a provider Brama keeps a pool of accounts for).
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
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
const route = required('JEDEN_LEASE_ROUTE');
const [provider] = route.split('/');
assert.ok(provider && route.includes('/'), 'JEDEN_LEASE_ROUTE names a provider/model route');
assert.ok(!process.env.JEDEN_MODEL_ENDPOINT, 'the journey requires the real Stado-selected Brama route');
assert.ok(!process.env.BRAMA_URL, 'an endpoint override must not replace the directory fixture');
const runs = join(root, '.build/model-lease');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), route,
  testSha256: hash(fileURLToPath(import.meta.url)), commands: [], observations: {}, verdict: 'failed' };
function invoke(program, args, cwd, env) {
  const result = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, args, cwd, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  return result;
}
function succeeds(result) {
  assert.ok(result.status !== null && !result.status, result.stderr);
  return result.stdout;
}
function sessionDirectories(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (!entry.isDirectory()) return [];
    const inner = sessionDirectories(path);
    return readdirSync(path).includes('transcript.jsonl') ? [path, ...inner] : inner;
  });
}
try {
  assert.equal(succeeds(invoke('git', ['rev-parse', 'HEAD'], root, process.env)).trim(), revision);
  assert.equal(succeeds(invoke('git', ['status', '--porcelain'], root, process.env)).trim(), '',
    'commit the candidate before qualification');
  const sessions = join(output, 'sessions');
  const workspace = join(output, 'workspace');
  mkdirSync(sessions, { recursive: true });
  mkdirSync(workspace, { recursive: true });
  const env = { ...process.env, JEDEN_SESSION_ROOT: sessions };

  // A verb the command does not have is the caller's mistake: it exits as every
  // usage refusal of this binary does (measured on `jeden relay`), naming the
  // three invocations.
  const usageStatus = invoke(binary, ['relay'], workspace, env).status;
  assert.ok(usageStatus, 'a usage refusal exits unsuccessfully');
  const refused = invoke(binary, ['lease', 'take'], workspace, env);
  assert.equal(refused.status, usageStatus, 'an unknown lease verb is a usage refusal');
  assert.match(refused.stderr, /jeden lease release <lease-id>/);
  assert.match(refused.stderr, /jeden lease end <session-id>/);

  // A lease handed in JEDEN_LEASE that is not Brama's lease document stops the
  // run with its cause; it is never read as no lease. No session directory of
  // the leased journey below is written by it.
  const handed = invoke(binary, ['run', 'Respond exactly: OK', '--cwd', workspace, '--model', route,
    '--model-only'], workspace, { ...env, JEDEN_SESSION_ROOT: join(output, 'handed-sessions'),
    JEDEN_LEASE: '{"lease": "not a lease document"}' });
  assert.ok(handed.status, 'an unreadable handed lease must refuse the run');
  assert.match(handed.stderr, /JEDEN_LEASE does not hold Brama's lease document/);

  // Brama states a sessions limit for the route's provider, so its sessions are leased.
  const before = JSON.parse(succeeds(invoke(binary, ['lease', 'list', '--json'], workspace, env)));
  report.observations.registerBefore = before;
  assert.ok(Number.isInteger(before.limits[provider]),
    `Brama must state a sessions limit for ${provider}; it answered ${JSON.stringify(before.limits)}`);

  // A real session on the route takes its lease, runs on it, and gives it back at its end.
  succeeds(invoke(binary, ['run', 'Respond exactly: OK', '--cwd', workspace, '--model', route,
    '--model-only', '--json'], workspace, env));
  const [session] = sessionDirectories(sessions);
  assert.ok(session, 'the run recorded no session');
  const exported = JSON.parse(succeeds(invoke(binary, ['export', session], workspace, env)));
  writeFileSync(join(session, 'qualification-export.json'), JSON.stringify(exported));
  const leased = exported.events.filter((event) => event.type === 'model_subscription_route');
  report.observations.leasedSteps = leased.map((event) => event.data);
  const lease = [...report.observations.leasedSteps].pop();
  assert.ok(lease, 'the session ran no step on a Brama lease');
  assert.equal(lease.providerId, provider);
  assert.ok(lease.leaseId && lease.subscriptionId, 'the recorded lease names its id and account');
  const usage = JSON.parse(readFileSync(join(workspace, '.jeden/usage.json'), 'utf8'));
  report.observations.usageBilling = usage.events.map((event) => event.billing);
  assert.ok(usage.events.some((event) => event.billing?.leaseId === lease.leaseId),
    'the usage ledger must name the lease the completion ran on');

  const after = JSON.parse(succeeds(invoke(binary, ['lease', 'list', '--json'], workspace, env)));
  report.observations.registerAfter = after;
  assert.ok(!after.leases.some((entry) => entry.id === lease.leaseId),
    'the session ended, so Brama must no longer hold its lease');

  // Ending a session that holds nothing releases nothing and says so.
  const ended = JSON.parse(succeeds(invoke(binary, ['lease', 'end', lease.sessionId, '--json'],
    workspace, env)));
  assert.deepEqual(ended.released, []);
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
