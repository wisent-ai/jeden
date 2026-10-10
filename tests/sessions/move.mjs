// Real RPC session transitions and filesystem refusals; called by the lifecycle journey.
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, realpathSync, renameSync, rmdirSync } from 'node:fs';
import { join, relative } from 'node:path';

export async function exerciseMove({ rpc, prompt, options, output, exported, report }) {
  const session = await rpc('session/new', { options });
  await prompt(session, '/checkpoint before-move');
  const statePath = join(session.sessionPath, 'state.json');
  const originalState = readFileSync(statePath);
  const before = exported(session.sessionPath, 'move-before');
  const context = (events) => events.findLast((event) => event.type === 'checkpoint').data.messages;
  const originalContext = context(before);
  const destination = join(output, `workspace-${randomUUID()}`);
  const child = join(destination, randomUUID());
  mkdirSync(child, { recursive: true });
  const savedState = join(session.sessionPath, `saved-state-${randomUUID()}.json`);
  renameSync(statePath, savedState);
  mkdirSync(statePath);
  try {
    await assert.rejects(prompt(session, `/move ${destination}`), (error) =>
      error.message.includes(statePath) && error.message.includes('not a regular state file'));
    assert.deepEqual(readFileSync(savedState), originalState, 'refusal must not alter the saved state');
  } finally {
    rmdirSync(statePath);
    renameSync(savedState, statePath);
  }
  await prompt(session, '/checkpoint after-refused-move');
  assert.deepEqual(context(exported(session.sessionPath, 'move-refused')), originalContext,
    'a refused move must preserve the model-visible context');
  assert.equal(JSON.parse(readFileSync(statePath)).cwd, JSON.parse(originalState).cwd);
  report.cases.push({ name: 'move-state-refusal', verdict: 'passed', statePath, destination });

  await assert.rejects(prompt(session, '/move'), /Usage: \/move <directory>/);
  await assert.rejects(prompt(session, `/move ${statePath}`), (error) =>
    error.message.includes('not a directory') && error.message.includes(statePath));
  const absent = join(destination, randomUUID());
  await assert.rejects(prompt(session, `/move ${absent}`), (error) =>
    error.message.includes('cannot move to') && error.message.includes(absent));
  assert.equal(JSON.parse(readFileSync(statePath)).cwd, JSON.parse(originalState).cwd);

  await prompt(session, `/move ${destination}`);
  assert.equal(JSON.parse(readFileSync(statePath)).cwd, realpathSync(destination));
  await prompt(session, `/move ${relative(destination, child)}`);
  assert.equal(JSON.parse(readFileSync(statePath)).cwd, realpathSync(child),
    'a subsequent relative move must start at the new active directory');
  const reset = await prompt(session, '/clear');
  const path = reset.sessionPath;
  assert.ok(path, 'reset result must identify the active session');
  assert.equal(JSON.parse(readFileSync(join(path, 'state.json'))).cwd, realpathSync(child),
    'SDK reset must use the moved working directory, not the original session options');
  report.cases.push({ name: 'move-and-reset', verdict: 'passed', source: session.sessionPath,
    sessionPath: path, cwd: realpathSync(child) });
}
