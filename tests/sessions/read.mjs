// Actual CLI/RPC ledger reads against an isolated session, with OS-enforced refusal.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, constants, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

export async function exerciseRead({ binary, env, rpc, prompt, options, exported, report }) {
  const session = await rpc('session/new', { options });
  await prompt(session, '/checkpoint before-read-refusal');
  const transcript = join(session.sessionPath, 'transcript.jsonl');
  const state = join(session.sessionPath, 'state.json');
  const beforeTranscript = readFileSync(transcript);
  const beforeState = readFileSync(state);
  const mode = statSync(transcript).mode;
  let refusal;
  try {
    chmodSync(transcript, mode & ~(constants.S_IRUSR | constants.S_IRGRP | constants.S_IROTH));
    try { readFileSync(transcript); } catch (error) { refusal = error; }
    assert.ok(refusal?.errno, 'the host must enforce the actual transcript read refusal');
    const identifiesFailure = (message) => message.includes(`cannot read ${transcript}`)
      && message.includes(`os error ${Math.abs(refusal.errno)}`);
    await assert.rejects(prompt(session, '/checkpoint refused-read'),
      (error) => identifiesFailure(error.message));
    for (const command of ['show', 'export']) {
      const args = [command, session.sessionPath];
      const result = spawnSync(binary, args, { cwd: options.cwd, env, encoding: 'utf8' });
      report.commands.push({ program: binary, args, cwd: options.cwd, status: result.status,
        signal: result.signal, stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
      assert.ifError(result.error);
      assert.equal(result.signal, null);
      assert.ok(result.status !== null && result.status, `${command} must refuse an unreadable ledger`);
      assert.ok(identifiesFailure(`${result.stdout}\n${result.stderr}`));
    }
    assert.deepEqual(readFileSync(state), beforeState,
      'a refused ledger read must not publish a new active leaf');
  } finally {
    chmodSync(transcript, mode);
  }
  assert.deepEqual(readFileSync(transcript), beforeTranscript,
    'neither CLI nor RPC may append after failing to read the prior ledger');
  assert.deepEqual(readFileSync(state), beforeState);
  await prompt(session, '/checkpoint after-read-refusal');
  const events = exported(session.sessionPath, 'read-refusal-recovered');
  assert.ok(events.some((event) => event.type === 'checkpoint'
    && event.data?.label === 'before-read-refusal'));
  assert.ok(!events.some((event) => event.type === 'checkpoint'
    && event.data?.label === 'refused-read'));
  assert.equal(events.findLast((event) => event.type === 'checkpoint')?.data?.label,
    'after-read-refusal', 'the same live session must persist a checkpoint once reading is restored');
  report.cases.push({ name: 'transcript-read-refusal', verdict: 'passed', transcript,
    cause: refusal.message, errno: refusal.errno, sessionPath: session.sessionPath });
}
