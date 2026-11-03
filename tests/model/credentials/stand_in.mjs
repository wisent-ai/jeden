// Real journey: Jeden works without Brama. When Brama does not answer, or is
// not routed to the process at all, a turn is served by the OpenAI-compatible
// provider kept in Jeden's own token store, and standard error names it and
// why; with an empty store the run is refused naming both causes.
// Inputs: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_PROVIDER_ENDPOINT,
// JEDEN_TEST_MODEL, JEDEN_TEST_PROVIDER_TOKEN_A, and JEDEN_TEST_BRAMA_BIN, the
// real brama binary (its serve is started and its pool and sign-in API read;
// its directory leads PATH and its digest and version are in the report).
// No simulated
// provider. The success path of Brama's own gateway (a completed sign-in and a
// model turn on its subscription) needs a real subscription account signed in
// and is qualified where one is; this journey covers the refusals.
import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { constants, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { networkInterfaces } from 'node:os';
import { createServer } from 'node:net';
import { basename, dirname, join, resolve } from 'node:path';
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

/** Port zero asks the operating system for a port nothing holds:
 * https://nodejs.org/api/net.html#serverlistenport-host-backlog-callback */
const ANY_PORT = 0;

// This machine's IPv4 loopback address, as its own interfaces name it.
function loopback() {
  const found = Object.values(networkInterfaces()).flat()
    .find((entry) => entry?.internal && entry.family === 'IPv4');
  assert.ok(found, 'this machine names no IPv4 loopback interface');
  return found.address;
}

// A loopback URL nothing listens on: a port bound, read, then released.
async function closedPort() {
  const host = loopback();
  const server = createServer();
  await new Promise((done) => server.listen(ANY_PORT, host, done));
  const { port } = server.address();
  await new Promise((done) => server.close(done));
  return `http://${host}:${port}`;
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
  // The brama that Jeden starts is the one named here, found first on PATH,
  // and the report says which build it was.
  const brama = resolve(required('JEDEN_TEST_BRAMA_BIN'));
  assert.equal(basename(brama), 'brama', `${brama} is not a brama binary`);
  report.bramaBinary = brama;
  report.bramaSha256 = hash(brama);
  report.bramaVersion = succeeds(invoke(brama, ['--version'], process.env)).trim();

  // Brama is routed and credentialed but does not answer.
  const unanswering = await closedPort();
  const env = { ...process.env, HOME: home, JEDEN_SESSION_ROOT: sessions,
    PATH: `${dirname(brama)}:${process.env.PATH}`,
    BRAMA_URL: unanswering, BRAMA_TOKEN: randomUUID(), WISENT_APP_AGENT_AUTH_SECRET: randomUUID(),
    WISENT_APP_AGENT_ID: 'jeden-stand-in-journey' };
  for (const name of ['JEDEN_MODEL_ENDPOINT', 'JEDEN_MODEL_KEY', 'JEDEN_DATABASE_URL']) delete env[name];
  const turn = ['run', 'Respond exactly: OK', '--model', model, '--model-only', '--cwd', workspace];

  // 1. Nothing stands in: refused naming Brama's failure, the empty store and
  //    the absent grants file of Jeden's own gateway.
  const grants = join(home, '.jeden/brama/grants.json');
  const empty = invoke(binary, turn, env);
  refuses(empty, 'Brama does not serve this process');
  refuses(empty, "no provider stands in for it: Jeden's token store holds no OpenAI-compatible provider");
  refuses(empty, `Jeden holds no subscription grant of its own (${grants}:`);
  refuses(empty, 'jeden token sign-in PROVIDER --account EMAIL --reason TEXT signs one in');

  // 1b. An owner-only grants file that is no grant map: the run is refused
  //     naming the file, and the sign-in that would start the real brama
  //     serve is refused with what it printed.
  mkdirSync(dirname(grants), { recursive: true, mode: constants.S_IRWXU });
  writeFileSync(grants, '["not a map"]', { mode: constants.S_IRUSR | constants.S_IWUSR });
  const untrusted = invoke(binary, turn, env);
  refuses(untrusted, 'no provider stands in for it: Jeden\'s grants file');
  refuses(untrusted, 'is not a map from subscription to grant');
  const untrustedSignIn = invoke(binary, ['token', 'sign-in', 'claude-code', '--account',
    'journey@example.com', '--reason', 'stand-in journey: a grants file that is no map'], env, '\n');
  refuses(untrustedSignIn, 'brama serve ended before its listener was bound');
  refuses(untrustedSignIn, 'is not a map from subscription to grant');
  rmSync(grants);

  // 1b2. A grants file holding an empty map: no subscription is signed in,
  //      so the run is refused the same way as with no file at all.
  writeFileSync(grants, '{}', { mode: constants.S_IRUSR | constants.S_IWUSR });
  const emptied = invoke(binary, turn, env);
  refuses(emptied, 'Brama does not serve this process');
  refuses(emptied, 'Jeden holds no subscription grant of its own');
  rmSync(grants);

  // 1c. A sign-in into Jeden's own gateway: the real gateway begins it and
  //     Jeden prints the provider's page; an empty paste is refused.
  const signIn = invoke(binary, ['token', 'sign-in', 'claude-code', '--account', 'journey@example.com',
    '--reason', 'stand-in journey: an empty paste is refused'], env, '\n');
  refuses(signIn, 'no code was pasted');
  assert.ok(signIn.stderr.includes('Open this page, sign in as journey@example.com'), redacted(signIn.stderr));
  //     The refused sign-in banked nothing: status still says no subscription
  //     is signed into Jeden's own gateway, and names the command that does.
  const ownGateway = JSON.parse(succeeds(invoke(binary, ['token', 'status', '--json'], env))).ownGateway;
  assert.ok(ownGateway.state.startsWith('no subscription signed in; jeden token sign-in'), JSON.stringify(ownGateway));

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
