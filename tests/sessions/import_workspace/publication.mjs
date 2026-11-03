import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

function publishedCleanupFailure(context, withFixture, identity) {
  const { jeden, source, environment, invoke, success, report, retain, digest } = context;
  const name = 'published-backup';
  withFixture(context, name, (root) => {
    const scoped = { ...environment, JEDEN_SESSION_ROOT: root };
    const first = JSON.parse(success(invoke(jeden, ['import', source, '--json'], scoped)));
    assert.deepEqual(first.failures, [], JSON.stringify(first));
    const [session, ...others] = first.sessions;
    assert.ok(session);
    assert.deepEqual(others, []);
    const destination = realpathSync(session.sessionPath);
    assert.equal(dirname(destination), realpathSync(root), 'only the isolated native ledger may be changed');
    const transcript = join(destination, 'transcript.jsonl');
    const previous = { identity: identity(destination), transcriptSha256: digest(transcript) };
    const before = JSON.parse(readFileSync(join(destination, 'state.json'), 'utf8'));
    success(invoke('/bin/chmod', ['+a', 'everyone deny delete_child', destination]));
    success(invoke('/bin/chmod', ['+a', 'everyone deny delete', transcript]));
    const failed = JSON.parse(success(invoke(jeden, ['import', source, '--refresh', '--json'], scoped)));
    report.filesystemObservations[name] = { root, first, previous, failed, destination };
    retain();
    assert.deepEqual(failed.sessions, []);
    const [refusal, ...additional] = failed.failures;
    assert.ok(refusal);
    assert.deepEqual(additional, []);
    assert.equal(refusal.sourcePath, source);
    const [kept, ...unrelated] = readdirSync(root, { withFileTypes: true })
      .filter((entry) => entry.isDirectory() && entry.name.startsWith('.previous-'));
    assert.ok(kept, 'a refused backup cleanup must retain its observed backup');
    assert.deepEqual(unrelated, []);
    const backup = join(root, kept.name);
    const current = identity(destination);
    assert.deepEqual(identity(backup), previous.identity, 'the previous actual directory moved to the backup');
    assert.notDeepEqual(current, previous.identity, 'a different real directory was published at the native session path');
    assert.equal(digest(join(backup, 'transcript.jsonl')), previous.transcriptSha256);
    const after = JSON.parse(readFileSync(join(destination, 'state.json'), 'utf8'));
    assert.equal(after.id, before.id);
    assert.equal(after.cwd, before.cwd);
    assert.equal(digest(join(destination, 'artifacts', `${session.format}-source.jsonl`)), digest(source));
    assert.ok(refusal.error.includes(destination), refusal.error);
    assert.ok(refusal.error.includes(backup), refusal.error);
    report.filesystemObservations[name] = {
      root, first, previous, failed, destination, backup, current, before, after,
    };
    retain();
  });
}

