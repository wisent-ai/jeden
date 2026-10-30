// Real journey: Jeden takes its model service from Stado's capability
// resolution, never by dialling Brama through the service directory. In a
// home whose registry places Brama nowhere, the real Stado JEDEN_TEST_STADO
// names (first on PATH as `stado`):
//   1. with no endpoint declared, `jeden run` is refused naming every
//      inference alternative Stado tried (brama, model-endpoint) with its
//      cause, and why nothing else stands in;
//   2. with the real OpenAI-compatible provider declared as the endpoint in
//      Brama's place (capabilities.inference.endpoint.url, its model, and
//      the role whose item holds its key, kept in an owner-only file store),
//      the turn is served by that provider with that model, standard error
//      says so, and Stado records that model-endpoint served jeden as a
//      fallback.
// Inputs: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_STADO,
// JEDEN_TEST_PROVIDER_ENDPOINT, JEDEN_TEST_MODEL, JEDEN_TEST_PROVIDER_TOKEN_A.
// No simulated provider; the key never reaches the report.
import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, symlinkSync, writeFileSync } from 'node:fs';
import { basename, join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { commands } from './commands.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const area = join(root, '.build/model-resolved', randomUUID());
const home = join(area, 'home');
const bin = join(area, 'bin');
const workspace = join(area, 'workspace');
const sessions = join(area, 'sessions');
for (const directory of [home, bin, workspace, sessions, join(area, 'tmp'), join(area, 'storage')]) {
  mkdirSync(directory, { recursive: true });
}
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const sensitive = [];
const redacted = (value) => value === undefined ? undefined : sensitive.reduce(
  (text, secret) => text.replaceAll(secret, '<redacted>'), value);
const report = { testSha256: hash(fileURLToPath(import.meta.url)), commands: [], verdict: 'failed' };
const required = (name) => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} must name a real resolved-provider journey input`);
  return value;
};
const { invoke, succeeds, refuses } = commands({ root, area, report, redacted });

try {
  const binary = resolve(required('JEDEN_BIN'));
  const revision = required('JEDEN_TEST_SOURCE_REVISION');
  const stado = resolve(required('JEDEN_TEST_STADO'));
  assert.equal(basename(stado), 'stado', `${stado} is not a stado binary`);
  report.revision = revision;
  report.binarySha256 = hash(binary);
  report.stadoSha256 = hash(stado);
  assert.equal(succeeds(invoke('git', ['rev-parse', 'HEAD'], process.env)).trim(), revision);
  symlinkSync(stado, join(bin, 'stado'));
  report.stadoVersion = succeeds(invoke(stado, ['--version'], process.env)).trim();

  // The candidate Stado leads PATH; the rest of the caller's PATH stays, as the
  // stand-in journey keeps it, because Jeden's extension host runs on it.
  const env = {
    PATH: `${bin}:${process.env.PATH}`, HOME: home, TMPDIR: join(area, 'tmp'),
    WC_STORAGE_BACKEND: 'local', WC_LOCAL_STORAGE_PATH: join(area, 'storage'),
    JEDEN_SESSION_ROOT: sessions,
  };
  const stadoRun = (args, input) => succeeds(invoke(stado, args, env, input));
  const turn = ['run', 'Respond exactly: OK', '--model-only', '--cwd', workspace];
  stadoRun(['config', 'init']);

  // 1. Nothing answers in Brama's place: every alternative is named.
  const refused = invoke(binary, turn, env);
  refuses(refused, 'no inference alternative answers jeden');
  refuses(refused, 'brama at ');
  refuses(refused, 'model-endpoint at capabilities.inference.endpoint.url');
  refuses(refused, "no provider stands in for it: Jeden's token store holds no OpenAI-compatible provider");

  // 2. The operator declares the real provider in Brama's place.
  const endpoint = required('JEDEN_TEST_PROVIDER_ENDPOINT').replace(/\/+$/, '');
  const model = required('JEDEN_TEST_MODEL');
  const token = required('JEDEN_TEST_PROVIDER_TOKEN_A');
  sensitive.push(token);
  const store = join(area, 'credentials.json');
  const role = 'jeden-journey-provider';
  stadoRun(['config', 'set', 'credentials.store', `file://${store}`]);
  stadoRun(['credentials', 'put', '--role', role], JSON.stringify({ token }));
  stadoRun(['config', 'set', 'capabilities.inference.endpoint.url', endpoint]);
  stadoRun(['config', 'set', 'capabilities.inference.endpoint.model', model]);
  stadoRun(['config', 'set', 'capabilities.inference.endpoint.key_role', role]);

  // The provider's answer is its own: the journey proves the turn was served
  // by it, not what a given model writes.
  const served = invoke(binary, turn, env);
  assert.ok(succeeds(served).trim(), 'the provider Stado resolved answered the turn with no text');
  assert.ok(served.stderr.includes(
    `using model-endpoint at ${endpoint} with model ${model}, which Stado resolves in Brama's place instead`),
  redacted(served.stderr));
  const status = JSON.parse(succeeds(invoke(binary, ['token', 'status', '--json'], env)));
  report.tokenStatus = status;
  const listed = JSON.parse(stadoRun(['capabilities', 'inference', '--json']));
  const kept = listed.alternatives[0].served.jeden;
  assert.equal(kept.provider, 'model-endpoint', JSON.stringify(listed));
  assert.equal(kept.fallback, true, JSON.stringify(listed));
  report.verdict = 'passed';
} catch (error) {
  report.error = redacted(String(error.stack || error));
  throw error;
} finally {
  writeFileSync(join(area, 'report.json'), JSON.stringify(report, (_, value) =>
    typeof value === 'string' ? redacted(value) : value, '  '));
  console.log(`${report.verdict}: ${join(area, 'report.json')}`);
}
