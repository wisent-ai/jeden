// Real catalogue discovery through the candidate CLI, with an isolated copy of a genuine catalogue.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
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
const fixture = resolve(required('JEDEN_TEST_MARKETPLACE_CATALOGUE'));
const original = readFileSync(fixture);
const catalogue = JSON.parse(original);
assert.ok(Array.isArray(catalogue.plugins), 'supply a genuine plugin catalogue');
const names = catalogue.plugins.map(plugin => plugin.name);
assert.ok(names.some(name => typeof name === 'string' && name.trim()), 'catalogue must declare a named plugin');
const runs = join(root, '.build/marketplace-catalogue');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const home = join(output, 'home');
const project = join(output, 'project');
const source = `catalogue-${randomUUID()}`;
const cache = join(home, '.jeden/plugins/cache/marketplaces', source);
const preferred = join(cache, '.jeden-plugin/marketplace.json');
const alternate = join(cache, '.claude-plugin/marketplace.json');
const registry = join(project, '.jeden/plugins.json');
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const report = { revision, binary, binarySha256: hash(readFileSync(binary)), fixture,
  fixtureSha256: hash(original), testSha256: hash(readFileSync(fileURLToPath(import.meta.url))),
  commands: [], cases: [], verdict: 'failed' };
const env = { ...process.env, HOME: home, JEDEN_PLUGINS_HOME: home };
function command(program, args, cwd = project) {
  const result = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, args, cwd, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  return result;
}
function succeeds(result) {
  assert.ok(result.status !== null && !result.status, result.stderr);
  return result.stdout.trim();
}
function discover() {
  return command(binary, ['run', '/marketplace discover', '--json', '--cwd', project]);
}
function discovered() {
  const text = JSON.parse(succeeds(discover())).text;
  for (const name of names.filter(name => typeof name === 'string' && name.trim())) {
    assert.ok(text.includes(`${name}@${source}`), text);
  }
  return text;
}
function refused(path, cause) {
  const result = discover();
  assert.ok(result.status !== null && result.status, 'unreadable catalogue was accepted');
  assert.ok(result.stderr.includes(path), result.stderr);
  assert.ok(result.stderr.includes(cause), result.stderr);
  assert.deepEqual(readFileSync(alternate), original, 'refusal changed the alternate catalogue');
  report.cases.push({ path, cause, stderr: result.stderr });
}
try {
  mkdirSync(dirname(registry), { recursive: true });
  mkdirSync(dirname(preferred), { recursive: true });
  mkdirSync(dirname(alternate), { recursive: true });
  assert.equal(succeeds(command('git', ['rev-parse', 'HEAD'], root)), revision);
  assert.equal(succeeds(command('git', ['status', '--porcelain'], root)), '', 'commit source and journey before qualification');
  const short = succeeds(command('git', ['rev-parse', '--short', revision], root));
  report.binaryVersion = succeeds(command(binary, ['--version']));
  assert.ok(report.binaryVersion.endsWith(`.${short}`), 'candidate must embed the clean source revision');
  const declaration = JSON.stringify({ sources: { [source]: { source: dirname(dirname(fixture)) } } });
  writeFileSync(registry, declaration);
  writeFileSync(alternate, original);
  const baseline = discovered();
  writeFileSync(preferred, original);
  assert.equal(discovered(), baseline);
  const characters = [...original.toString().trimEnd()];
  assert.equal(characters.pop(), '}');
  const malformed = characters.join('');
  writeFileSync(preferred, malformed);
  refused(preferred, 'invalid JSON');
  assert.equal(readFileSync(preferred, 'utf8'), malformed);
  rmSync(preferred);
  mkdirSync(preferred);
  let observed;
  try { readFileSync(preferred); } catch (error) { observed = error; }
  assert.ok(observed, 'host must reject reading a directory as a file');
  refused(preferred, String(Math.abs(observed.errno)));
  rmSync(preferred, { recursive: true });
  assert.equal(discovered(), baseline, 'removing invalid preferred path must restore alternate discovery');
  writeFileSync(preferred, original);
  assert.equal(discovered(), baseline, 'restored catalogue must recover discovery');
  assert.equal(readFileSync(registry, 'utf8'), declaration);
  assert.deepEqual(readFileSync(fixture), original, 'original catalogue changed');
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
