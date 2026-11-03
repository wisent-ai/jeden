import assert from 'node:assert/strict';
import { mkdirSync, readFileSync, realpathSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

function refusesChangedCompletion(context, name, field, change) {
  const { output, jeden, source, environment, invoke, success, report, retain, digest } = context;
  const root = join(output, `${name}-sessions`);
  mkdirSync(root, { recursive: true });
  const scoped = { ...environment, JEDEN_SESSION_ROOT: root };
  const imported = JSON.parse(success(invoke(jeden, ['import', source, '--json'], scoped)));
  assert.deepEqual(imported.failures, [], JSON.stringify(imported));
  const [session, ...others] = imported.sessions;
  assert.ok(session, 'the actual importer must issue the isolated native ledger');
  assert.deepEqual(others, []);
  const destination = realpathSync(session.sessionPath);
  assert.equal(dirname(destination), realpathSync(root));
  const completion = join(destination, 'completion.json');
  const document = JSON.parse(readFileSync(completion, 'utf8'));
  assert.ok(document.requests.length, 'a genuine pending request is required; an empty source cannot qualify typed request admission');
  change(document);
  writeFileSync(completion, JSON.stringify(document));
  const snapshot = () => {
    const identity = statSync(destination, { bigint: true });
    return {
      device: identity.dev.toString(), inode: identity.ino.toString(),
      state: digest(join(destination, 'state.json')),
      transcript: digest(join(destination, 'transcript.jsonl')),
      completion: digest(completion),
      sourceArtifact: digest(join(destination, 'artifacts', `${session.format}-source.jsonl`)),
    };
  };
  const before = snapshot();
  const observation = { root, destination, completion, field, before };
  report.completionRefusals[name] = observation;
  retain();
  const canonical = invoke(jeden, ['todo', 'list', '--session', destination, '--json'], scoped);
  observation.canonical = canonical;
  observation.afterCanonical = snapshot();
  retain();
  assert.equal(canonical.signal, null, 'a killed canonical reader is not a typed admission refusal');
  assert.ok(canonical.status, 'the actual canonical completion reader must refuse the malformed retained document');
  const canonicalLine = canonical.stderr.split(/\r?\n/)
    .find((line) => line.includes(completion));
  assert.ok(canonicalLine, 'the canonical refusal must name the actual completion file');
  // Preserve the reader's observed cause, not a guessed field-name rendering.
  const canonicalCause = canonicalLine.slice(canonicalLine.indexOf(completion));
  observation.canonicalCause = canonicalCause;
  assert.deepEqual(observation.afterCanonical, before, 'a failed canonical read must preserve the complete retained state');
  const refreshed = JSON.parse(success(invoke(jeden, ['import', source, '--refresh', '--json'], scoped)));
  observation.refresh = refreshed;
  observation.afterRefresh = snapshot();
  retain();
  assert.deepEqual(refreshed.sessions, [], 'malformed retained work must not authorize a destructive refresh');
  const [refusal, ...unrelated] = refreshed.failures;
  assert.ok(refusal, 'the real refresh must retain its source-specific refusal');
  assert.deepEqual(unrelated, []);
  assert.equal(refusal.sourcePath, source);
  assert.ok(refusal.error.includes(canonicalCause),
    'refresh must retain the actual canonical completion path and read refusal');
  assert.deepEqual(observation.afterRefresh, before,
    'a refused refresh must preserve directory identity, completion, transcript, metadata and retained source bytes');
}

export function completionJourneys(context) {
  context.report.completionRefusals = {};
  refusesChangedCompletion(context, 'missing-required-tasks', 'tasks', (document) => {
    assert.ok(Array.isArray(document.tasks));
    delete document.tasks;
  });
  refusesChangedCompletion(context, 'non-boolean-planned', 'planned', (document) => {
    const [request] = document.requests;
    assert.equal(request.planned, false, 'the actual imported request must still be unplanned');
    request.planned = String(request.planned);
  });
}
