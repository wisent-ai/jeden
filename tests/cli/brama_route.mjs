// Real test that Jeden takes Brama's address from Stado's service directory
// at every start, not from a port written into an environment file: the
// adapter port the resolver binds for the consumer `jeden` is handed out by
// the host and changes, so a written copy goes stale.
//
// Runs the built jeden against this host's real Stado:
//   1. an environment file holding a stale address is overridden by Stado's
//      route, and doctor names both the route and the stale copy;
//   2. a BRAMA_URL the caller exported is used as given;
//   3. without a Stado CLI, the file's address is used and Stado's refusal
//      is named;
//   4. without Stado and without any address, `jeden run` refuses before any
//      model call and says what to declare.
// Every command, its exit status and output go to the run's report.json.
//
// Usage: JEDEN_BIN=<built jeden> node tests/cli/brama_route.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const binary = process.env.JEDEN_BIN;
if (!binary) {
  throw new Error('JEDEN_BIN must name the built jeden this test runs; the test proves that binary, not whichever is on PATH');
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const runs = join(root, 'build/real-tests/brama-route');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const workspace = join(output, 'workspace');
const bareHome = join(output, 'home');
mkdirSync(workspace, { recursive: true });
mkdirSync(bareHome, { recursive: true });
// Addresses under the reserved example domain: the copy a file kept from an
// earlier day, and one a caller exports on purpose.
const staleCopy = 'http://stale-copy.example';
const exported = 'http://exported.example';
const report = {
  started_at: new Date().toISOString(), binary, commands: [], verdict: 'failed',
  scope: "Real jeden doctor and run against this host's Stado service directory and an isolated workspace; whether Brama answers is recorded, not asserted.",
};

function record(program, args, env, cwd) {
  const result = spawnSync(program, args, { cwd, encoding: 'utf8', env });
  report.commands.push({ program, args, cwd, env_keys: Object.keys(env), exit_status: result.status, signal: result.signal, stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  return result;
}
// The host's own environment minus any Brama address it carries, so each
// case states its address itself.
function hostEnv(extra = {}) {
  const env = { ...process.env, ...extra };
  if (!('BRAMA_URL' in extra)) delete env.BRAMA_URL;
  delete env.JEDEN_MODEL_ENDPOINT;
  return env;
}
function bramaProbe(env) {
  const result = record(binary, ['doctor', '--json'], env, workspace);
  assert.ok(!result.error, `jeden doctor did not run: ${result.error}`);
  const doctor = JSON.parse(result.stdout);
  const probe = doctor.probes.find((entry) => entry.subsystem === 'brama');
  assert.ok(probe, `jeden doctor reported no brama probe: ${result.stdout}`);
  return JSON.stringify(probe);
}

try {
  report.revision = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim();
  report.test_sha256 = createHash('sha256').update(readFileSync(fileURLToPath(import.meta.url))).digest('hex');
  report.binary_sha256 = createHash('sha256').update(readFileSync(binary)).digest('hex');

  const connect = record('stado', ['service', 'directory', 'connect', 'brama', '--consumer', 'jeden', '--no-verify', '--json'], hostEnv(), workspace);
  assert.ok(!connect.status && !connect.error, `this host's Stado does not route brama for the consumer jeden: ${connect.stderr || connect.error}`);
  const routed = JSON.parse(connect.stdout).url;
  assert.ok(routed, `stado answered without a url: ${connect.stdout}`);

  // 1. Stado's route replaces the address an environment file holds.
  writeFileSync(join(workspace, '.env'), `BRAMA_URL=${staleCopy}\n`);
  const fromStado = bramaProbe(hostEnv());
  assert.ok(fromStado.includes("address from Stado's service directory (consumer jeden)"), fromStado);
  assert.ok(fromStado.includes(`still holds ${staleCopy}`), fromStado);
  assert.ok(!fromStado.includes(`${staleCopy}/v1`), `doctor called the stale copy: ${fromStado}`);

  // 2. An exported address is the caller's choice and is used as given.
  const fromCaller = bramaProbe(hostEnv({ BRAMA_URL: exported }));
  assert.ok(fromCaller.includes('address from BRAMA_URL in the process environment'), fromCaller);

  // 3. Without a Stado CLI the file's copy is used, and the refusal is named.
  const noStado = { PATH: '/usr/bin:/bin', HOME: bareHome };
  const fromFile = bramaProbe(noStado);
  assert.ok(fromFile.includes('address from an environment file, because Stado did not route it: the Stado CLI could not be started'), fromFile);

  // 4. No Stado and no address: the run stops before any model call and
  //    names the declaration that would route it.
  rmSync(join(workspace, '.env'));
  const refused = record(binary, ['run', 'Respond exactly: OK', '--model-only'], noStado, workspace);
  assert.ok(refused.status && !refused.error, `jeden run should have been refused: ${refused.stdout}`);
  assert.match(refused.stderr, /BRAMA_URL is required; declare Jeden's route with `stado service directory consumer-add brama jeden/);
  assert.match(refused.stderr, /Stado did not route it: the Stado CLI could not be started/);

  report.routed_url = routed;
  report.verdict = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
} finally {
  report.finished_at = new Date().toISOString();
  rmSync(workspace, { recursive: true, force: true });
  rmSync(bareHome, { recursive: true, force: true });
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, '  ')}\n`);
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
if (report.verdict !== 'passed') {
  throw new Error(`brama route journey failed: ${report.error}`);
}
