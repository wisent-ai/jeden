// Real test that `jeden update` recovers an interrupted update only from
// what it could examine. A journal left in the backed-up phase beside a
// backup path that cannot be examined (a symbolic link to itself) is refused
// naming that path, and the journal and stage stay exactly where they were;
// read as absent, recovery threw away the stage and journal and left the
// last-known-good binary unrestored. Once the backup is a real file the same
// recovery restores it as the target and removes the stage and journal, and
// the update lock the first run took was released with that run.
// Every command, its exit status and output go to the run's report.json.
//
// Usage: JEDEN_BIN=<built jeden> node tests/update/recovery.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const binary = process.env.JEDEN_BIN;
if (!binary) {
  throw new Error('JEDEN_BIN must name the built jeden this test runs; the test proves that binary, not whichever is on PATH');
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const runs = join(root, 'build/real-tests/update-recovery');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const report = { started_at: new Date().toISOString(), binary, commands: [], verdict: 'failed' };

const install = join(output, 'install');
mkdirSync(install);
const name = 'jeden-under-test';
const target = join(install, name);
const stage = join(install, `.${name}.jeden-update.stage`);
const backup = join(install, `.${name}.jeden-update.backup`);
const journal = join(install, `.${name}.jeden-update.journal.json`);
const lock = join(install, `.${name}.jeden-update.lock`);
const journalText = `${JSON.stringify({
  phase: 'backedUp',
  fromVersion: 'installed',
  toVersion: 'interrupted',
  artifactSha256: createHash('sha256').update('interrupted').digest('hex'),
  previousState: null,
})}\n`;

function update() {
  const args = ['update'];
  const result = spawnSync(binary, args, {
    cwd: root,
    encoding: 'utf8',
    env: {
      ...process.env,
      JEDEN_UPDATE_TARGET: target,
      // Recovery runs before the manifest is read; the manifest is never reached.
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

  writeFileSync(target, 'uncertain target\n');
  writeFileSync(stage, 'interrupted stage\n');
  writeFileSync(journal, journalText);
  symlinkSync(backup, backup);

  const refused = update();
  assert.ok(refused.status, `recovery accepted a backup it could not examine: ${refused.stdout}`);
  assert.match(refused.stderr, new RegExp(`${backup.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')} cannot be examined`));
  assert.equal(readFileSync(journal, 'utf8'), journalText, 'the journal changed although recovery refused');
  assert.equal(readFileSync(stage, 'utf8'), 'interrupted stage\n', 'the stage changed although recovery refused');
  assert.ok(lstatSync(backup).isSymbolicLink(), 'the unexaminable backup was touched');
  assert.equal(readFileSync(target, 'utf8'), 'uncertain target\n', 'the target changed although recovery refused');

  rmSync(backup);
  writeFileSync(backup, 'last-known-good\n');
  const recovered = update();
  assert.doesNotMatch(recovered.stderr, /cannot be examined/);
  // The first run's lock went with its process: the second took it, and the
  // lock file stays in place naming the process that last held it.
  assert.doesNotMatch(recovered.stderr, /another updater holds the update lock/);
  assert.match(readFileSync(lock, 'utf8'), /^\d+\n$/, 'the update lock does not name its last holder');
  assert.equal(readFileSync(target, 'utf8'), 'last-known-good\n', 'the last-known-good binary was not restored');
  assert.ok(!existsSync(backup), 'the restored backup is still beside the target');
  assert.ok(!existsSync(stage), 'the interrupted stage was left behind');
  assert.ok(!existsSync(journal), 'the recovered journal was left behind');
  report.verdict = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, '  ')}\n`);
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
if (report.verdict !== 'passed') {
  throw new Error(`update recovery journey failed: ${report.error}`);
}
