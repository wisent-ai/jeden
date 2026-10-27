import assert from 'node:assert/strict';
import { closeSync, copyFileSync, mkdirSync, openSync, readFileSync, readdirSync, realpathSync, rmSync, statSync, symlinkSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { userInfo } from 'node:os';
import { publicationJourneys } from './publication.mjs';

function withFixture(context, name, exercise) {
  const { output, invoke, success, report, retain } = context;
  assert.equal(process.platform, 'darwin', 'the refusal journeys require real macOS ACLs');
  const root = join(output, `${name}-sessions`);
  mkdirSync(root, { recursive: true });
  report.filesystemObservations[name] = { root };
  let failure;
  const restoreFirst = [];
  try {
    exercise(root, restoreFirst);
  } catch (error) {
    failure = error;
    report.filesystemObservations[name].qualificationFailure = String(error.stack || error);
  }
  const cleanupFailures = [];
  // Restore directly addressable parent permissions before traversing them.
  for (const path of [...restoreFirst, root]) {
    try {
      success(invoke('/bin/chmod', [path === root ? '-RN' : '-N', path]));
    } catch (cleanup) {
      cleanupFailures.push(cleanup);
    }
  }
  if (cleanupFailures.length) {
    report.filesystemObservations[name].aclResetFailures =
      cleanupFailures.map((error) => String(error.stack || error));
  }
  try {
    retain();
  } catch (publication) {
    cleanupFailures.push(publication);
  }
  if (cleanupFailures.length) {
    const failures = failure ? [failure, ...cleanupFailures] : cleanupFailures;
    throw new AggregateError(failures,
      `qualification, ACL restoration or evidence retention failed:\n${failures.map((error) => String(error.stack || error)).join('\n')}`);
  }
  if (failure) throw failure;
}

function creationFailure(context, { name, rule, artifact, probeCreation }) {
  const { jeden, source, environment, invoke, success, report, retain } = context;
  withFixture(context, name, (root) => {
    // Native inheritance and the alternative deletion permissions:
    // https://github.com/apple-oss-distributions/file_cmds/blob/main/chmod/chmod.1
    success(invoke('/bin/chmod', ['+a', `user:${userInfo().username} deny delete_child`, root]));
    success(invoke('/bin/chmod', ['+a', rule, root]));
    const failed = JSON.parse(success(invoke(jeden, ['import', source, '--json'],
      { ...environment, JEDEN_SESSION_ROOT: root })));
    report.filesystemObservations[name] = { failed, root };
    retain();
    assert.deepEqual(failed.sessions, [], 'a failed import cannot become a session');
    const [observed, ...others] = failed.failures;
    assert.ok(observed, 'the real importer reports the failed source');
    assert.deepEqual(others, []);
    assert.equal(observed.sourcePath, source);
    const directories = readdirSync(root, { withFileTypes: true })
      .filter((entry) => entry.isDirectory() && entry.name.startsWith('.import-'));
    const [staged, ...unrelated] = directories;
    assert.ok(staged, 'the actual failed rollback leaves its staging directory for inspection');
    assert.deepEqual(unrelated, []);
    const staging = join(root, staged.name);
    const target = join(staging, ...artifact);
    report.filesystemObservations[name] = { failed, root, staging, target };
    retain();
    assert.ok(observed.error.includes(target), observed.error);
    const targetEnd = observed.error.indexOf(target) + target.length;
    const cleanupStart = observed.error.lastIndexOf(staging);
    assert.ok(cleanupStart >= targetEnd,
      `the cleanup target must be reported independently after the initiating ${name} failure: ${observed.error}`);
    if (probeCreation) {
      // Measure opening this exact retained target for creation itself.
      let descriptor;
      let cause;
      try {
        descriptor = openSync(target, 'wx');
      } catch (error) {
        cause = error;
      } finally {
        if (descriptor !== undefined) closeSync(descriptor);
      }
      assert.ok(cause, `the actual fault fixture unexpectedly allowed creation of ${target}`);
      assert.equal(cause.syscall, 'open');
      report.filesystemObservations[name].creationRefusal = {
        code: cause.code, errno: cause.errno, syscall: cause.syscall,
        path: cause.path, message: cause.message,
      };
      retain();
      const initiating = observed.error.slice(targetEnd, cleanupStart);
      assert.ok(initiating.includes(`os error ${Math.abs(cause.errno)}`), observed.error);
    }
  });
}

function identity(path) {
  const actual = statSync(path, { bigint: true });
  return { device: actual.dev.toString(), inode: actual.ino.toString() };
}


function metadataRefusal(context, { name, artifact }) {
  const { jeden, source, environment, invoke, success, report, retain, digest } = context;
  withFixture(context, name, (root, restoreFirst) => {
    const scoped = { ...environment, JEDEN_SESSION_ROOT: root };
    const initial = JSON.parse(success(invoke(jeden, ['import', source, '--json'], scoped)));
    assert.deepEqual(initial.failures, [], JSON.stringify(initial));
    const [session, ...others] = initial.sessions;
    assert.ok(session);
    assert.deepEqual(others, []);
    const destination = realpathSync(session.sessionPath);
    assert.equal(dirname(destination), realpathSync(root));
    const state = join(destination, 'state.json');
    const transcript = join(destination, 'transcript.jsonl');
    const protectedDirectory = join(root, 'unreadable-metadata');
    mkdirSync(protectedDirectory);
    const protectedState = join(protectedDirectory, 'state.json');
    copyFileSync(state, protectedState);
    const target = artifact ? join(destination, artifact) : protectedState;
    if (artifact) {
      rmSync(target, { force: true });
      symlinkSync(protectedState, target);
    }
    const before = {
      identity: identity(destination), stateSha256: digest(state),
      transcriptSha256: digest(transcript), targetSha256: digest(target),
    };
    // The parent itself remains addressable for ACL restoration; stat of
    // its child (also through the optional-artifact link) genuinely fails.
    restoreFirst.push(protectedDirectory);
    success(invoke('/bin/chmod', ['+a', 'everyone deny search', protectedDirectory]));
    let cause;
    try {
      statSync(target);
    } catch (error) {
      cause = error;
    }
    assert.ok(cause, `the actual fixture unexpectedly allowed metadata inspection of ${target}`);
    const metadataError = {
      code: cause.code, errno: cause.errno, syscall: cause.syscall,
      path: cause.path, message: cause.message,
    };
    report.filesystemObservations[name] = { root, initial, destination, target, before, metadataError };
    retain();
    const failed = JSON.parse(success(invoke(jeden, ['import', source, '--refresh', '--json'], scoped)));
    report.filesystemObservations[name].failed = failed;
    retain();
    assert.deepEqual(failed.sessions, []);
    const [refusal, ...additional] = failed.failures;
    assert.ok(refusal);
    assert.deepEqual(additional, []);
    assert.equal(refusal.sourcePath, source);
    assert.ok(refusal.error.includes(target), refusal.error);
    assert.ok(refusal.error.includes(`os error ${Math.abs(cause.errno)}`), refusal.error);
    assert.deepEqual(identity(destination), before.identity,
      'unknown adoption, work or lineage metadata cannot authorize replacement');
    assert.equal(digest(state), before.stateSha256);
    assert.equal(digest(transcript), before.transcriptSha256);
  });
  const observed = report.filesystemObservations[name];
  assert.equal(digest(observed.target), observed.before.targetSha256,
    'the refused refresh preserves the exact isolated artifact bytes');
}

function workReadRefusal(context) {
  const { jeden, source, environment, invoke, success, report, retain, digest } = context;
  const name = 'work-read';
  withFixture(context, name, (root) => {
    const scoped = { ...environment, JEDEN_SESSION_ROOT: root };
    const initial = JSON.parse(success(invoke(jeden, ['import', source, '--json'], scoped)));
    assert.deepEqual(initial.failures, [], JSON.stringify(initial));
    const [session, ...others] = initial.sessions;
    assert.ok(session);
    assert.deepEqual(others, []);
    const destination = realpathSync(session.sessionPath);
    assert.equal(dirname(destination), realpathSync(root));
    const target = join(destination, 'completion.json');
    assert.ok(statSync(target).isFile(), 'the actual importer must persist a regular completion record');
    const state = join(destination, 'state.json');
    const transcript = join(destination, 'transcript.jsonl');
    const before = {
      identity: identity(destination), stateSha256: digest(state),
      transcriptSha256: digest(transcript), targetSha256: digest(target),
    };
    success(invoke('/bin/chmod', ['+a', 'everyone deny read', target]));
    let cause;
    try {
      readFileSync(target);
    } catch (error) {
      cause = error;
    }
    const readError = cause ? {
      code: cause.code, errno: cause.errno, syscall: cause.syscall,
      path: cause.path, message: cause.message,
    } : null;
    report.filesystemObservations[name] = { root, initial, destination, target, before, readError };
    retain();
    assert.ok(cause, `the real ACL fixture unexpectedly allowed reading ${target}`);
    const failed = JSON.parse(success(invoke(jeden, ['import', source, '--refresh', '--json'], scoped)));
    report.filesystemObservations[name].failed = failed;
    retain();
    assert.deepEqual(failed.sessions, []);
    const [refusal, ...additional] = failed.failures;
    assert.ok(refusal);
    assert.deepEqual(additional, []);
    assert.equal(refusal.sourcePath, source);
    assert.ok(refusal.error.includes(target), refusal.error);
    assert.ok(refusal.error.includes(`os error ${Math.abs(cause.errno)}`), refusal.error);
    assert.deepEqual(identity(destination), before.identity);
    assert.equal(digest(state), before.stateSha256);
    assert.equal(digest(transcript), before.transcriptSha256);
  });
  const observed = report.filesystemObservations[name];
  assert.equal(digest(observed.target), observed.before.targetSha256,
    'a refused completion read must preserve the original work record bytes');
}

export function filesystemJourneys(context, session) {
  creationFailure(context, {
    name: 'snapshot',
    rule: 'everyone deny add_file,delete,delete_child,directory_inherit,only_inherit',
    artifact: ['artifacts', `${session.format}-source.jsonl`], probeCreation: false,
  });
  creationFailure(context, {
    name: 'state-json',
    // Inherit only to staging, not to its artifacts directory.
    rule: 'everyone deny add_file,delete,delete_child,directory_inherit,only_inherit,limit_inherit',
    artifact: ['state.json'], probeCreation: true,
  });
  publicationJourneys(context, withFixture, identity);
  metadataRefusal(context, { name: 'adoption-metadata', artifact: 'adopted' });
  workReadRefusal(context);
  metadataRefusal(context, { name: 'lineage-metadata', artifact: null });
}
