// Real journey: Jeden works without Brama. When Brama does not answer, or is
// not routed to the process at all, a turn is served by the OpenAI-compatible
// provider kept in Jeden's own token store, and standard error names it and
// why; with an empty store the run is refused naming both causes.
// Inputs: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_PROVIDER_ENDPOINT,
// JEDEN_TEST_MODEL, JEDEN_TEST_PROVIDER_TOKEN_A. No simulated provider.
import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { commands } from './commands.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const area = join(root, '.build/model-stand-in', randomUUID());
const home = join(area, 'home');
const workspace = join(area, 'workspace');
const sessions = join(area, 'sessions');
for (const directory of [home, workspace, sessions]) mkdirSync(directory, { recursive: true });
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const sensitive = [];
const redacted = (value) => value === undefined ? undefined : sensitive.reduce(
  (text, secret) => text.replaceAll(secret, '<redacted>'), value);
const report = {
  testSha256: hash(fileURLToPath(import.meta.url)), commands: [], verdict: 'failed',
};
const required = (name) => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} must name a real provider journey input`);
  return value;
};
const { invoke, succeeds, refuses } = commands({ root, area, report, redacted });

// A loopback port nothing listens on: bound, read, then released.
async function closedPort() {
  const server = createServer();
  await new Promise((done) => server.listen(0, '127.0.0.1', done));
  const { port } = server.address();
  await new Promise((done) => server.close(done));
  return port;
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
  const shortRevision = succeeds(invoke('git', ['rev-parse', '--short', revision], process.env)).trim();
  assert.ok(succeeds(invoke(binary, ['--version'], process.env)).trim().endsWith(`.${shortRevision}`),
    'the binary must embed the candidate revision');

  // Brama is routed and credentialed but does not answer.
  const unanswering = `http://127.0.0.1:${await closedPort()}`;
  const env = { ...process.env, HOME: home, JEDEN_SESSION_ROOT: sessions,
    BRAMA_URL: unanswering, BRAMA_TOKEN: randomUUID(), WISENT_APP_AGENT_AUTH_SECRET: randomUUID(),
    WISENT_APP_AGENT_ID: 'jeden-stand-in-journey' };
  for (const name of ['JEDEN_MODEL_ENDPOINT', 'JEDEN_MODEL_KEY', 'JEDEN_DATABASE_URL']) delete env[name];
  const turn = ['run', 'Respond exactly: OK', '--model', model, '--model-only', '--cwd', workspace];

  // 1. Nothing stands in: refused naming Brama's failure and the empty store.
  const empty = invoke(binary, turn, env);
  refuses(empty, 'Brama does not serve this process');
  refuses(empty, "no provider stands in for it: Jeden's token store holds no OpenAI-compatible provider");

  // 2. A stored provider serves the turn Brama does not, and stderr says so.
  succeeds(invoke(binary, ['token', 'add', 'stand-in', '--endpoint', endpoint, '--json'], env, `${token}\n`));
  const served = invoke(binary, turn, env);
  assert.equal(succeeds(served).trim(), 'OK');
  assert.ok(served.stderr.includes('Brama does not serve this process'), redacted(served.stderr));
  assert.ok(served.stderr.includes(`using the OpenAI-compatible provider ${endpoint} from Jeden's token store instead`),
    redacted(served.stderr));

  // 3. No Brama route at all (no address, no Stado CLI): the same provider serves.
  const unrouted = { ...env, PATH: '/usr/bin:/bin' };
  for (const name of ['BRAMA_URL', 'BRAMA_TOKEN', 'WISENT_APP_AGENT_AUTH_SECRET']) delete unrouted[name];
  const local = invoke(binary, turn, unrouted);
  assert.equal(succeeds(local).trim(), 'OK');
  assert.ok(local.stderr.includes('Brama is not routed to this process'), redacted(local.stderr));
  assert.ok(local.stderr.includes(`using the OpenAI-compatible provider ${endpoint}`), redacted(local.stderr));
  report.verdict = 'passed';
} catch (error) {
  report.error = redacted(String(error.stack || error));
  throw error;
} finally {
  writeFileSync(join(area, 'report.json'), JSON.stringify(report, (_, value) =>
    typeof value === 'string' ? redacted(value) : value, '  '));
  console.log(`${report.verdict}: ${join(area, 'report.json')}`);
}
