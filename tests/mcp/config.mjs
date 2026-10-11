// Real MCP declaration lifecycle through Jeden's CLI, without starting a tool server.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
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
const runs = join(root, '.build/mcp-config');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const home = join(output, 'home');
const project = join(output, 'project');
const userFile = join(home, '.jeden/mcp.json');
const projectFile = join(project, '.jeden/mcp.json');
const name = `declared-${randomUUID()}`;
const declaration = { type: 'stdio', command: binary, args: ['mcp'], enabled: false };
const clientMetadata = { declaration: name, sourceRevision: revision };
const valid = JSON.stringify({ mcpServers: { [name]: declaration }, disabledServers: [name], clientMetadata });
const hash = path => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary),
  testSha256: hash(fileURLToPath(import.meta.url)), commands: [], cases: [], verdict: 'failed' };
const env = { ...process.env, HOME: home };
function command(program, args, cwd = project) {
  const result = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, args, cwd, home, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.signal, null, 'a refused declaration must not terminate by signal');
  return result;
}
function succeeds(result) {
  assert.ok(result.status !== null && !result.status, result.stderr);
  return result.stdout.trim();
}
function invoke(slash) {
  return command(binary, ['run', slash, '--json', '--cwd', project]);
}
function listed() {
  return JSON.parse(succeeds(invoke('/mcp list'))).text;
}
function refused(path, slash) {
  const before = [userFile, projectFile].map(file => [file, readFileSync(file)]);
  const result = invoke(slash);
  assert.ok(result.status !== null && result.status, `${slash} accepted invalid ${path}`);
  assert.ok(result.stderr.includes(path), result.stderr);
  for (const [file, bytes] of before) {
    assert.deepEqual(readFileSync(file), bytes, `refusal changed ${file}`);
  }
  report.cases.push({ path, slash, preserved: before.map(([file]) => ({ file, sha256: hash(file) })) });
}

try {
  mkdirSync(dirname(userFile), { recursive: true });
  mkdirSync(dirname(projectFile), { recursive: true });
  report.checkoutRevision = succeeds(command('git', ['rev-parse', 'HEAD'], root));
  assert.equal(report.checkoutRevision, revision);
  assert.equal(succeeds(command('git', ['status', '--porcelain'], root)), '',
    'commit the candidate and journey before qualification');
  const short = succeeds(command('git', ['rev-parse', '--short', revision], root));
  report.binaryVersion = succeeds(command(binary, ['--version']));
  assert.ok(report.binaryVersion.endsWith(`.${short}`), 'candidate must embed the clean source revision');
  const absent = listed();
  writeFileSync(userFile, valid);
  writeFileSync(projectFile, JSON.stringify({}));
  const configured = listed();
  assert.ok(configured.includes(name), configured);
  assert.ok(configured.includes('disabled'), configured);
  assert.notEqual(configured, absent);

  const invalid = [
    ['syntax', '{'],
    ['root', JSON.stringify([])],
    ['servers', JSON.stringify({ mcpServers: [] })],
    ['server', JSON.stringify({ mcpServers: { [name]: false } })],
    ['enabled', JSON.stringify({ mcpServers: { [name]: { ...declaration, enabled: 'false' } } })],
    ['disabled', JSON.stringify({ mcpServers: { [name]: declaration }, disabledServers: {} })],
    ['disabled-member', JSON.stringify({ mcpServers: { [name]: declaration }, disabledServers: [false] })],
  ];
  for (const [label, bytes] of invalid) {
    // A valid project layer cannot conceal the malformed earlier user layer.
    writeFileSync(userFile, bytes);
    writeFileSync(projectFile, valid);
    refused(userFile, '/mcp list');
    refused(userFile, `/mcp remove --scope user ${name}`);
    writeFileSync(userFile, valid);
    writeFileSync(projectFile, bytes);
    refused(projectFile, '/mcp list');
    refused(projectFile, `/mcp remove --scope project ${name}`);
    report.cases.push({ case: label });
  }
  rmSync(projectFile);
  mkdirSync(projectFile);
  const directory = invoke('/mcp list');
  assert.ok(directory.status !== null && directory.status, 'directory must not count as absent declarations');
  assert.ok(directory.stderr.includes(projectFile), directory.stderr);
  assert.ok(directory.stderr.includes(String(constants.errno.EISDIR)), directory.stderr);
  rmSync(projectFile, { recursive: true });
  assert.equal(listed(), configured, 'removing invalid project state restores user declarations');
  succeeds(invoke(`/mcp remove --scope user ${name}`));
  const persisted = JSON.parse(readFileSync(userFile, 'utf8'));
  assert.equal(Object.hasOwn(persisted.mcpServers, name), false);
  assert.equal(persisted.disabledServers.includes(name), false);
  assert.deepEqual(persisted.clientMetadata, clientMetadata, 'mutation discarded unrelated configuration');
  assert.equal(listed(), absent, 'successful removal must be visible through the reader');
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
