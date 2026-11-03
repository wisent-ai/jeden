import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { startRpc } from '../rpc.mjs';

export async function rpcRefusal({ binary, cwd, env, report, model, maxSteps, prompt }) {
  const steps = Number(maxSteps);
  assert.ok(Number.isSafeInteger(steps), 'JEDEN_TEST_MAX_STEPS must be an integer');
  const client = startRpc({ binary, cwd, env, report });
  let result;
  const failures = [];
  try {
    const session = await client.rpc('session/new', {
      options: { cwd, model, maxSteps: steps, allowCommand: true },
    });
    let error;
    const requestId = randomUUID();
    let response;
    try {
      response = await client.rpc('session/prompt', {
        sessionId: session.sessionId, requestId, prompt,
      });
    } catch (failure) {
      error = failure;
    }
    const events = report.frames.filter(({ frame }) =>
      frame.method === 'session/event' && frame.params?.requestId === requestId)
      .map(({ frame }) => frame.params);
    result = { error, response, session, requestId, events };
    await client.rpc('shutdown', {});
  } catch (error) {
    failures.push(error);
  } finally {
    try {
      const command = await client.end();
      assert.ok(command.status !== null && !command.status,
        `RPC process did not shut down successfully: ${JSON.stringify(command)}`);
    } catch (error) {
      failures.push(error);
    }
  }
  if (failures.length) {
    if (result?.error) failures.unshift(result.error);
    throw new AggregateError(failures, `RPC refusal qualification failed: ${failures.map(String).join('; ')}`);
  }
  return result;
}
