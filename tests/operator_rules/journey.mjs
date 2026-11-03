// Real journey: a standing rule the operator states in one session holds in
// a later, separate session, with no mention of it in the later prompt. The
// first run's own model reads the rule from the operator's message and keeps
// it in Jeden's memory (here the local SQLite stand-in, no fleet database);
// the second run's system prompt carries it.
// Inputs: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_PROVIDER_ENDPOINT,
// JEDEN_TEST_MODEL, JEDEN_TEST_PROVIDER_TOKEN_A. No simulated provider.
import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { commands } from '../model/credentials/commands.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const area = join(root, '.build/operator-rules', randomUUID());
const home = join(area, 'home');
const workspace = join(area, 'workspace');
const sessions = join(area, 'sessions');
for (const directory of [home, workspace, sessions]) mkdirSync(directory, { recursive: true });
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const sensitive = [];
const redacted = (value) => value === undefined ? undefined : sensitive.reduce(
  (text, secret) => text.replaceAll(secret, '<redacted>'), value);
const report = { testSha256: hash(fileURLToPath(import.meta.url)), commands: [], verdict: 'failed' };
const required = (name) => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} must name a real provider journey input`);
  return value;
};
const { invoke, succeeds } = commands({ root, area, report, redacted });

// Every file a session wrote, so the ledger is read wherever it keeps entries.
function files(directory) {
  return readdirSync(directory).flatMap((name) => {
    const path = join(directory, name);
    return statSync(path).isDirectory() ? files(path) : [path];
  });
}

try {
  const binary = resolve(required('JEDEN_BIN'));
  const revision = required('JEDEN_TEST_SOURCE_REVISION');
  const endpoint = required('JEDEN_TEST_PROVIDER_ENDPOINT').replace(/\/+$/, '');
  const model = required('JEDEN_TEST_MODEL');
  const token = required('JEDEN_TEST_PROVIDER_TOKEN_A');
  sensitive.push(token, endpoint);
  report.revision = revision;
  report.binarySha256 = hash(binary);
  assert.equal(succeeds(invoke('git', ['rev-parse', 'HEAD'], process.env)).trim(), revision);

  // No Brama route, no Stado, no fleet database: Jeden on its own.
  const env = { ...process.env, HOME: home, JEDEN_SESSION_ROOT: sessions, PATH: '/usr/bin:/bin' };
  for (const name of ['BRAMA_URL', 'BRAMA_TOKEN', 'WISENT_APP_AGENT_AUTH_SECRET', 'JEDEN_MODEL_ENDPOINT',
    'JEDEN_MODEL_KEY', 'JEDEN_DATABASE_URL']) delete env[name];
  succeeds(invoke(binary, ['token', 'add', 'rules', '--endpoint', endpoint, '--json'], env, `${token}\n`));
  const turn = (prompt) => ['run', prompt, '--model', model, '--cwd', workspace];

  // 1. The operator states a standing rule; the reading keeps it.
  succeeds(invoke(binary, turn('Od teraz zawsze kończ każdą odpowiedź słowem KONIEC.'), env));
  const kept = files(sessions).map((path) => readFileSync(path, 'utf8'))
    .some((text) => text.includes('"source":"operator_rules"') && text.includes('"kept":true')
      && text.includes('KONIEC'));
  assert.ok(kept, 'the first session ledger records no kept operator rule');

  // 2. A separate session, whose prompt says nothing of the rule, follows it.
  const later = invoke(binary, turn('Napisz jedno słowo: TAK'), env);
  const answer = succeeds(later).trim();
  assert.ok(!later.stderr.includes("the operator's rules are not in this prompt"), redacted(later.stderr));
  assert.match(answer, /KONIEC\W*$/, `the later session did not follow the kept rule: ${answer}`);
  report.verdict = 'passed';
} catch (error) {
  report.error = redacted(String(error.stack || error));
  throw error;
} finally {
  writeFileSync(join(area, 'report.json'), JSON.stringify(report, (_, value) =>
    typeof value === 'string' ? redacted(value) : value, '  '));
  console.log(`${report.verdict}: ${join(area, 'report.json')}`);
}
