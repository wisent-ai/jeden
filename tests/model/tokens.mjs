// Real provider and Jeden CLI journey: private tokens, refusal, rotation and
// persistent removal. No simulated endpoint or copied operator home.
import assert from 'node:assert/strict';
import { commands } from './credentials/commands.mjs';
import { createHash, randomUUID } from 'node:crypto';
import { chmodSync, constants, existsSync, mkdirSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startRpc } from '../hooks/rpc.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const area = join(root, '.build/model-tokens', randomUUID());
const home = join(area, 'home');
const workspace = join(area, 'workspace');
const sessions = join(area, 'sessions');
for (const directory of [home, workspace, sessions]) mkdirSync(directory, { recursive: true });
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const sensitive = [];
const redacted = (value) => value === undefined ? undefined : sensitive.reduce(
  (text, secret) => text.replaceAll(secret, '<redacted>'), value);
const redactValue = (_, value) => typeof value === 'string' ? redacted(value) : value;
const report = {
  revision: process.env.JEDEN_TEST_SOURCE_REVISION,
  binary: process.env.JEDEN_BIN,
  endpointSha256: undefined,
  model: process.env.JEDEN_TEST_MODEL,
  testSha256: hash(fileURLToPath(import.meta.url)),
  commands: [], frames: [], verdict: 'failed',
};
const required = (name) => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} must name a real provider journey input`);
  return value;
};
const { invoke, succeeds, refuses, list } = commands({ root, area, report, redacted });
try {
  const binary = resolve(required('JEDEN_BIN'));
  const revision = required('JEDEN_TEST_SOURCE_REVISION');
  const endpoint = required('JEDEN_TEST_PROVIDER_ENDPOINT');
  const model = required('JEDEN_TEST_MODEL');
  const goodA = required('JEDEN_TEST_PROVIDER_TOKEN_A');
  const goodB = required('JEDEN_TEST_PROVIDER_TOKEN_B');
  assert.notEqual(goodA, goodB, 'two distinct live provider credentials are required');
  sensitive.push(goodA, goodB, endpoint);
  const provider = new URL(endpoint);
  assert.equal(provider.protocol, 'https:', 'qualify against a real HTTPS provider');
  assert.equal(provider.username, '', 'provider endpoint must not contain credentials');
  assert.equal(provider.password, '', 'provider endpoint must not contain credentials');
  assert.equal(provider.search, '', 'provider endpoint must not contain a query');
  assert.equal(provider.hash, '', 'provider endpoint must not contain a fragment');
  report.endpointSha256 = createHash('sha256').update(endpoint).digest('hex');
  report.binarySha256 = hash(binary);
  const env = { ...process.env, HOME: home, JEDEN_MODEL_ENDPOINT: endpoint,
    JEDEN_SESSION_ROOT: sessions };
  delete env.JEDEN_MODEL_KEY;
  delete env.BRAMA_URL;
  delete env.BRAMA_TOKEN;
  delete env.WISENT_APP_AGENT_AUTH_SECRET;
  delete env.JEDEN_DATABASE_URL;
  assert.equal(succeeds(invoke('git', ['rev-parse', 'HEAD'], env)).trim(), revision);
  assert.equal(succeeds(invoke('git', ['status', '--porcelain'], env)).trim(), '',
    'qualify a committed clean source revision');
  const shortRevision = succeeds(invoke('git', ['rev-parse', '--short', revision], env)).trim();
  const version = succeeds(invoke(binary, ['--version'], env)).trim();
  assert.ok(version.endsWith(`.${shortRevision}`),
    `the binary must embed the candidate revision, not merely be named in the report: ${version}`);
  assert.deepEqual(list(binary, env), []);
  assert.equal(JSON.parse(succeeds(invoke(binary, ['token', 'status', '--json'], env))).keySource,
    'none', 'an empty store must not masquerade as a configured credential');
  refuses(invoke(binary, ['token', 'remove', randomUUID()], env), 'unknown token id');
  assert.ok(!existsSync(join(home, '.jeden/model-tokens.sqlite3')),
    'refusing an unknown removal must not create a credential store');
  const bad = `invalid-${randomUUID()}`;
  sensitive.push(bad);
  refuses(invoke(binary, ['token', 'add', 'insecure', '--endpoint', 'http://example.com'], env, bad),
    'HTTPS or loopback HTTP');
  assert.ok(!existsSync(join(home, '.jeden/model-tokens.sqlite3')),
    'an invalid endpoint must not create a credential store');
  const invalidStatus = JSON.parse(succeeds(invoke(binary, ['token', 'status', '--json'],
    { ...env, JEDEN_MODEL_ENDPOINT: 'http://example.com', JEDEN_MODEL_KEY: goodA })));
  assert.equal(invalidStatus.keySource, 'refused');
  assert.match(invalidStatus.routeRefusal, /JEDEN_MODEL_ENDPOINT: model endpoint must be HTTPS or loopback HTTP/);
  assert.equal(invalidStatus.endpoint, null, 'invalid direct URL must not identify a usable route');
  const linkedHome = join(area, 'linked-home');
  mkdirSync(linkedHome);
  symlinkSync(home, join(linkedHome, '.jeden'));
  try {
    refuses(invoke(binary, ['token', 'list'], { ...env, HOME: linkedHome }),
      'token directory');
  } finally {
    rmSync(linkedHome, { recursive: true, force: true });
  }
  const add = (label, token) => JSON.parse(succeeds(invoke(binary,
    ['token', 'add', label, '--endpoint', endpoint, '--json'], env, `${token}\n`)));
  const rejected = add('rejected', bad);
  const first = add('primary', goodA);
  const second = add('secondary', goodB);
  assert.deepEqual(list(binary, env).map((row) => row.id), [rejected.id, first.id, second.id]);
  assert.equal(JSON.parse(succeeds(invoke(binary, ['token', 'status', '--json'], env))).keySource,
    'local-token-list');
  const environment = JSON.parse(succeeds(invoke(binary, ['token', 'status', '--json'],
    { ...env, JEDEN_MODEL_KEY: goodA })));
  assert.equal(environment.keySource, 'environment');
  assert.equal(environment.selected, null, 'the process key takes precedence over the list');
  const redactFrame = (frame) => JSON.parse(JSON.stringify(frame, redactValue));
  const rpc = startRpc({ binary, cwd: root, env, report, redactFrame });
  try {
    const remote = await rpc.rpc('token/status', {});
    assert.deepEqual(remote.tokens.map((row) => row.id), [rejected.id, first.id, second.id]);
    assert.equal(remote.keySource, 'local-token-list');
    await assert.rejects(rpc.rpc('token/remove', { id: randomUUID() }),
      /credential_error: unknown token id/);
    const added = await rpc.rpc('token/add',
      { label: 'desktop', endpoint, token: goodA });
    assert.deepEqual(list(binary, env).map((row) => row.id),
      [rejected.id, first.id, second.id, added.id],
      'an RPC add must persist through the CLI on the selected host');
    await rpc.rpc('token/remove', { id: added.id });
    assert.deepEqual(list(binary, env).map((row) => row.id),
      [rejected.id, first.id, second.id],
      'an RPC removal must persist through the CLI');
  } finally {
    await rpc.end();
  }
  const environmentRpc = startRpc({ binary, cwd: root,
    env: { ...env, JEDEN_MODEL_ENDPOINT: `${endpoint.replace(/\/+$/, '')}/`, JEDEN_MODEL_KEY: goodA }, report });
  try {
    const remote = await environmentRpc.rpc('token/status', {});
    assert.equal(remote.keySource, 'environment');
    assert.equal(remote.endpoint, endpoint.replace(/\/+$/, ''),
      'RPC status reports the canonical endpoint used to match stored keys');
    assert.equal(remote.selected, null,
      'the RPC must not present a stored token as selected when the process key takes precedence');
  } finally {
    await environmentRpc.end();
  }
  refuses(invoke(binary, ['token', 'add', 'primary', '--endpoint', endpoint], env, `${goodA}\n`),
    'already exists');
  refuses(invoke(binary, ['token', 'remove', randomUUID()], env), 'unknown token id');
  const concurrent = [
    startRpc({ binary, cwd: root, env, report, redactFrame }),
    startRpc({ binary, cwd: root, env, report, redactFrame }),
  ];
  try {
    const results = await Promise.allSettled(concurrent.map(client => client.rpc('token/add',
      { label: 'concurrent', endpoint, token: goodA })));
    assert.deepEqual(results.map(result => result.status).sort(), ['fulfilled', 'rejected'],
      'only one competing add may claim a label');
    const stored = results.find(result => result.status === 'fulfilled').value;
    assert.match(results.find(result => result.status === 'rejected').reason.message,
      /credential_error: (token label concurrent already exists as|begin token insertion for concurrent:)/);
    assert.deepEqual(list(binary, env).filter(row => row.label === 'concurrent').map(row => row.id),
      [stored.id], 'the winning ID must be the only persisted token for that label');
    succeeds(invoke(binary, ['token', 'remove', stored.id], env));
  } finally {
    await Promise.all(concurrent.map(client => client.end()));
  }
  const prompt = 'Respond exactly with TOKEN_ROTATION_OK and nothing else.';
  const run = invoke(binary, ['run', prompt, '--model-only', '--cwd', workspace,
    '--model', model], { ...env, JEDEN_LEASE: '{invalid', JEDEN_MODEL_ENDPOINT: `${endpoint.replace(/\/+$/, '')}/` });
  assert.ok(!run.status && run.stdout.includes('TOKEN_ROTATION_OK'),
    `the real provider did not answer the changed path: ${run.stderr}\n${run.stdout}`);
  const after = list(binary, env);
  const refusedKey = after.find((row) => row.id === rejected.id);
  assert.ok(refusedKey?.lastFailure,
    'the refused key has no recorded failed operation or provider response');
  assert.match(refusedKey.lastFailure, /^.+: .*HTTP \d+:/,
    'the actual provider must refuse the bad key, not a local simulation');
  assert.ok(after.some((row) => row.active && row.id !== rejected.id),
    'a successful real call must persist its selected provider credential');
  const inspected = invoke(binary, ['run', prompt, '--cwd', workspace, '--model', model], env);
  assert.ok(!inspected.status && inspected.stdout.includes('TOKEN_ROTATION_OK'),
    `the direct provider did not complete intake and acceptance inspection with stored keys: ${inspected.stderr}\n${inspected.stdout}`);
  const afterInspection = list(binary, env);
  assert.ok(afterInspection.some((row) => row.active && row.id !== rejected.id),
    'completion inspection must retain a selected live direct-provider credential');
  const status = JSON.parse(succeeds(invoke(binary, ['token', 'status', '--json'], { ...env, JEDEN_MODEL_ENDPOINT: `${endpoint.replace(/\/+$/, '')}/` })));
  assert.equal(status.endpoint, endpoint.replace(/\/+$/, ''));
  assert.equal(status.keySource, 'local-token-list');
  assert.ok(status.selected, 'status must name the selected credential for its endpoint');
  const humanStatus = succeeds(invoke(binary, ['token', 'status'], env));
  assert.ok(humanStatus.includes(status.endpoint) && humanStatus.includes('local-token-list'),
    'text status must identify the selected direct provider and key source');
  for (const secret of [bad, goodA, goodB]) {
    assert.ok(!JSON.stringify(status).includes(secret), 'JSON status exposed a model credential');
    assert.ok(!humanStatus.includes(secret), 'text status exposed a model credential');
  }
  const environmentRun = invoke(binary, ['run', prompt, '--model-only',
    '--cwd', workspace, '--model', model], {
    ...env, JEDEN_MODEL_KEY: goodB, BRAMA_CATALOG_TTL_MS: 'invalid',
  });
  assert.ok(!environmentRun.status && environmentRun.stdout.includes('TOKEN_ROTATION_OK'),
    `the process-owned provider key did not answer: ${environmentRun.stderr}\n${environmentRun.stdout}`);
  assert.deepEqual(list(binary, env), afterInspection,
    'a direct run with JEDEN_MODEL_KEY must not select or mutate the stored pool');
  const deniedEnvironment = invoke(binary, ['run', prompt, '--model-only',
    '--cwd', workspace, '--model', model], { ...env, JEDEN_MODEL_KEY: bad });
  refuses(deniedEnvironment, 'HTTP');
  assert.ok(!`${deniedEnvironment.stdout}\n${deniedEnvironment.stderr}`.includes(bad),
    'the direct environment-key refusal exposed the credential');
  assert.deepEqual(list(binary, env), afterInspection,
    'a refused process key must not silently fall back to or mutate the stored pool');
  const insecureRun = invoke(binary, ['run', prompt, '--model-only',
    '--cwd', workspace, '--model', model], {
    ...env, JEDEN_MODEL_ENDPOINT: 'http://example.com', JEDEN_MODEL_KEY: goodA,
    BRAMA_URL: endpoint, BRAMA_TOKEN: goodB,
  });
  refuses(insecureRun, 'JEDEN_MODEL_ENDPOINT: model endpoint must be HTTPS or loopback HTTP');
  assert.ok(!`${insecureRun.stdout}\n${insecureRun.stderr}`.includes(goodA),
    'an insecure endpoint refusal must not expose the supplied credential');
  const embedded = new URL(endpoint);
  embedded.username = bad;
  const embeddedRun = invoke(binary, ['run', prompt, '--model-only',
    '--cwd', workspace, '--model', model], {
    ...env, JEDEN_MODEL_ENDPOINT: embedded.href, JEDEN_MODEL_KEY: goodA,
  });
  refuses(embeddedRun, 'model endpoint must be HTTPS or loopback HTTP, without credentials');
  assert.ok(!`${embeddedRun.stdout}\n${embeddedRun.stderr}`.includes(bad),
    'an invalid endpoint refusal must not expose embedded credentials');
  refuses(invoke(binary, ['run', prompt, '--model-only', '--cwd', workspace,
    '--model', 'any'], env), 'Brama virtual route');
  const virtualDoctor = JSON.parse(invoke(binary, ['doctor', '--json', '--cwd', workspace],
    { ...env, JEDEN_MODEL: 'any' }).stdout);
  const virtualRoute = virtualDoctor.probes.find((entry) => entry.subsystem === 'model');
  assert.equal(virtualRoute?.state, 'unavailable',
    'doctor must refuse a Brama virtual route selected for a direct provider');
  assert.match(virtualRoute.detail, /Brama virtual route/);
  const doctor = JSON.parse(invoke(binary, ['doctor', '--json', '--cwd', workspace],
    { ...env, JEDEN_MODEL: model, JEDEN_MODEL_ENDPOINT: `${endpoint.replace(/\/+$/, '')}/` }).stdout);
  const providerProbe = doctor.probes.find((entry) => entry.subsystem === 'model');
  assert.ok(providerProbe, 'doctor must report the direct provider as a model route, not a Brama dependency');
  assert.equal(providerProbe.state, 'healthy',
    `the selected model must resolve through the real direct catalog: ${providerProbe.detail}`);
  const setup = JSON.parse(succeeds(invoke(binary, ['run', '/setup validate', '--json', '--cwd', workspace], { ...env, JEDEN_MODEL: model })));
  assert.match(setup.text, /- model: healthy/, 'setup validation must show the observed direct-provider probe');
  const file = join(home, '.jeden/model-tokens.sqlite3');
  assert.ok(statSync(file).isFile(), 'the list must persist across CLI processes');
  assert.ok(!(statSync(file).mode & (constants.S_IRWXG | constants.S_IRWXO)),
    'the credential file must not be accessible to other users');
  chmodSync(file, constants.S_IRUSR | constants.S_IWUSR | constants.S_IRGRP);
  try {
    refuses(invoke(binary, ['token', 'list'], env), 'not a private file owned by this user');
  } finally {
    chmodSync(file, constants.S_IRUSR | constants.S_IWUSR);
  }
  const tokenDirectory = join(home, '.jeden');
  chmodSync(tokenDirectory, constants.S_IRWXU | constants.S_IRGRP | constants.S_IXGRP);
  try {
    refuses(invoke(binary, ['token', 'list'], env), 'token directory');
  } finally {
    chmodSync(tokenDirectory, constants.S_IRWXU);
  }
  for (const row of [first, second]) {
    succeeds(invoke(binary, ['token', 'remove', row.id], env));
  }
  const exhausted = invoke(binary, ['run', prompt, '--model-only', '--cwd', workspace,
    '--model', model], env);
  refuses(exhausted, 'HTTP');
  const refusal = `${exhausted.stdout}\n${exhausted.stderr}`;
  assert.doesNotMatch(refusal, /\bBrama\b/,
    'a direct provider refusal must not be attributed to Brama');
  assert.ok(!refusal.includes(bad), 'the rejected provider credential leaked in the refusal');
  const refusedDoctor = JSON.parse(invoke(binary, ['doctor', '--json', '--cwd', workspace],
    { ...env, JEDEN_MODEL: model }).stdout);
  const failedRoute = refusedDoctor.probes.find((entry) => entry.subsystem === 'model');
  assert.equal(failedRoute?.state, 'unavailable',
    'doctor must refuse the direct route when every stored credential is rejected');
  assert.match(failedRoute.detail, /HTTP \d+:/,
    'doctor must expose the real provider catalog refusal');
  assert.ok(!failedRoute.detail.includes(bad), 'doctor leaked the rejected credential');
  succeeds(invoke(binary, ['token', 'remove', rejected.id], env));
  assert.deepEqual(list(binary, env), []);
  report.verdict = 'passed';
} catch (error) {
  report.error = redacted(String(error.stack || error));
  throw error;
} finally {
  rmSync(home, { recursive: true, force: true });
  writeFileSync(join(area, 'report.json'), JSON.stringify(report, redactValue, '  '));
  console.log(`${report.verdict}: ${join(area, 'report.json')}`);
}
