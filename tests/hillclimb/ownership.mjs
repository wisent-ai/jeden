// Real-process ownership contention, not a fabricated lock or model provider.
// Pause only this journey's isolated owner, observe the kernel's stopped state,
// run the contender, then resume and collect the owner's actual round result.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { setImmediate } from 'node:timers/promises';

export async function duringOwnedClimb({ binary, args, cwd, env, ledger, report, probe }) {
  assert.ok(env, 'The owned climber requires the isolated qualification environment');
  const command = { program: binary, args, cwd, stdout: '', stderr: '' };
  report.commands.push(command);
  const child = spawn(binary, args, { cwd, env, stdio: ['ignore', 'pipe', 'pipe'] });
  let acknowledge;
  let refuse;
  const started = new Promise((resolve, reject) => { acknowledge = resolve; refuse = reject; });
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  child.stdout.on('data', (text) => { command.stdout += text; });
  child.stderr.on('data', (text) => {
    command.stderr += text;
    if (command.stderr.includes(`; ledger ${ledger} (`)) acknowledge();
  });
  child.once('error', (error) => {
    command.error = error.message;
    refuse(error);
  });
  const closed = new Promise((resolve) => child.once('close', (status, signal) => {
    command.exit_status = status;
    command.signal = signal;
    refuse(new Error(`The owner exited before its ledger was observed: ${command.stderr}`));
    resolve({ status, signal, stdout: command.stdout, stderr: command.stderr });
  }));
  const failures = [];
  let stopped = false;
  try {
    await started;
    stopped = child.kill('SIGSTOP');
    assert.ok(stopped, 'the actual owner must still exist when the contention observation begins');
    let observed = false;
    while (!observed) {
      const args = ['-o', 'state=', '-p', String(child.pid)];
      const state = spawnSync('ps', args, { encoding: 'utf8' });
      report.commands.push({ program: 'ps', args, status: state.status, stdout: state.stdout, stderr: state.stderr });
      assert.ok(state.status !== null && !state.status, state.stderr);
      observed = state.stdout.trim().startsWith('T');
      if (!observed) await setImmediate();
    }
    await probe();
  } catch (error) {
    failures.push(error);
    if (!stopped && child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
  } finally {
    if (stopped && !child.kill('SIGCONT')) failures.push(new Error('The isolated owner could not be resumed'));
  }
  const result = await closed;
  if (failures.length) throw new AggregateError(failures, 'The real concurrent-climber observation failed');
  return result;
}
