// Automatic post-answer compaction refusals through the real Jeden RPC and a
// real SessionStart hook process (compaction-hook.mjs); no model, provider or
// transport substitute. The caller states the compaction threshold, so the
// first answer of each session crosses it and the threshold compaction runs.
//
// automatic-compaction-refusal: the hook refuses the compact restoration; the
// answer reports auto-compaction (threshold) with the hook's cause, the
// failure is recorded as auto_compaction_error, and the still-pending
// restoration refuses the next request without another model action.
//
// automatic-compaction-recording-refusal: the hook also takes write
// permission off the session's transcript before refusing, so recording the
// failure is genuinely refused; the answer must carry both causes, the
// transcript path and the measured OS error, and the transcript must hold
// exactly the bytes it held when the hook ran.
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startRpc } from '../rpc.mjs';

const hook = join(dirname(fileURLToPath(import.meta.url)), 'hook.mjs');
const quote = (text) => `'${text.replaceAll("'", "'\\''")}'`;

export async function exerciseAutoCompaction({ binary, model, maxSteps, output, env, report, exported }) {
  const threshold = process.env.JEDEN_TEST_COMPACTION_THRESHOLD;
  assert.ok(threshold?.trim(),
    'JEDEN_TEST_COMPACTION_THRESHOLD is required: the token count past which this journey makes Jeden compact after an answer');
  for (const lockTranscript of [false, true]) {
    const name = lockTranscript ? 'automatic-compaction-recording-refusal' : 'automatic-compaction-refusal';
    const workspace = join(output, `${name}-workspace`);
    mkdirSync(join(workspace, '.jeden'), { recursive: true });
    const marker = `restoration refused by ${name} ${randomUUID()}`;
    const control = join(output, `${name}-control.json`);
    writeFileSync(control, JSON.stringify({
      refuseSource: 'compact', lockTranscript, sessionRoot: env.JEDEN_SESSION_ROOT, marker,
    }));
    writeFileSync(join(workspace, '.jeden/hooks.json'), JSON.stringify({ hooks: {
      SessionStart: [{ command: `exec ${quote(process.execPath)} ${quote(hook)} ${quote(control)}` }],
    } }));
    const evidence = { name, verdict: 'pending', workspace, control, marker, threshold };
    report.cases.push(evidence);
    const client = startRpc({ binary, cwd: workspace, env: { ...env, JEDEN_COMPACTION_THRESHOLD: threshold }, report });
    const failures = [];
    try {
      const options = { cwd: workspace, model, maxSteps, allowCommand: true };
      const session = await client.rpc('session/new', { options });
      evidence.sessionPath = session.sessionPath;
      const prompt = (text) => client.rpc('session/prompt', { sessionId: session.sessionId, prompt: text });
      await assert.rejects(prompt('Answer in one sentence: what is this workspace for? Do not edit files.'), (error) => {
        evidence.error = error.message;
        return error.message.includes('auto-compaction (threshold) failed')
          && error.message.includes('SessionStart (compact)') && error.message.includes(marker);
      }, 'an answer past the compaction threshold must report the failed instruction restoration and its cause');
      assert.ok(existsSync(`${control}.startup.payload.json`), 'startup restoration must have run through the same hook');
      if (lockTranscript) {
        const lock = JSON.parse(readFileSync(`${control}.lock.json`, 'utf8'));
        const cause = JSON.parse(readFileSync(`${control}.cause.json`, 'utf8'));
        evidence.cause = cause;
        assert.ok(evidence.error.includes('recording the failure also failed'), evidence.error);
        assert.ok(evidence.error.includes(lock.path), `the recording refusal must name ${lock.path}: ${evidence.error}`);
        assert.ok(evidence.error.includes(`os error ${Math.abs(cause.errno)}`), evidence.error);
        assert.deepEqual(readFileSync(lock.path), readFileSync(`${control}.before`),
          'a refused recording changed the transcript');
      } else {
        const events = exported(session.sessionPath, name);
        assert.ok(events.some((event) => event.type === 'auto_compaction_error' && String(event.data.error).includes(marker)),
          'the failed automatic compaction must be recorded with its cause');
        const actions = events.filter((event) => event.type === 'action').length;
        await assert.rejects(prompt('Continue after the failed compaction.'),
          (error) => error.message.includes('SessionStart (compact)') && error.message.includes(marker),
          'the pending restoration must refuse the next request');
        const after = exported(session.sessionPath, `${name}-after`);
        assert.equal(after.filter((event) => event.type === 'action').length, actions,
          'a model action ran after the instruction restoration failed');
      }
    } catch (error) {
      failures.push(error);
      evidence.failure = String(error);
    } finally {
      try {
        if (existsSync(`${control}.lock.json`)) {
          const lock = JSON.parse(readFileSync(`${control}.lock.json`, 'utf8'));
          chmodSync(lock.path, lock.mode);
        }
      } catch (error) {
        failures.push(error);
        evidence.restorationFailure = String(error);
      } finally {
        try {
          await client.end();
        } catch (error) {
          failures.push(error);
          evidence.shutdownFailure = String(error);
        }
      }
    }
    if (failures.length) {
      evidence.verdict = 'failed';
      throw new AggregateError(failures, `${name}: ${failures.map(String).join('; ')}`);
    }
    evidence.verdict = 'passed';
  }
}
