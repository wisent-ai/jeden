// A real append can commit before the state mirror fails; never call it unwritten.
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { chmodSync, closeSync, constants, openSync, readFileSync, statSync, unlinkSync } from 'node:fs';
import { join } from 'node:path';

export async function exerciseMirror({ rpc, prompt, options, exported, report }) {
  const session = await rpc('session/new', { options });
  await prompt(session, '/checkpoint before-mirror-refusal');
  const statePath = join(session.sessionPath, 'state.json');
  const transcript = join(session.sessionPath, 'transcript.jsonl');
  const originalState = readFileSync(statePath);
  const before = exported(session.sessionPath, 'mirror-before');
  const mode = statSync(session.sessionPath).mode;
  const probe = join(session.sessionPath, `mirror-probe-${randomUUID()}`);
  let refusal;
  let probeFd;
  let diagnostic;
  const failures = [];
  try {
    chmodSync(session.sessionPath, mode & ~(constants.S_IWUSR | constants.S_IWGRP | constants.S_IWOTH));
    try {
      probeFd = openSync(probe, 'wx');
    } catch (error) { refusal = error; }
    assert.ok(refusal?.errno, 'the host must enforce actual state-file creation refusal');
    closeSync(openSync(transcript, 'a'));
    await assert.rejects(prompt(session, '/checkpoint committed-before-mirror-refusal'), (error) => {
      diagnostic = error.message;
      return diagnostic.includes('committed, but active leaf mirror update failed:')
        && diagnostic.includes('cannot create active-leaf state file')
        && diagnostic.includes(session.sessionPath)
        && diagnostic.includes(`os error ${Math.abs(refusal.errno)}`);
    });
    assert.deepEqual(readFileSync(statePath), originalState,
      'a failed mirror must leave the previous state bytes unchanged');
  } catch (error) {
    failures.push(error);
  } finally {
    try { chmodSync(session.sessionPath, mode); } catch (error) { failures.push(error); }
    if (probeFd !== undefined) {
      try { closeSync(probeFd); } catch (error) { failures.push(error); }
      try { unlinkSync(probe); } catch (error) { failures.push(error); }
    }
  }
  if (failures.length) {
    throw new AggregateError(failures, `active-leaf mirror qualification failed: ${failures.map(String).join('; ')}`);
  }
  const after = exported(session.sessionPath, 'mirror-committed');
  const committed = after.filter((event) => event.type === 'checkpoint'
    && event.data?.label === 'committed-before-mirror-refusal');
  assert.deepEqual(after.filter((event) => before.some((prior) => prior.id === event.id)), before,
    'the committed append must preserve every prior event');
  const [checkpoint] = committed;
  assert.ok(checkpoint, 'the reported committed checkpoint must actually exist in the ledger');
  assert.ok(diagnostic.includes(`event ${checkpoint.id} committed`),
    'the refusal must name the exact event persisted before the mirror failure');
  assert.deepEqual(committed, [checkpoint], 'one refused operation must not append duplicate checkpoints');
  assert.deepEqual(readFileSync(statePath), originalState);
  await prompt(session, '/checkpoint after-mirror-refusal');
  const recovered = exported(session.sessionPath, 'mirror-recovered');
  const final = recovered.findLast((event) => event.type === 'checkpoint');
  assert.equal(final?.data?.label, 'after-mirror-refusal');
  assert.equal(JSON.parse(readFileSync(statePath)).activeLeaf, final.id,
    'the next successful checkpoint must publish its actual active leaf');
  assert.deepEqual(recovered.find((event) => event.id === checkpoint.id), checkpoint,
    'recovery must not remove the already committed event');
  report.cases.push({ name: 'active-leaf-mirror-refusal', verdict: 'passed', statePath,
    committedEvent: checkpoint.id, recoveredEvent: final.id, diagnostic,
    cause: refusal.message, errno: refusal.errno });
}
