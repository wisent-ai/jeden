// The real CLI reads an isolated workspace .env before running a command.
// Malformed declarations must fail with their source line rather than silently
// dropping a variable; replacing the file with a valid declaration recovers.
// Run with JEDEN_BIN and JEDEN_TEST_SOURCE_REVISION set to the candidate.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
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
const runs = join(root, '.build/setup-env-files');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const home = join(output, 'home');
const project = join(output, 'project');
mkdirSync(home);
mkdirSync(project);
const envFile = join(project, '.env');
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary),
  testSha256: hash(fileURLToPath(import.meta.url)), commands: [], verdict: 'failed' };
const env = { ...process.env, HOME: home, JEDEN_TAMA_REGISTRY: '' };
delete env.JEDEN_MODEL_ENDPOINT;
delete env.JEDEN_MODEL_KEY;
delete env.BRAMA_URL;
delete env.WISENT_APP_AGENT_ID;
delete env.WISENT_APP_AGENT_SECRET;

function invoke(args) {
  const result = spawnSync(binary, args, { cwd: project, env, encoding: 'utf8' });
  report.commands.push({ args, envFile: readFileSync(envFile, 'utf8'), status: result.status,
    signal: result.signal, stdout: result.stdout, stderr: result.stderr,
    error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.signal, null, 'the CLI must answer rather than die by signal');
  return result;
}

function sourceCommand(program, args) {
  const result = spawnSync(program, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ program, args, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

try {
  report.checkoutRevision = sourceCommand('git', ['rev-parse', 'HEAD']);
  assert.equal(report.checkoutRevision, revision, 'qualify the declared checkout');
  const short = sourceCommand('git', ['rev-parse', '--short', revision]);
  assert.equal(sourceCommand('git', ['status', '--porcelain']), '',
    'commit the candidate and journey before qualification');
  report.binaryVersion = sourceCommand(binary, ['--version']);
  assert.ok(report.binaryVersion.endsWith(`.${short}`),
    `candidate binary must embed the clean source revision: ${report.binaryVersion}`);
  for (const malformed of ['export SAMPLE=value', ' SAMPLE=value', 'SAMPLE =value', '=value']) {
    writeFileSync(envFile, `${malformed}\n`);
    const result = invoke(['run', '/hooks', '--json', '--cwd', project]);
    assert.ok(result.status, `the CLI accepted malformed declaration ${malformed}`);
    assert.ok(result.stderr.includes(envFile), result.stderr);
    assert.ok(result.stderr.includes('line index 0 (zero-based)'), result.stderr);
    assert.ok(result.stderr.includes(`holds \`${malformed}\``), result.stderr);
  }
  for (const [malformed, cause] of [
    ['SAMPLE="unterminated', 'double-quoted value is not valid JSON'],
    ['SAMPLE="bad\\q"', 'invalid escape'],
    ["SAMPLE='unterminated", 'single-quoted value has no closing quote'],
  ]) {
    writeFileSync(envFile, `${malformed}\n`);
    const result = invoke(['run', '/hooks', '--json', '--cwd', project]);
    assert.ok(result.status, `the CLI accepted malformed value ${malformed}`);
    assert.ok(result.stderr.includes(envFile), result.stderr);
    assert.ok(result.stderr.includes('line index 0 (zero-based)'), result.stderr);
    assert.ok(result.stderr.includes(cause), result.stderr);
  }

  writeFileSync(envFile, 'JEDEN_MODEL_ENDPOINT=https://example.invalid/recovered\n');
  const recovered = invoke(['run', '/setup status', '--json', '--cwd', project]);
  assert.ok(recovered.status !== null && !recovered.status, recovered.stderr);
  assert.ok(JSON.parse(recovered.stdout).text.includes('Endpoint: https://example.invalid/recovered\n'),
    `the CLI did not read the repaired declaration: ${recovered.stdout}`);
  writeFileSync(envFile,
    'JEDEN_MODEL_ENDPOINT=https://example.invalid/recovered\nJEDEN_MODEL_KEY= # outside comment\n');
  const empty = invoke(['run', '/setup status', '--json', '--cwd', project]);
  assert.ok(empty.status !== null && !empty.status, empty.stderr);
  const emptyText = JSON.parse(empty.stdout).text;
  assert.ok(emptyText.includes('Endpoint: https://example.invalid/recovered\n'), emptyText);
  const emptySource = invoke(['token', 'status', '--json']);
  assert.ok(emptySource.status !== null && !emptySource.status, emptySource.stderr);
  assert.equal(JSON.parse(emptySource.stdout).keySource, 'none',
    'a comment after an empty value became a provider key');
  writeFileSync(envFile, 'JEDEN_MODEL_ENDPOINT=https://example.invalid/plain # outside comment\n');
  const commented = invoke(['run', '/setup status', '--json', '--cwd', project]);
  assert.ok(commented.status !== null && !commented.status, commented.stderr);
  assert.ok(JSON.parse(commented.stdout).text.includes('Endpoint: configured by JEDEN_MODEL_ENDPOINT'),
    `the setup screen did not recognize the direct endpoint: ${commented.stdout}`);
  assert.ok(!commented.stdout.includes('https://example.invalid/plain'),
    'the setup checklist exposed the provider endpoint path');
  assert.equal(JSON.parse(invoke(['token', 'status', '--json']).stdout).endpoint,
    'https://example.invalid/plain', 'an unquoted comment became part of the endpoint');

  const invalidEndpoint = 'https://user:example-secret@api.example.invalid';
  writeFileSync(envFile, `JEDEN_MODEL_ENDPOINT=${JSON.stringify(invalidEndpoint)}\n`);
  const invalid = invoke(['run', '/setup status', '--json', '--cwd', project]);
  assert.ok(invalid.status !== null && !invalid.status, invalid.stderr);
  const invalidText = JSON.parse(invalid.stdout).text;
  assert.ok(invalidText.includes('Endpoint: refused — JEDEN_MODEL_ENDPOINT: model endpoint must be HTTPS or loopback HTTP, without credentials, query or fragment'), invalidText);
  assert.ok(!`${invalid.stdout}\n${invalid.stderr}`.includes('example-secret'), 'the setup status exposed endpoint credentials');
  writeFileSync(envFile, 'JEDEN_MODEL_ENDPOINT=https://@api.example.invalid\n');
  const emptyUserinfo = invoke(['token', 'status', '--json']);
  assert.equal(emptyUserinfo.status, 0, emptyUserinfo.stderr);
  const refusedUrl = JSON.parse(emptyUserinfo.stdout);
  assert.equal(refusedUrl.keySource, 'refused');
  assert.match(refusedUrl.routeRefusal, /without credentials, query or fragment/);
  writeFileSync(envFile, 'JEDEN_MODEL_ENDPOINT=https://example.invalid/plain\n');
  const agentId = "'alpha";
  const saved = invoke(['run', `/setup agent-id ${agentId}`, '--json', '--cwd', project]);
  assert.ok(saved.status !== null && !saved.status, saved.stderr);
  const userEnvFile = join(home, '.jeden', '.env');
  assert.equal(readFileSync(userEnvFile, 'utf8'), `WISENT_APP_AGENT_ID=${JSON.stringify(agentId)}\n`);
  writeFileSync(userEnvFile,
    `UNRELATED=keep\r\n# note\r\nWISENT_APP_AGENT_ID=${JSON.stringify(agentId)}\r\nTAIL=unchanged`);
  const updated = invoke(['run', '/setup agent-id beta', '--json', '--cwd', project]);
  assert.ok(updated.status !== null && !updated.status, updated.stderr);
  assert.equal(readFileSync(userEnvFile, 'utf8'),
    'UNRELATED=keep\r\n# note\r\nWISENT_APP_AGENT_ID=beta\r\nTAIL=unchanged');
  const appended = invoke(['run', '/setup brama-url http://example.invalid', '--json', '--cwd', project]);
  assert.ok(appended.status !== null && !appended.status, appended.stderr);
  assert.equal(readFileSync(userEnvFile, 'utf8'),
    'UNRELATED=keep\r\n# note\r\nWISENT_APP_AGENT_ID=beta\r\nTAIL=unchanged\nBRAMA_URL=http://example.invalid\n');
  report.finalUserEnv = readFileSync(userEnvFile, 'utf8');
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
