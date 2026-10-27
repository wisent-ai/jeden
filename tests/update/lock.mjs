// Real test that `jeden update`'s lock belongs to the process holding it:
// the operating system releases it when that updater ends however it ends,
// so the next updater takes it without a lock file to remove, and the lock
// file stays beside the install naming the process that last held it.
// Releasing the lock used to mean removing its file, and a removal or a
// directory sync that failed was dropped without a word.
// Every command, its exit status and output go to the run's report.json.
//
// Usage: JEDEN_BIN=<built jeden> node tests/update/lock.mjs
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
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const runs = join(root, 'build/real-tests/update-lock');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const report = { started_at: new Date().toISOString(), binary, commands: [], verdict: 'failed' };

const install = join(output, 'install');
mkdirSync(install);
const name = 'jeden-under-test';
const target = join(install, name);
const lock = join(install, `.${name}.jeden-update.lock`);

function update() {
  const args = ['update'];
  const result = spawnSync(binary, args, {
    cwd: root,
    encoding: 'utf8',
    env: {
      ...process.env,
      JEDEN_UPDATE_TARGET: target,
      // The lock is taken before the manifest is read; the missing manifest
      // ends each run after the lock was held and released.
      JEDEN_UPDATE_MANIFEST: join(output, 'no-manifest.json'),
    },
  });
  report.commands.push({ args, exit_status: result.status, signal: result.signal, stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ok(!result.error, `jeden update did not run: ${result.error}`);
  return result;
}

try {
  report.revision = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim();
  report.test_sha256 = createHash('sha256').update(readFileSync(fileURLToPath(import.meta.url))).digest('hex');
  report.binary_sha256 = createHash('sha256').update(readFileSync(binary)).digest('hex');
  writeFileSync(target, 'installed\n');

  const first = update();
  assert.match(first.stderr, /no-manifest\.json/, `the first update did not reach its manifest: ${first.stderr}`);
  const firstHolder = readFileSync(lock, 'utf8');
  assert.match(firstHolder, /^\d+\n$/, 'the update lock does not name the process that held it');

  const second = update();
  assert.doesNotMatch(second.stderr, /another updater holds the update lock/, 'the lock outlived the updater that held it');
  assert.match(second.stderr, /no-manifest\.json/, `the second update did not reach its manifest: ${second.stderr}`);
  const secondHolder = readFileSync(lock, 'utf8');
  assert.match(secondHolder, /^\d+\n$/, 'the update lock does not name the process that held it');
  assert.notEqual(secondHolder, firstHolder, 'the second updater did not record itself as the holder');
  assert.equal(readFileSync(target, 'utf8'), 'installed\n', 'an update that read no manifest changed the target');
  report.verdict = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, '  ')}\n`);
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
if (report.verdict !== 'passed') {
  throw new Error(`update lock journey failed: ${report.error}`);
}
