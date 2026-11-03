// Actual CLI/RPC ledger reads against an isolated session, with OS-enforced refusal.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, constants, existsSync, readFileSync, statSync } from 'node:fs';
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
  const failures = [];
  try {
    chmodSync(transcript, mode & ~(constants.S_IRUSR | constants.S_IRGRP | constants.S_IROTH));
    try { readFileSync(transcript); } catch (error) { refusal = error; }
    assert.ok(refusal?.errno, 'the host must enforce the actual transcript read refusal');
    const identifiesFailure = (message) => message.includes(`cannot read ${transcript}`)
      && message.includes(`os error ${Math.abs(refusal.errno)}`);
    await assert.rejects(prompt(session, '/checkpoint refused-read'),
      (error) => identifiesFailure(error.message));
    const refusedExport = join(session.sessionPath, 'read-refused-export.json');
    for (const command of ['show', 'export']) {
      const args = command === 'export'
        ? [command, session.sessionPath, refusedExport] : [command, session.sessionPath];
      const result = spawnSync(binary, args, { cwd: options.cwd, env, encoding: 'utf8' });
      report.commands.push({ program: binary, args, cwd: options.cwd, status: result.status,
        signal: result.signal, stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
      assert.ifError(result.error);
      assert.equal(result.signal, null);
      assert.ok(result.status !== null && result.status, `${command} must refuse an unreadable ledger`);
      assert.ok(identifiesFailure(`${result.stdout}\n${result.stderr}`));
    }
    assert.ok(!existsSync(refusedExport), 'the refused export must not write output');
    assert.deepEqual(readFileSync(state), beforeState,
      'a refused ledger read must not publish a new active leaf');
  } catch (error) {
    failures.push(error);
  } finally {
    try { chmodSync(transcript, mode); } catch (error) { failures.push(error); }
  }
  if (failures.length) {
    throw new AggregateError(failures, `transcript read qualification failed: ${failures.map(String).join('; ')}`);
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
  const output = join(session.sessionPath, 'export-argument-check.html');
  for (const extra of [
    [],
    [output, '-x'],
    [output, '--json'],
    [output, '--html', '--markdown'],
    [output, '--html', '--html'],
    [output, 'another-output'],
  ]) {
    const args = ['export', session.sessionPath, ...extra];
    const result = spawnSync(binary, args, { cwd: options.cwd, env, encoding: 'utf8' });
    report.commands.push({ program: binary, args, cwd: options.cwd, status: result.status,
      signal: result.signal, stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
    assert.ifError(result.error);
    assert.ok(result.status !== null && result.status, `export accepted invalid arguments ${extra.join(' ')}`);
    assert.ok(`${result.stdout}\n${result.stderr}`.includes('usage: jeden export'),
      `export did not explain its argument refusal: ${result.stderr}`);
    assert.ok(!existsSync(output), 'an argument refusal must not write output');
  }
  const args = ['export', session.sessionPath, output, '--html'];
  const valid = spawnSync(binary, args, { cwd: options.cwd, env, encoding: 'utf8' });
  report.commands.push({ program: binary, args, cwd: options.cwd, status: valid.status,
    signal: valid.signal, stdout: valid.stdout, stderr: valid.stderr, error: valid.error?.message });
  assert.ifError(valid.error);
  assert.equal(valid.status, 0, valid.stderr);
  assert.ok(readFileSync(output, 'utf8').includes('after-read-refusal'),
    'the valid HTML export must contain the recovered checkpoint');
  report.cases.push({ name: 'transcript-read-refusal', verdict: 'passed', transcript,
    cause: refusal.message, errno: refusal.errno, sessionPath: session.sessionPath });
}
