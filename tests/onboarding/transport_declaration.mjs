// Real CLI test: `/onboarding` refuses a half-declared Stado integration and
// an account with no user name by name, instead of falling back to offline
// reporting or to a shared placeholder subject. With neither integration
// variable declared the journey opens offline. HOME and the project are
// isolated under .build; every command, its exit status and its output are
// kept in report.json.
//
// Run: JEDEN_BIN=target/debug/jeden JEDEN_TEST_SOURCE_REVISION=$(git rev-parse HEAD) \
//   node tests/onboarding/transport_declaration.mjs
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
const runs = join(root, '.build/onboarding-transport-declaration');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  commands: [], verdict: 'failed' };

const home = join(output, 'home');
const project = join(output, 'project');
mkdirSync(home, { recursive: true });
mkdirSync(project, { recursive: true });
const base = { ...process.env, HOME: home };
delete base.STADO_INTEGRATION_API_URL;
delete base.JEDEN_STADO_INTEGRATION_TOKEN;

function onboarding(name, env) {
  const args = ['run', '/onboarding', '--json', '--cwd', project];
  const result = spawnSync(binary, args, { cwd: project, env, encoding: 'utf8' });
  report.commands.push({ name, args, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.signal, null, '/onboarding must not end by signal');
  return result;
}
function refused(name, env, words) {
  const result = onboarding(name, env);
  assert.ok(result.status, `${name}: /onboarding accepted it: ${result.stdout}`);
  assert.ok(result.stderr.includes(words), `${name}: did not say ${words}: ${result.stderr}`);
}

try {
  refused('url-without-token', { ...base, STADO_INTEGRATION_API_URL: 'https://integrations.example' },
    'STADO_INTEGRATION_API_URL is declared without JEDEN_STADO_INTEGRATION_TOKEN');
  refused('token-without-url', { ...base, JEDEN_STADO_INTEGRATION_TOKEN: 'declared-token' },
    'JEDEN_STADO_INTEGRATION_TOKEN is declared without STADO_INTEGRATION_API_URL');
  const nameless = { ...base };
  delete nameless.USER;
  refused('no-user', nameless, 'USER cannot be read');

  const offline = onboarding('offline', base);
  assert.ok(offline.status !== null && !offline.status, offline.stderr);
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
