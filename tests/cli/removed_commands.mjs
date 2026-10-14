// Real test that a command folded into a general primitive is gone, not kept
// as a second spelling: `jeden worktree` (cleaning up Git worktrees jeden
// cannot create is git's own `worktree list|remove|prune`) and `jeden
// restore` (resuming every agent session stopped since a time is `oko
// sessions resume --since`) are refused as unknown commands with the same
// usage status any unknown word gets, with or without their former actions
// and options, and print nothing.
// Every command, its exit status and output go to the run's report.json.
//
// Usage: JEDEN_BIN=<built jeden> node tests/cli/removed_commands.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const binary = process.env.JEDEN_BIN;
if (!binary) {
  throw new Error('JEDEN_BIN must name the built jeden this test runs; the test proves that binary, not whichever is on PATH');
}
const worktree = 'worktree';
const restore = 'restore';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const runs = join(root, 'build/real-tests/removed-commands');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const report = { started_at: new Date().toISOString(), binary, commands: [], verdict: 'failed' };

function jeden(args) {
  const result = spawnSync(binary, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ args, exit_status: result.status, signal: result.signal, stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ok(!result.error, `jeden ${args.join(' ')} did not run: ${result.error}`);
  return result;
}

function refusedAsUnknown(removed, args, usage) {
  const line = [removed, ...args];
  const result = jeden(line);
  assert.equal(result.status, usage, `jeden ${line.join(' ')} exited ${result.status}, not as an unknown command`);
  assert.match(result.stderr, new RegExp(`unknown command: ${removed}`));
  assert.equal(result.stdout, '', `jeden ${line.join(' ')} printed output`);
}

try {
  report.revision = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim();
  report.test_sha256 = createHash('sha256').update(readFileSync(fileURLToPath(import.meta.url))).digest('hex');
  report.binary_sha256 = createHash('sha256').update(readFileSync(binary)).digest('hex');

  const unknown = jeden(['no-such-command']);
  assert.ok(unknown.status, `an unknown command was accepted: ${unknown.stdout}`);
  refusedAsUnknown(worktree, [], unknown.status);
  refusedAsUnknown(worktree, ['clear'], unknown.status);
  refusedAsUnknown(restore, [], unknown.status);
  refusedAsUnknown(restore, ['--since', 'today'], unknown.status);
  refusedAsUnknown(restore, ['open', join(output, 'session.jsonl')], unknown.status);
  report.verdict = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, '  ')}\n`);
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
if (report.verdict !== 'passed') {
  throw new Error(`removed commands journey failed: ${report.error}`);
}
