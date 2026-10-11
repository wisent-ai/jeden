// Real CLI collaboration and mode state: an absent state file is no session,
// while an unreadable, malformed or non-object one is refused naming the file,
// before any relay event is written and without changing either file.
// Run with JEDEN_BIN and JEDEN_TEST_SOURCE_REVISION naming the candidate.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = resolve(required('JEDEN_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const runs = join(root, '.build/collab-state');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const home = join(output, 'home');
const project = join(output, 'project');
const state = join(project, '.jeden/collab.json');
const relay = join(project, '.jeden/collab-relay.jsonl');
const mode = join(project, '.jeden/mode-state.json');
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary),
  testSha256: hash(fileURLToPath(import.meta.url)), commands: [], verdict: 'failed' };
const env = { ...process.env, HOME: home };

function invoke(command) {
  const args = ['run', command, '--json', '--cwd', project];
  const result = spawnSync(binary, args, { cwd: project, env, encoding: 'utf8' });
  report.commands.push({ binary, args, cwd: project, home, status: result.status,
    signal: result.signal, stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.signal, null, `${command} must not terminate by signal`);
  return result;
}
function answered(command) {
  const result = invoke(command);
  assert.ok(result.status !== null && !result.status, `${command} failed: ${result.stderr}`);
  return JSON.parse(result.stdout).text;
}
function refused(command, path, cause) {
  const kept = [state, relay, mode].filter(existsSync).map((file) => [file, readFileSync(file)]);
  const result = invoke(command);
  assert.ok(result.status !== null && result.status, `${command} was not refused`);
  assert.ok(result.stderr.includes(path), `${command} refusal does not name ${path}: ${result.stderr}`);
  if (cause) assert.ok(result.stderr.includes(cause), `${command} refusal lacks "${cause}": ${result.stderr}`);
  for (const [file, bytes] of kept) {
    assert.deepEqual(readFileSync(file), bytes, `${command} changed ${file} while refusing`);
  }
}

try {
  mkdirSync(home, { recursive: true });
  mkdirSync(dirname(state), { recursive: true });

  assert.match(answered('/collab status'), /Collab off/, 'absent state must read as no session');
  answered('/collab start');
  const started = JSON.parse(readFileSync(state, 'utf8'));
  assert.equal(started.host.relayFile, relay, 'start did not record the default file relay');
  assert.ok(existsSync(relay), 'start did not write the relay file');
  const validState = readFileSync(state);
  const validRelay = readFileSync(relay);

  appendFileSync(relay, 'this is not an event\n');
  const viewed = answered('/collab view');
  assert.ok(viewed.includes(relay) && viewed.includes('not valid JSON'),
    `a malformed relay event was not named: ${viewed}`);
  writeFileSync(relay, validRelay);

  writeFileSync(state, '{"host":');
  for (const command of ['/collab status', '/collab stop', '/leave', `/join ${relay}`]) {
    refused(command, state, 'not valid JSON');
  }
  writeFileSync(state, '[]');
  refused('/collab status', state, 'does not hold a JSON object');
  rmSync(state);
  mkdirSync(state);
  refused('/collab stop', state, 'cannot be read');
  rmSync(state, { recursive: true });

  writeFileSync(state, validState);
  assert.match(answered('/collab stop'), /Collab hosting stopped/);
  assert.equal(JSON.parse(readFileSync(state, 'utf8')).host, null, 'stop did not clear the host');

  writeFileSync(mode, '{"lastSessionPath":');
  refused('/dump', mode, 'not valid JSON');
  // One field of the wrong type is the whole state refused, never every mode
  // turned off and then written back as defaults by the next mode change.
  writeFileSync(mode, JSON.stringify({ plan: { enabled: 'yes' }, goal: { objective: 'kept' } }));
  refused('/roles', mode, 'is not a valid mode state');
  refused('/plan on', mode, 'is not a valid mode state');
  writeFileSync(mode, '[]');
  refused('/roles', mode, 'does not hold a JSON object');
  rmSync(mode);
  const absent = invoke('/dump');
  assert.ok(absent.status !== null && absent.status, '/dump without a session was not refused');
  assert.ok(absent.stderr.includes('No current Rust session is recorded yet'),
    `an absent mode state must say no session is recorded: ${absent.stderr}`);
  assert.ok(!absent.stderr.includes(mode), 'an absent mode state was reported as a broken file');
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