function publicationAndRecoveryFailure(context, withFixture, identity) {
  const { jeden, source, environment, invoke, success, report, retain, digest } = context;
  const name = 'publication-and-recovery';
  withFixture(context, name, (root) => {
    const scoped = { ...environment, JEDEN_SESSION_ROOT: root };
    const first = JSON.parse(success(invoke(jeden, ['import', source, '--json'], scoped)));
    assert.deepEqual(first.failures, [], JSON.stringify(first));
    const [session, ...others] = first.sessions;
    assert.ok(session, 'the real importer must create the isolated ledger');
    assert.deepEqual(others, []);
    const destination = realpathSync(session.sessionPath);
    assert.equal(dirname(destination), realpathSync(root));
    const snapshot = (directory) => ({
      identity: identity(directory),
      state: digest(join(directory, 'state.json')),
      transcript: digest(join(directory, 'transcript.jsonl')),
      completion: digest(join(directory, 'completion.json')),
      sourceArtifact: digest(join(directory, 'artifacts', `${session.format}-source.jsonl`)),
    });
    const before = snapshot(destination);
    const profile = join(root, 'publication.sb');
    // -f and -D are sandbox-exec's native interface. The policy affects only
    // creation at this real destination, not moving its old inode to backup.
    const policy = '(version 1)\n(allow default)\n'
      + '(deny file-write-create (literal (param "IMPORT_DESTINATION")))\n';
    writeFileSync(profile, policy);
    const observation = { root, destination, first, before, profile, policy,
      policySha256: digest(profile), sourceSha256: digest(source) };
    report.filesystemObservations[name] = observation;
    retain();
    const sandboxImport = (options) => JSON.parse(success(invoke('/usr/bin/sandbox-exec',
      ['-f', profile, '-D', `IMPORT_DESTINATION=${destination}`, jeden,
        'import', source, ...options, '--json'], scoped)));
    const failed = sandboxImport(['--refresh']);
    observation.failed = failed;
    observation.destinationExists = existsSync(destination);
    observation.entries = readdirSync(root, { withFileTypes: true })
      .map((entry) => ({ name: entry.name, directory: entry.isDirectory() }));
    retain();
    assert.deepEqual(failed.sessions, []);
    const [refusal, ...additional] = failed.failures;
    assert.ok(refusal, 'the real importer must report the combined failure');
    assert.deepEqual(additional, []);
    assert.equal(refusal.sourcePath, source);
    assert.equal(observation.destinationExists, false,
      'this case must reach failed publication and recovery, not an earlier admission refusal');
    const [kept, ...unrelated] = observation.entries
      .filter((entry) => entry.directory && entry.name.startsWith('.previous-'));
    assert.ok(kept, 'failed recovery must leave the real previous directory');
    assert.deepEqual(unrelated, []);
    const backup = join(root, kept.name);
    observation.backup = backup;
    observation.afterFailure = snapshot(backup);
    retain();
    assert.deepEqual(observation.afterFailure, before,
      'both refused renames must preserve the original inode and every retained ledger artifact');
    const stagingIndex = refusal.error.indexOf(join(root, '.import-'));
    const backupIndex = refusal.error.indexOf(backup);
    assert.ok(stagingIndex >= 0 && backupIndex > stagingIndex, refusal.error);
    assert.ok(refusal.error.slice(stagingIndex, backupIndex).includes(destination), refusal.error);
    assert.ok(refusal.error.slice(backupIndex + backup.length).includes(destination), refusal.error);
    // Observe recovery independently through the same real importer. Its actual
    // cause must also survive when recovery follows a failed publication.
    const recovery = sandboxImport([]);
    observation.recovery = recovery;
    observation.afterRecoveryRefusal = snapshot(backup);
    retain();
    assert.deepEqual(recovery.sessions, []);
    const [recoveryRefusal, ...otherRecovery] = recovery.failures;
    assert.ok(recoveryRefusal);
    assert.deepEqual(otherRecovery, []);
    assert.equal(recoveryRefusal.sourcePath, source);
    assert.ok(recoveryRefusal.error.includes(backup), recoveryRefusal.error);
    assert.ok(recoveryRefusal.error.includes(destination), recoveryRefusal.error);
    assert.ok(refusal.error.includes(recoveryRefusal.error), refusal.error);
    assert.deepEqual(observation.afterRecoveryRefusal, before);
    assert.equal(existsSync(destination), false);
    // The normal public import operation, not a manual filesystem repair,
    // restores its interrupted publication after the restricted child exits.
    const restored = JSON.parse(success(invoke(jeden, ['import', source, '--json'], scoped)));
    observation.restored = restored;
    observation.afterRestore = snapshot(destination);
    observation.backupExistsAfterRestore = existsSync(backup);
    retain();
    assert.deepEqual(restored.failures, [], JSON.stringify(restored));
    assert.deepEqual(restored.sessions.map((entry) => realpathSync(entry.sessionPath)), [destination]);
    assert.deepEqual(observation.afterRestore, before);
    assert.equal(observation.backupExistsAfterRestore, false);
    assert.equal(digest(source), observation.sourceSha256, 'qualification must not change its genuine input');
  });
}

export function publicationJourneys(context, withFixture, identity) {
  publishedCleanupFailure(context, withFixture, identity);
  publicationAndRecoveryFailure(context, withFixture, identity);
}
