// Real native execution, cancellation and connection-owned cleanup. No model
// provider, worker or acceptance result is simulated. Run with a candidate binary
// and a real Brama route; this journey does not qualify the rendered Desktop UI.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startRpc } from '../hooks/rpc.mjs';
import { openQualificationTarget, qualificationEnvironment } from './fixtures/target.mjs';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} must be declared`);
  return value;
}
const root = fileURLToPath(new URL('../../', import.meta.url));
const binary = resolve(required('JEDEN_BIN'));
const model = required('JEDEN_TEST_MODEL');
const expectedRevision = required('JEDEN_TEST_SOURCE_REVISION');
const output = join(root, '.build', `hillclimb-control-${randomUUID()}`);
const transport = join(output, 'transport', 'root');
mkdirSync(transport, { recursive: true });
const ledger = join(output, 'history.jsonl');
const report = {
  binary, model, ledger, commands: [], frames: [], verdict: 'failed',
  binarySha256: createHash('sha256').update(readFileSync(binary)).digest('hex'),
};
let environment;
let fixture;
function command(program, args, cwd = root) {
  assert.ok(environment, 'The isolated qualification environment has not been established');
  const run = spawnSync(program, args, { cwd, env: environment, encoding: 'utf8' });
  const observed = { program, args, cwd, status: run.status, signal: run.signal,
    stdout: run.stdout, stderr: run.stderr, error: run.error?.message };
  report.commands.push(observed);
  if (run.error) throw run.error;
  assert.ok(run.status !== null && !run.status, `${program}: ${run.stderr}`);
  return run.stdout;
}
let client;
const failures = [];
try {
  environment = qualificationEnvironment(root, output);
  fixture = openQualificationTarget({ sourceRoot: root, output, command, report });
  const workspace = fixture.repository;
  const selectedLedger = relative(workspace, ledger);
  const wrongLedger = resolve(transport, selectedLedger);
  const once = new Set([workspace]).size;
  report.localSourceRevision = command('git', ['rev-parse', 'HEAD']).trim();
  assert.equal(report.localSourceRevision, expectedRevision);
  report.localSourcePatch = command('git', ['diff', 'HEAD', '--binary']);
  report.binaryVersion = command(binary, ['--version']).trim();
  report.sourceHashes = Object.fromEntries([
    'rust/hillclimb/execution.rs', 'rust/hillclimb/status/mod.rs', 'rust/autonomy.rs',
    'rust/rpc/server/hillclimb/mod.rs', 'rust/rpc/server/hillclimb/parameters.rs',
    'rust/rpc/server/hillclimb/requests.rs', 'tests/hillclimb/control.mjs',
    'tests/hillclimb/fixtures/greet.mjs',
    'tests/hillclimb/fixtures/target.mjs', 'tests/hillclimb/fixtures/README.md',
  ].map((path) => [path, createHash('sha256').update(readFileSync(join(root, path))).digest('hex')]));
  const before = readFileSync(join(workspace, 'greet.mjs'), 'utf8');
  client = startRpc({ binary, cwd: transport, env: environment, report });
  const options = { cwd: workspace, scope: 'repository', model, ledger: selectedLedger,
    allowWrite: true, allowCommand: true };
  await assert.rejects(client.rpc('hillclimb/run', { ...options, allowCommand: false }),
    (error) => error.code === 'permission_denied');
  await assert.rejects(client.rpc('hillclimb/run', { ...options, cwd: ' \t' }),
    (error) => error.code === 'invalid_params');
  await assert.rejects(client.rpc('hillclimb/cancel', { requestId: randomUUID() }),
    (error) => error.code === 'request_not_running');
  assert.deepEqual((await client.rpc('hillclimb/active', {})).requests, []);
  assert.equal(readFileSync(join(workspace, 'greet.mjs'), 'utf8'), before);
  assert.equal(existsSync(ledger), false, 'refusals do not create a ledger or change the target');

  const help = 'Usage: node greet.mjs NAME';
  // Focus is data, not a CLI option even when its first character is a dash.
  const completed = await client.rpc('hillclimb/run', { ...options, rounds: once,
    focus: `--help is missing: implement it in greet.mjs to print exactly ${JSON.stringify(help)}; preserve the greeting. Do not commit, push or create remotes.` });
  assert.equal(completed.accepted, once, JSON.stringify(completed));
  assert.equal(completed.rounds.length, once);
  assert.equal(resolve(completed.ledger), ledger);
  assert.equal(existsSync(wrongLedger), false, 'the relative ledger belongs to cwd, not the RPC process directory');
  const [first] = completed.rounds;
  assert.equal(first.ending, 'accepted');
  assert.ok(existsSync(first.receipt), 'the actual independent acceptance receipt exists');
  assert.equal(command(process.execPath, ['greet.mjs', '--help'], workspace).trim(), help);
  assert.equal(command(process.execPath, ['greet.mjs', 'Reader'], workspace).trim(), 'Hello, Reader');
  const status = await client.rpc('hillclimb/status', { cwd: workspace, ledger: selectedLedger });
  assert.deepEqual(status.rounds, completed.rounds);
  assert.equal(status.accepted, completed.accepted);
  assert.deepEqual((await client.rpc('hillclimb/active', {})).requests, []);
  const fromCli = JSON.parse(command(binary, ['hillclimb', '--status', '--cwd', workspace,
    '--ledger', selectedLedger, '--json'], transport));
  assert.deepEqual(fromCli, status, 'CLI status needs no grants and reads the same selected ledger');

  const wasCancelled = (error) => error.code === 'hillclimb_failed' && error.data?.cancellationRequested === true;
  const pending = client.request('hillclimb/run', { ...options,
    focus: 'Improve useful error messages while preserving greeting and --help. Do not commit, push or create remotes.' });
  await client.observeFrame((frame) => frame.type === 'hillclimbProgress' && frame.requestId === pending.id, pending.response);
  const [acknowledgement] = await Promise.all([
    client.rpc('hillclimb/cancel', { requestId: pending.id }),
    assert.rejects(pending.response, wasCancelled),
  ]);
  assert.equal(acknowledgement.requestId, pending.id);
  assert.equal(acknowledgement.cancellationRequested, true);
  assert.deepEqual((await client.rpc('hillclimb/active', {})).requests, [],
    'the cancellation response alone is not cleanup; the native run result precedes this assertion');
  await assert.rejects(client.rpc('hillclimb/cancel', { requestId: pending.id }),
    (error) => error.code === 'request_not_running');
  await client.rpc('shutdown', {});
  const firstExit = await client.end();
  assert.ok(firstExit.status !== null && !firstExit.status, firstExit.stderr);
  client = undefined;

  // EOF is a normal owned connection lifecycle, not a process kill or restart.
  client = startRpc({ binary, cwd: transport, env: environment, report });
  const onClose = client.request('hillclimb/run', { ...options,
    focus: 'Improve the greeter diagnostics without changing --help. Do not commit, push or create remotes.' });
  await client.observeFrame((frame) => frame.type === 'hillclimbProgress' && frame.requestId === onClose.id, onClose.response);
  const [eofExit] = await Promise.all([client.end(), assert.rejects(onClose.response, wasCancelled)]);
  assert.ok(eofExit.status !== null && !eofExit.status, eofExit.stderr);
  client = undefined;

  // A later CLI owner must acquire the released lock and actually improve it.
  const after = JSON.parse(command(binary, ['hillclimb', '--cwd', workspace, '--model', model,
    '--ledger', selectedLedger, '--rounds', String(once), '--allow-write', '--allow-command', '--json',
    'Add greet.mjs --goodbye NAME printing exactly Goodbye, followed by a space and NAME; preserve greeting and --help. Do not commit, push or create remotes.'], transport));
  assert.equal(after.accepted, once, JSON.stringify(after));
  assert.equal(command(process.execPath, ['greet.mjs', '--goodbye', 'Reader'], workspace).trim(), 'Goodbye, Reader');
  assert.equal(command(process.execPath, ['greet.mjs', '--help'], workspace).trim(), help);
  const final = JSON.parse(command(binary, ['hillclimb', '--status', '--cwd', workspace,
    '--ledger', selectedLedger, '--json'], transport));
  const persisted = readFileSync(ledger, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
  assert.deepEqual(final.rounds, persisted);
  assert.deepEqual(final.rounds.find((round) => round.round === first.round), first, 'earlier accepted work remains recorded');
  assert.equal(existsSync(wrongLedger), false);
  report.finalState = final;
  assert.equal(command('git', ['rev-parse', 'HEAD']).trim(), report.localSourceRevision,
    'the candidate source revision must remain unchanged during the journey');
  assert.equal(command('git', ['diff', 'HEAD', '--binary']), report.localSourcePatch,
    'the candidate source snapshot must remain unchanged during the journey');
} catch (error) {
  failures.push(error);
} finally {
  if (client) {
    try {
      const stopped = await client.end();
      assert.ok(stopped.status !== null && !stopped.status, stopped.stderr);
    } catch (error) { failures.push(error); }
  }
  if (fixture) {
    try { fixture.restore(); } catch (error) { failures.push(error); }
  }
  report.verdict = failures.length ? 'failed' : 'passed';
  report.failures = failures.map((error) => String(error));
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '\t'));
  console.log(join(output, 'report.json'));
}
if (failures.length) throw new AggregateError(failures, 'The real native hillclimb control journey failed');
