import assert from 'node:assert/strict';
import { copyFileSync, readFileSync, realpathSync } from 'node:fs';
import { isAbsolute, join } from 'node:path';

function committedSource({ invoke, success }, root, expected) {
  assert.equal(success(invoke('git', ['-C', root, 'rev-parse', 'HEAD'])).trim(), expected);
  success(invoke('git', ['-C', root, 'diff', '--quiet', 'HEAD']));
  assert.equal(success(invoke('git', ['-C', root, 'ls-files', '--others', '--exclude-standard'])), '');
}

export function bindJedenExecutable(context, root, expected) {
  const { jeden, invoke, success, report, retain, digest } = context;
  committedSource(context, root, expected);
  const version = success(invoke(jeden, ['--version'])).trim();
  report.jedenExecutableIdentity = { version, sha256: digest(jeden) };
  retain();
  // The existing build contract emits this source-bearing development form.
  // A release override or a dirty/unknown identity cannot attest this commit.
  const observed = /^jeden \S+\+dev\.\d+\.(?<revision>[a-fA-F\d]+)$/.exec(version);
  assert.ok(observed?.groups?.revision,
    `the actual Jeden version carries no clean source revision: ${version}`);
  const resolved = success(invoke('git', ['-C', root, 'rev-parse', '--verify',
    `${observed.groups.revision}^{commit}`])).trim();
  report.jedenExecutableIdentity.sourceRevision = resolved;
  retain();
  assert.equal(resolved, expected, 'the actual Jeden executable must identify the candidate commit');
}

export function bindOkoExecutable(context, root, expected) {
  const { oko, invoke, success, output, environment, report, retain, digest } = context;
  committedSource(context, root, expected);
  const version = success(invoke(oko, ['version'])).trim();
  const location = success(invoke(oko, ['defects', 'location'])).trim();
  assert.ok(isAbsolute(location), 'the actual Oko register location must be absolute');
  const register = join(output, 'oko-identity-register.json');
  copyFileSync(location, register);
  const sourceSha256 = digest(register);
  const actual = JSON.parse(readFileSync(register, 'utf8'));
  assert.ok(Array.isArray(actual.defects), 'the actual Oko issuer document must contain its defects array');
  const [record] = actual.defects;
  assert.ok(record && typeof record.id === 'string' && record.id,
    'the existing integrity probe requires an actual issued defect, not a handwritten test record');
  report.okoExecutableIdentity = {
    version, sha256: digest(oko), registerSource: location, registerSnapshotSha256: sourceSha256,
    isolatedRegister: register, actualDefect: record.id,
  };
  retain();
  const scoped = { ...environment, OKO_DEFECT_REGISTER: register };
  try {
    const selected = success(invoke(oko, ['defects', 'location'], scoped)).trim();
    assert.equal(realpathSync(selected), realpathSync(register));
    // This existing product probe checks compiled OKO_BUILT_REVISION before
    // reading its isolated row. A version label or digest alone cannot.
    const observed = JSON.parse(success(invoke(oko, ['defects', 'integrity-inventory',
      '--id', record.id, '--revision', `oko@${expected}`, '--json'], scoped)));
    report.okoExecutableIdentity.observed = observed;
    retain();
    assert.equal(observed.sourceRevision, `oko@${expected}`);
    assert.deepEqual(observed.remaining, [], 'the actual source-bound CLI and persisted row must agree');
  } catch (error) {
    report.okoExecutableIdentity.error = String(error.stack || error);
    try { retain(); }
    catch (publication) {
      throw new AggregateError([error, publication],
        'executable identity refusal and qualification-report publication failed');
    }
    throw error;
  }
}
