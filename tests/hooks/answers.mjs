// Runs the real Jeden CLI, model route, extension host and hook processes.
// Required: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_MODEL,
// JEDEN_TEST_MAX_STEPS, and the normal Brama environment. No build is performed.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { rpcRefusal } from './refusal-rpc.mjs';

function required(name) {
  const value = process.env[name];
  if (!value?.trim()) throw new Error(`${name} is required`);
  return value;
}
const binary = resolve(required('JEDEN_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const model = required('JEDEN_TEST_MODEL');
const maxSteps = required('JEDEN_TEST_MAX_STEPS');
assert.ok(!process.env.JEDEN_MODEL_ENDPOINT, 'qualification requires the real Stado-selected Brama route');
assert.ok(!process.env.BRAMA_URL, 'an endpoint override must not replace the directory fixture');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const fixture = join(root, 'tests/hooks/fixture.mjs');
const runs = join(root, '.build/hook-journeys');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = {
  startedAt: new Date().toISOString(), binary, declaredSourceRevision: revision,
  binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  fixtureSha256: hash(fixture), model, commands: [], frames: [], cases: [], verdict: 'failed',
};
const quote = (text) => `'${text.replaceAll("'", "'\\''")}'`;
function invoke(program, args, cwd, env) {
  const result = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, args, cwd, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  return result;
}
function succeeds(result) {
  assert.ok(result.status !== null && !result.status, result.stderr);
  return result.stdout;
}

try {
  report.checkoutRevision = succeeds(invoke('git', ['rev-parse', 'HEAD'], root, process.env)).trim();
  assert.equal(report.checkoutRevision, revision, 'run the journey from the declared candidate revision');
  assert.equal(succeeds(invoke('git', ['status', '--porcelain'], root, process.env)).trim(), '',
    'commit the candidate and tests before qualification');
  report.binaryVersion = succeeds(invoke(binary, ['--version'], root, process.env));
  for (const [mode, expectedAction, failedOperation, transport] of [
    ['intermediate', 'message', null, 'cli'], ['final', 'final', null, 'cli'],
    ['extension', 'final', null, 'cli'], ['signal', 'final', null, 'cli'],
    ['positive', 'final', null, 'cli'],
    ['recording', 'message', 'recording stop-hook violation', 'cli'],
    ['recording', 'final', 'recording stop-hook violation', 'cli'],
    ['completion-state', 'message', 'reading completion state', 'cli'],
    ['completion-state', 'final', 'reading completion state', 'cli'],
    ['recording', 'message', 'recording stop-hook violation', 'rpc'],
    ['recording', 'final', 'recording stop-hook violation', 'rpc'],
    ['completion-state', 'message', 'reading completion state', 'rpc'],
    ['completion-state', 'final', 'reading completion state', 'rpc'],
    ['open-requests', 'message', 'reading open completion requests', 'cli'],
    ['open-requests', 'final', 'reading open completion requests', 'cli'],
    ['open-requests', 'message', 'reading open completion requests', 'rpc'],
    ['open-requests', 'final', 'reading open completion requests', 'rpc'],
  ]) {
    const directory = join(output, `${transport}-${mode}-${expectedAction}`);
    const workspace = join(directory, 'workspace');
    const config = join(workspace, '.jeden');
    mkdirSync(config, { recursive: true });
    const receipt = join(directory, 'hook-inputs.jsonl');
    writeFileSync(receipt, '');
    const rejected = `withheld-${randomUUID()}`;
    const accepted = `accepted-${randomUUID()}`;
    const env = { ...process.env, JEDEN_SESSION_ROOT: join(directory, 'sessions'),
      JEDEN_TEST_HOOK_MODE: mode, JEDEN_TEST_HOOK_RECEIPT: receipt,
      JEDEN_TEST_REJECTED_TEXT: rejected, JEDEN_TEST_ACCEPTED_TEXT: accepted };
    if (mode === 'extension') {
      const extensions = join(config, 'extensions');
      mkdirSync(extensions);
      copyFileSync(fixture, join(extensions, 'qualification.mjs'));
    } else {
      writeFileSync(join(config, 'hooks.json'), JSON.stringify({ hooks: {
        Stop: [{ command: `exec ${quote(process.execPath)} ${quote(fixture)}` }],
      } }));
    }
    const instruction = expectedAction === 'message'
      ? `First send a native intermediate message action with text exactly ${rejected}, then finish with ${accepted}.`
      : `Finish with text exactly ${rejected}.`;
    const task = `This is a real native hook qualification. Do not modify files or run tools. ${instruction} If a hook refuses or fails, continue with a final answer containing ${accepted} instead. Never repeat refused text after a refusal.`;
    let result;
    const failures = [];
    try {
      if (transport === 'rpc') {
        result = await rpcRefusal({ binary, cwd: workspace, env, report, model, maxSteps, prompt: task });
      } else {
        result = invoke(binary, ['run', task, '--cwd', workspace, '--model', model,
          '--max-steps', maxSteps, '--allow-command', '--json'], workspace, env);
      }
    } catch (error) {
      failures.push(error);
    } finally {
      try {
        const permissionReceipt = `${receipt}.recording.json`;
        if (existsSync(permissionReceipt)) {
          const { path, mode: originalMode } = JSON.parse(readFileSync(permissionReceipt, 'utf8'));
          chmodSync(path, originalMode);
        }
      } catch (error) {
        failures.push(error);
      }
    }
    if (failures.length) {
      if (result?.error) failures.unshift(result.error);
      throw new AggregateError(failures, `hook qualification failed: ${failures.map(String).join('; ')}`);
    }
    const inputs = readFileSync(receipt, 'utf8').split('\n').filter(Boolean).map(JSON.parse);
    const candidate = inputs.find((input) => input.last_assistant_message?.includes(rejected));
    assert.ok(candidate, 'the real model must reach the requested hook candidate, not an easier path');
    const session = dirname(candidate.transcript_path);
    const exported = invoke(binary, ['export', session], workspace, env);
    const events = JSON.parse(succeeds(exported)).events;
    writeFileSync(join(directory, 'session-export.json'), exported.stdout);
    const actions = events.filter((event) => event.type === 'action').map((event) => event.data.action);
    assert.ok(actions.some((action) => action.action === expectedAction && action.text?.includes(rejected)),
      'a real action of the requested kind must have been produced');
    if (failedOperation) {
      const failure = JSON.parse(readFileSync(`${receipt}.recording.json`, 'utf8'));
      const cause = JSON.parse(readFileSync(`${receipt}.cause.json`, 'utf8'));
      let visible;
      if (transport === 'rpc') {
        assert.ok(result.error instanceof Error, 'failed refusal handling must reject the RPC prompt');
        assert.equal(result.session.sessionPath, session, 'the refusal must belong to the requested session');
        visible = result.error.message;
        const notification = result.events.find((event) => event.kind === 'error');
        assert.ok(notification && visible.includes(notification.message),
          'the RPC error notification must retain the same failure as the prompt response');
        assert.ok(notification.message.includes(failure.reason)
          && notification.message.includes(failedOperation)
          && notification.message.includes(failure.path)
          && notification.message.includes(`os error ${Math.abs(cause.errno)}`),
          'the graphical-client event lost part of the actual cause chain');
      } else {
        assert.ok(result.status !== null && result.status, 'failed refusal persistence must fail the turn');
        visible = `${result.stdout}\n${result.stderr}`;
      }
      assert.ok(visible.includes(failure.reason), 'the initiating Stop reason was lost');
      assert.ok(visible.includes(failedOperation), 'the failed operation was lost');
      assert.ok(visible.includes(failure.path), 'the refused file path was lost');
      assert.ok(Number.isInteger(cause.errno), 'the actual filesystem refusal must expose an OS error');
      assert.ok(visible.includes(`os error ${Math.abs(cause.errno)}`), 'the actual OS cause was lost');
      assert.ok(!visible.includes(rejected), 'refused candidate text reached the caller');
      assert.deepEqual(readFileSync(failure.path), readFileSync(`${receipt}.before`),
        'failed refusal handling must not change the affected file');
      assert.ok(!events.some((event) => (event.type === 'assistant_message' || event.type === 'final')
        && event.data.text.includes(rejected)), 'the refused candidate was published');
      report.cases.push({ mode, expectedAction, failedOperation, transport, session, failure, cause, verdict: 'passed' });
      continue;
    }
    succeeds(result);
    const visible = `${result.stdout}\n${result.stderr}`;
    if (mode === 'positive') {
      assert.ok(visible.includes(rejected), 'a positive nonblocking hook exit must preserve the answer');
      assert.ok(events.some((event) => event.type === 'final' && event.data.text.includes(rejected)));
    } else {
      assert.ok(!visible.includes(rejected), 'refused text reached a CLI output channel');
      assert.ok(visible.includes(accepted), 'the native loop did not continue to the accepted answer');
      assert.ok(inputs.some((input) => input.stop_hook_active && input.last_assistant_message.includes(accepted)));
      assert.ok(!events.some((event) => (event.type === 'assistant_message' || event.type === 'final')
        && event.data.text.includes(rejected)), 'refused text was persisted as published');
      assert.ok(events.some((event) => event.data?.rule === 'stop-hook'), 'no retained Stop refusal');
      if (mode === 'extension') assert.ok(events.some((event) =>
        event.data?.message?.includes('extension hook dispatch failed')));
      if (mode === 'signal') assert.ok(events.some((event) =>
        event.data?.message?.includes('ended without an exit code')));
    }
    report.cases.push({ mode, transport, session, rejected, accepted, verdict: 'passed' });
  }
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  report.finishedAt = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
