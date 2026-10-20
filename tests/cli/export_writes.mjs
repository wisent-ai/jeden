// Real test that printing a session has one command: `jeden show` prints a
// session, and `jeden export` only writes one to a file. An export without
// its output file is refused as a usage error with nothing printed; with
// one, the file holds the record `jeden show --json` prints, and the
// command prints the path it wrote.
//
// Runs the built jeden against this machine's own recorded sessions; it
// reads them and writes only under the package's build directory.
// Every command, its exit status and output go to the run's report.json.
//
// Usage: JEDEN_BIN=<built jeden> JEDEN_TEST_SOURCE_REVISION=<commit> node tests/cli/export_writes.mjs
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
const expectedRevision = process.env.JEDEN_TEST_SOURCE_REVISION;
assert.ok(expectedRevision?.trim(), 'JEDEN_TEST_SOURCE_REVISION must name the candidate checkout');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const runs = join(root, 'build/real-tests/export-writes');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const report = {
  started_at: new Date().toISOString(), binary, commands: [], verdict: 'failed',
  scope: "Real jeden sessions, show and export against this machine's recorded sessions; files are written only under the run directory.",
};

function jeden(args) {
  const result = spawnSync(binary, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ args, exit_status: result.status, signal: result.signal, stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ok(!result.error, `jeden ${args.join(' ')} did not run: ${result.error}`);
  return result;
}
function git(args) {
  const result = spawnSync('git', args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ program: 'git', args, exit_status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

try {
  report.revision = git(['rev-parse', 'HEAD']);
  assert.equal(report.revision, expectedRevision, 'qualify only the declared checkout');
  assert.equal(git(['status', '--porcelain']), '', 'commit the candidate and journey before qualification');
  const short = git(['rev-parse', '--short', expectedRevision]);
  const version = jeden(['--version']);
  assert.equal(version.status, 0, version.stderr);
  assert.ok(version.stdout.trim().endsWith(`.${short}`),
    `the CLI binary must embed the clean source revision: ${version.stdout}`);
  report.test_sha256 = createHash('sha256').update(readFileSync(fileURLToPath(import.meta.url))).digest('hex');
  report.binary_sha256 = createHash('sha256').update(readFileSync(binary)).digest('hex');

  // Measure a usage refusal, not an unknown command's execution failure.
  const invalidLimit = jeden(['sessions', 'not-a-number']);
  assert.ok(invalidLimit.status, 'an invalid sessions limit was accepted');
  assert.match(invalidLimit.stderr, /whole number/);
  const usage = invalidLimit.status;

  const bare = jeden(['export']);
  assert.equal(bare.status, usage, `export without a session exited ${bare.status}`);
  assert.match(bare.stderr, /export requires a session id or path/);

  const listed = jeden(['sessions', '--json']);
  assert.ok(!listed.status, `jeden sessions failed: ${listed.stderr}`);
  const [session] = JSON.parse(listed.stdout);
  assert.ok(session, 'BLOCKED: no recorded session on this machine, so export has nothing to write');
  report.session = session;
  const zero = jeden(['sessions', '0', '--json']);
  assert.equal(zero.status, 0, zero.stderr);
  assert.deepEqual(JSON.parse(zero.stdout), [], 'a zero limit must list no sessions');
  for (const args of [['sessions', '1', 'extra'], ['show', session, 'extra']]) {
    const refused = jeden(args);
    assert.equal(refused.status, usage, `${args[0]} accepted a trailing argument`);
    assert.match(refused.stderr, /unexpected argument "extra"/);
    assert.equal(refused.stdout, '', 'an invalid read command printed a successful session');
  }

  const printed = jeden(['export', session]);
  assert.equal(printed.status, usage, `export without a file exited ${printed.status}`);
  assert.match(printed.stderr, /export requires the file to write/);
  assert.equal(printed.stdout, '', 'a refused export printed the session');

  const stray = jeden(['export', session, join(output, 'a.json'), '--pdf']);
  assert.equal(stray.status, usage, `an unknown export flag exited ${stray.status}`);
  assert.match(stray.stderr, /export: unexpected argument "--pdf"/);

  const file = join(output, 'session.json');
  const written = jeden(['export', session, file]);
  assert.ok(!written.status, `export failed: ${written.stderr}`);
  assert.equal(written.stdout.trim(), file, 'export did not print the path it wrote');

  const shown = jeden(['show', session, '--json']);
  assert.ok(!shown.status, `show failed: ${shown.stderr}`);
  assert.deepEqual(JSON.parse(readFileSync(file, 'utf8')), JSON.parse(shown.stdout), 'the exported file differs from what show --json prints');

  report.verdict = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, '  ')}\n`);
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
if (report.verdict !== 'passed') {
  throw new Error(`export journey failed: ${report.error}`);
}
