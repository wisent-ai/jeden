// Real RPC session transitions and filesystem refusals; called by the lifecycle journey.
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { chmodSync, closeSync, constants, mkdirSync, openSync, readFileSync, realpathSync, renameSync, rmdirSync, statSync, unlinkSync, writeFileSync } from 'node:fs';
import { join, relative } from 'node:path';

export async function exerciseMove({ rpc, prompt, options, output, exported, report }) {
  const session = await rpc('session/new', { options });
  await prompt(session, '/checkpoint before-move');
  const statePath = join(session.sessionPath, 'state.json');
  const originalState = readFileSync(statePath);
  const before = exported(session.sessionPath, 'move-before');
  const context = (events) => events.findLast((event) => event.type === 'checkpoint').data.messages;
  const originalContext = context(before);
  const destination = join(output, `workspace with spaces ${randomUUID()}`);
  const child = join(destination, randomUUID());
  mkdirSync(child, { recursive: true });
  const savedState = join(session.sessionPath, `saved-state-${randomUUID()}.json`);
  renameSync(statePath, savedState);
  let obstructionCreated = false;
  try {
    mkdirSync(statePath);
    obstructionCreated = true;
    await assert.rejects(prompt(session, `/move ${destination}`), (error) =>
      error.message.includes(statePath) && error.message.includes('not a regular state file'));
    assert.deepEqual(readFileSync(savedState), originalState, 'refusal must not alter the saved state');
  } finally {
    if (obstructionCreated) rmdirSync(statePath);
    renameSync(savedState, statePath);
  }
  await prompt(session, '/checkpoint after-refused-move');
  assert.deepEqual(context(exported(session.sessionPath, 'move-refused')), originalContext,
    'a refused move must preserve the model-visible context');
  assert.equal(JSON.parse(readFileSync(statePath)).cwd, JSON.parse(originalState).cwd);
  await prompt(session, '/move .');
  assert.equal(JSON.parse(readFileSync(statePath)).cwd, JSON.parse(originalState).cwd,
    'a refused move must preserve the active directory used by the next relative move');
  report.cases.push({ name: 'move-state-refusal', verdict: 'passed', statePath, destination });

  const mode = statSync(session.sessionPath).mode;
  const beforeStaging = readFileSync(statePath);
  const probe = join(session.sessionPath, `create-probe-${randomUUID()}`);
  let refusal;
  try {
    chmodSync(session.sessionPath, mode & ~(constants.S_IWUSR | constants.S_IWGRP | constants.S_IWOTH));
    try {
      closeSync(openSync(probe, 'wx'));
      unlinkSync(probe);
    } catch (error) {
      refusal = error;
    }
    assert.ok(refusal?.errno, 'the host must enforce the actual staging-file creation refusal');
    await assert.rejects(prompt(session, `/move ${destination}`), (error) =>
      error.message.includes('cannot create session state staging file')
        && error.message.includes(session.sessionPath)
        && error.message.includes(`os error ${Math.abs(refusal.errno)}`));
    assert.deepEqual(readFileSync(statePath), beforeStaging,
      'failed staging must preserve the exact prior state file');
  } finally {
    chmodSync(session.sessionPath, mode);
  }
  await prompt(session, '/checkpoint after-staging-refusal');
  assert.deepEqual(context(exported(session.sessionPath, 'move-staging-refused')), originalContext);
  await prompt(session, '/move .');
  assert.equal(JSON.parse(readFileSync(statePath)).cwd, JSON.parse(beforeStaging).cwd,
    'staging refusal must preserve the active directory used by the next relative move');
  report.cases.push({ name: 'move-staging-refusal', verdict: 'passed', statePath,
    cause: refusal.message, errno: refusal.errno });

  const stateMode = statSync(statePath).mode;
  const beforeRead = readFileSync(statePath);
  let readRefusal;
  try {
    chmodSync(statePath, stateMode & ~(constants.S_IRUSR | constants.S_IRGRP | constants.S_IROTH));
    try {
      readFileSync(statePath);
    } catch (error) {
      readRefusal = error;
    }
    assert.ok(readRefusal?.errno, 'the host must enforce the actual state-file read refusal');
    await assert.rejects(prompt(session, `/move ${destination}`), (error) =>
      error.message.includes('cannot read session state')
        && error.message.includes(statePath)
        && error.message.includes(`os error ${Math.abs(readRefusal.errno)}`));
  } finally {
    chmodSync(statePath, stateMode);
  }
  assert.deepEqual(readFileSync(statePath), beforeRead,
    'failed state read must preserve the exact prior state file');
  await prompt(session, '/checkpoint after-state-read-refusal');
  assert.deepEqual(context(exported(session.sessionPath, 'move-state-read-refused')), originalContext,
    'state read refusal must preserve the live model context');
  await prompt(session, '/move .');
  assert.equal(JSON.parse(readFileSync(statePath)).cwd, JSON.parse(beforeRead).cwd,
    'state read refusal must preserve the active directory used by the next relative move');
  report.cases.push({ name: 'move-state-read-refusal', verdict: 'passed', statePath,
    cause: readRefusal.message, errno: readRefusal.errno });

  for (const scenario of [
    { name: 'move-invalid-json', contents: '{', cause: 'EOF while parsing an object' },
    { name: 'move-invalid-state-shape', contents: '[]', cause: 'expected object' },
  ]) {
    const retainedState = readFileSync(statePath);
    try {
      writeFileSync(statePath, scenario.contents);
      await assert.rejects(prompt(session, `/move ${destination}`), (error) =>
        error.message.includes(`invalid ${statePath}`)
          && error.message.includes(scenario.cause));
      assert.equal(readFileSync(statePath, 'utf8'), scenario.contents,
        'a refused move must not replace malformed state with a newly invented state');
    } finally {
      writeFileSync(statePath, retainedState);
    }
    await prompt(session, `/checkpoint after-${scenario.name}`);
    assert.deepEqual(context(exported(session.sessionPath, scenario.name)), originalContext,
      'state decoding refusals must preserve the live model context');
    assert.equal(JSON.parse(readFileSync(statePath)).cwd, JSON.parse(retainedState).cwd);
    await prompt(session, '/move .');
    assert.equal(JSON.parse(readFileSync(statePath)).cwd, JSON.parse(retainedState).cwd,
      'state decoding refusal must preserve the active directory used by the next relative move');
    report.cases.push({ name: scenario.name, verdict: 'passed', statePath,
      cause: scenario.cause, destination });
  }

  await assert.rejects(prompt(session, '/move'), /Usage: \/move <directory>/);
  await assert.rejects(prompt(session, `/move ${statePath}`), (error) =>
    error.message.includes('not a directory') && error.message.includes(statePath));
  const absent = join(destination, randomUUID());
  await assert.rejects(prompt(session, `/move ${absent}`), (error) =>
    error.message.includes('cannot move to') && error.message.includes(absent));
  assert.equal(JSON.parse(readFileSync(statePath)).cwd, JSON.parse(originalState).cwd);

  const destinationContext = join(destination, '.jeden');
  mkdirSync(destinationContext);
  const cyclicContext = join(destinationContext, 'context.md');
  const beforeContextFailure = readFileSync(statePath);
  writeFileSync(cyclicContext, '@./context.md\n');
  try {
    await assert.rejects(prompt(session, `/move ${destination}`), (error) =>
      error.message.includes('context import cycle')
        && error.message.includes(realpathSync(cyclicContext)));
    assert.deepEqual(readFileSync(statePath), beforeContextFailure,
      'failed context preparation must precede any working-directory state publication');
  } finally {
    unlinkSync(cyclicContext);
  }
  await prompt(session, '/checkpoint after-context-refusal');
  assert.deepEqual(context(exported(session.sessionPath, 'move-context-refused')), originalContext,
    'failed destination context preparation must preserve the prior model window');
  await prompt(session, '/move .');
  assert.equal(JSON.parse(readFileSync(statePath)).cwd, JSON.parse(beforeContextFailure).cwd,
    'context preparation refusal must preserve the active directory');
  report.cases.push({ name: 'move-context-cycle-refusal', verdict: 'passed', cyclicContext, statePath });

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
