// Real CLI configuration layers, refusals, byte preservation and recovery.
// Run with JEDEN_BIN and JEDEN_TEST_SOURCE_REVISION naming the candidate.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync, rmSync, statSync } from 'node:fs';
import { constants } from 'node:os';
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
const runs = join(root, '.build/config-layers');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const home = join(output, 'home');
const project = join(output, 'project');
const legacy = join(home, '.jeden/config.json');
const current = join(home, '.jeden/config.yml');
const local = join(project, '.jeden/config.json');
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary),
  testSha256: hash(fileURLToPath(import.meta.url)), commands: [], states: [], verdict: 'failed' };
const env = { ...process.env, HOME: home };
const key = 'communication.mode';

function command(program, args, cwd) {
  const result = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, args, cwd, home,
    status: result.status, signal: result.signal, stdout: result.stdout,
    stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.signal, null, 'configuration refusal must not terminate by signal');
  return result;
}
function invoke(args) {
  return command(binary, [...args, '--cwd', project], project);
}
function setting() {
  const result = invoke(['config', 'get', key, '--json']);
  assert.ok(result.status !== null && !result.status, result.stderr);
  return JSON.parse(result.stdout);
}
function expect(value) {
  assert.deepEqual(setting().value, value);
}
function refused(path, operation = ['config', 'get', key, '--json']) {
  const result = invoke(operation);
  assert.ok(result.status !== null && result.status, `expected refusal for ${path}`);
  assert.ok(result.stderr.includes(path), `missing failed path: ${result.stderr}`);
  assert.equal(result.stdout.trim(), '', 'refusal must not report effective settings');
  return result.stderr;
}
function invalidFile(path, content) {
  writeFileSync(path, content);
  const before = readFileSync(path);
  refused(path);
  assert.deepEqual(readFileSync(path), before, 'config get changed refused input');
  report.states.push({ path, beforeSha256: createHash('sha256').update(before).digest('hex'),
    afterSha256: hash(path) });
}

try {
  mkdirSync(dirname(legacy), { recursive: true });
  mkdirSync(dirname(local), { recursive: true });
  const checkout = command('git', ['rev-parse', 'HEAD'], root);
  const short = command('git', ['rev-parse', '--short', revision], root);
  const working = command('git', ['status', '--porcelain'], root);
  const version = invoke(['--version']);
  for (const result of [checkout, short, working, version]) {
    assert.ok(result.status !== null && !result.status, result.stderr);
  }
  report.checkoutRevision = checkout.stdout.trim();
  report.binaryVersion = version.stdout.trim();
  assert.equal(report.checkoutRevision, revision, 'qualify the declared checkout');
  assert.equal(working.stdout.trim(), '', 'commit the candidate and journey before qualification');
  assert.ok(report.binaryVersion.endsWith(`.${short.stdout.trim()}`),
    `candidate must embed the clean source revision: ${report.binaryVersion}`);
  const declaration = setting();
  assert.equal(declaration.type, 'enum');
  const choices = declaration.enum.filter((value) => value !== declaration.default);
  const [legacyMode, currentMode] = choices;
  assert.ok(legacyMode && currentMode && legacyMode !== currentMode,
    'communication.mode must declare distinct override values');
  assert.equal(declaration.value, declaration.default);
  const legacyBytes = JSON.stringify({ communication: { mode: legacyMode } });
  const currentBytes = `communication:\n  mode: ${JSON.stringify(currentMode)}\n`;
  writeFileSync(legacy, legacyBytes);
  expect(legacyMode);
  writeFileSync(current, currentBytes);
  expect(currentMode);
  writeFileSync(local, JSON.stringify({ communication: { mode: declaration.default } }));
  expect(declaration.default);

  invalidFile(legacy, '{');
  const keptCurrent = readFileSync(current);
  const keptLegacy = readFileSync(legacy);
  refused(legacy, ['config', 'set', key, legacyMode]);
  assert.deepEqual(readFileSync(current), keptCurrent,
    'refused legacy retirement must not publish a new canonical layer');
  assert.deepEqual(readFileSync(legacy), keptLegacy,
    'refused config set must preserve the broken legacy layer');
  writeFileSync(legacy, legacyBytes);
  expect(declaration.default);
  invalidFile(current, 'communication: [');
  writeFileSync(current, currentBytes);
  invalidFile(local, '{');
  invalidFile(local, JSON.stringify([]));
  rmSync(local);
  expect(currentMode);

  // The candidate executable supplies real non-text bytes, not a fabricated encoding number.
  const executableBytes = readFileSync(binary);
  assert.throws(() => new TextDecoder(undefined, { fatal: true }).decode(executableBytes),
    'the selected native executable must provide an invalid text fixture');
  invalidFile(current, executableBytes);
  rmSync(current);
  mkdirSync(current);
  const readError = refused(current);
  assert.ok(readError.includes(String(constants.errno.EISDIR)),
    `directory refusal must retain the platform error: ${readError}`);
  assert.ok(statSync(current).isDirectory(), 'refused directory was replaced');
  report.states.push({ path: current, directoryPreserved: true, errno: constants.errno.EISDIR });
  rmSync(current, { recursive: true });
  expect(legacyMode);
  rmSync(legacy);
  expect(declaration.default);
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
