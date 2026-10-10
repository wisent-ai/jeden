// Real Jeden/Stado/Brama refusal journey. Requires an already-observed failing
// resolver route; never creates a fake provider, changes credentials or stops it.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { basename, delimiter, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} must identify the real transport fixture`);
  return value;
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = resolve(required('JEDEN_BIN'));
const stado = resolve(required('STADO_BIN'));
assert.equal(basename(stado), 'stado', 'the dependency must be discoverable by its real CLI name');
const dependencyPath = `${dirname(stado)}${delimiter}${required('PATH')}`;
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const model = required('JEDEN_TEST_MODEL');
const cause = required('JEDEN_TEST_TRANSPORT_CAUSE');
const promptPath = resolve(required('JEDEN_TEST_PROMPT_FILE'));
assert.ok(!process.env.JEDEN_MODEL_ENDPOINT, 'this journey requires the real Stado route, not direct-provider mode');
assert.ok(!process.env.BRAMA_URL, 'an endpoint override must not replace the directory fixture');
const area = join(root, '.build/model-transport', randomUUID());
const workspace = join(area, 'workspace');
const sessions = join(area, 'sessions');
mkdirSync(workspace, { recursive: true });
mkdirSync(sessions, { recursive: true });
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = {
  revision, binary, binarySha256: hash(binary), stadoSha256: hash(stado),
  testSha256: hash(fileURLToPath(import.meta.url)), promptSha256: hash(promptPath),
  model, cause, sessions, commands: [], verdict: 'failed',
};
function invoke(program, args, env = process.env) {
  const result = spawnSync(program, args, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ program, args, cwd: root, status: result.status,
    signal: result.signal, error: result.error?.message, stdout: result.stdout, stderr: result.stderr });
  writeFileSync(join(area, 'report.json'), JSON.stringify(report, null, '  '));
  assert.ifError(result.error);
  assert.equal(result.signal, null, 'the journey must observe a product refusal, not an interruption');
  return result;
}
function succeeds(result) {
  assert.ok(result.status !== null && !result.status, result.stderr);
  return result.stdout;
}
function transcripts(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? transcripts(path) : entry.name === 'transcript.jsonl' ? [path] : [];
  });
}
try {
  assert.equal(succeeds(invoke('git', ['rev-parse', 'HEAD'])).trim(), revision);
  succeeds(invoke('git', ['diff', '--exit-code', 'HEAD']));
  const route = JSON.parse(succeeds(invoke(stado, [
    'service', 'directory', 'connect', 'brama', '--consumer', 'jeden', '--no-verify', '--json',
  ])));
  assert.equal(route.service, 'brama');
  assert.equal(route.consumer, 'jeden');
  assert.ok(route.url, 'the real directory must name the observed adapter');
  report.route = route;
  // Status is retained even when its unhealthy exit is the expected fixture state.
  invoke(stado, ['resolver', 'status', '--json']);
  const env = { ...process.env, PATH: dependencyPath, JEDEN_SESSION_ROOT: sessions };
  const result = invoke(binary, [
    'run', readFileSync(promptPath, 'utf8'), '--model-only', '--json',
    '--cwd', workspace, '--model', model,
  ], env);
  assert.ok(result.status !== null && result.status, 'the real model request must refuse');
  const diagnostic = `${result.stdout}\n${result.stderr}`;
  assert.ok(diagnostic.includes(`${route.url.replace(/\/$/, '')}/v1/chat/completions`), diagnostic);
  assert.ok(diagnostic.includes('Brama') && diagnostic.includes('Stado'), diagnostic);
  assert.ok(diagnostic.includes(cause), 'the original observed transport cause must remain visible');
  const paths = transcripts(sessions);
  const events = paths.flatMap((path) => {
    const exported = succeeds(invoke(binary, ['export', dirname(path)], env));
    writeFileSync(join(dirname(path), 'qualification-export.json'), exported);
    return JSON.parse(exported).events;
  });
  assert.ok(events.some((event) => event.type === 'run_error' && event.data?.message?.includes(cause)),
    'the actual refusal must remain in the durable session');
  assert.ok(!events.some((event) => event.type === 'final'), 'a transport refusal must not manufacture completion');
  invoke(stado, ['resolver', 'status', '--json']);
  report.transcripts = paths;
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  writeFileSync(join(area, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(area, 'report.json')}`);
}
