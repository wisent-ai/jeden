// The screen language as Jeden Desktop and the terminal set it: `jeden rpc`
// `config/language/get` lists every fleet language, `config/language/set`
// saves one into the user configuration, `jeden config get ui.language` reads
// the same value back, and a code outside the list or a missing parameter is
// refused without changing the file. Run with JEDEN_BIN and
// JEDEN_TEST_SOURCE_REVISION naming the candidate; the report stays under
// .build/config-language.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { pinnedGit } from '../support/home.mjs';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = resolve(required('JEDEN_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const runs = join(root, '.build/config-language');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const home = join(output, 'home');
const project = join(output, 'project');
mkdirSync(join(home, '.jeden'), { recursive: true });
mkdirSync(project, { recursive: true });
const config = join(home, '.jeden/config.yml');
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), commands: [], verdict: 'failed' };
const env = { ...process.env, ...pinnedGit, HOME: home };

async function rpc(requests) {
  const child = spawn(binary, ['rpc'], { cwd: project, env, stdio: ['pipe', 'pipe', 'pipe'] });
  let stdout = '';
  let stderr = '';
  child.stdout.on('data', (chunk) => { stdout += chunk; });
  child.stderr.on('data', (chunk) => { stderr += chunk; });
  for (const request of requests) child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', ...request })}\n`);
  child.stdin.end(`${JSON.stringify({ jsonrpc: '2.0', id: 'shutdown', method: 'shutdown', params: {} })}\n`);
  const status = await new Promise((done) => child.on('close', done));
  report.commands.push({ program: binary, args: ['rpc'], requests, status, stdout, stderr });
  const answers = stdout.split('\n').filter(Boolean).map((line) => JSON.parse(line));
  return requests.map((request) => answers.find((answer) => answer.id === request.id));
}
function cli(args) {
  const result = spawnSync(binary, [...args, '--cwd', project], { cwd: project, env, encoding: 'utf8' });
  report.commands.push({ program: binary, args, status: result.status, stdout: result.stdout, stderr: result.stderr });
  return result;
}

try {
  writeFileSync(config, 'communication:\n  mode: normal\n');
  const [listed] = await rpc([{ id: 'get', method: 'config/language/get', params: {} }]);
  assert.equal(listed.result.language, 'auto', JSON.stringify(listed));
  const codes = listed.result.languages.map((language) => language.code);
  assert.ok(codes.length >= 65 && codes.includes('pl') && codes.includes('hi'), codes.join(' '));
  assert.ok(listed.result.languages.every((language) => language.native && language.english));

  const [saved, refusedCode, refusedParams] = await rpc([
    { id: 'set', method: 'config/language/set', params: { language: 'de' } },
    { id: 'bad', method: 'config/language/set', params: { language: 'xx' } },
    { id: 'none', method: 'config/language/set', params: {} },
  ]);
  assert.equal(saved.result.language, 'de', JSON.stringify(saved));
  assert.equal(refusedCode.error?.code, 'config_write_failed', JSON.stringify(refusedCode));
  assert.match(refusedCode.error.message, /xx/);
  assert.equal(refusedParams.error?.code, 'invalid_params', JSON.stringify(refusedParams));

  const read = cli(['config', 'get', 'ui.language', '--json']);
  assert.equal(read.status, 0, read.stderr);
  assert.equal(JSON.parse(read.stdout).value, 'de');
  // Jeden writes config.yml as JSON, which is valid YAML.
  const savedFile = JSON.parse(readFileSync(config, 'utf8'));
  assert.equal(savedFile.ui.language, 'de');
  assert.equal(savedFile.communication.mode, 'normal', 'the other settings stay');

  writeFileSync(config, 'not: [valid');
  const before = hash(config);
  // Every jeden command refuses to start on a configuration it cannot read,
  // naming the file: `jeden rpc` answers nothing and exits non-zero.
  await rpc([{ id: 'broken', method: 'config/language/get', params: {} }]);
  const refusal = report.commands.at(-1);
  assert.notEqual(refusal.status, 0, 'an unreadable configuration must stop jeden rpc');
  assert.match(refusal.stderr, /config\.yml is not valid/);
  assert.equal(hash(config), before, 'an unreadable configuration is never overwritten');
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
