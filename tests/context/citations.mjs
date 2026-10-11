// Real index citations and HTTP refusal; no index rows or provider replies are fabricated.
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
const binary = resolve(required('JEDEN_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const endpoint = required('JEDEN_TEST_GROUND_TRUTH_URL').replace(/\/+$/, '');
const query = required('JEDEN_TEST_GROUND_TRUTH_QUERY');
const limit = required('JEDEN_TEST_CONTEXT_LIMIT');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const runs = join(root, '.build/context-citations');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const workspace = join(output, 'workspace');
const home = join(output, 'home');
const config = join(workspace, '.jeden/config.json');
const hash = path => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { startedAt: new Date().toISOString(), revision, binary, endpoint, query, limit,
  binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  commands: [], http: [], cases: [], verdict: 'failed' };
const persist = () => writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
const env = { ...process.env, HOME: home };
function command(program, args, cwd = workspace) {
  const result = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, args, cwd, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  persist();
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.ok(result.status !== null && !result.status, result.stderr);
  return result.stdout.trim();
}
function select(url) {
  writeFileSync(config, JSON.stringify({ context: { advisor: { groundTruthUrl: url } } }));
}
function advice() {
  return JSON.parse(command(binary, ['context', 'recommend', query, '--limit', limit,
    '--source', 'ground-truth', '--json', '--cwd', workspace]));
}
async function search(url) {
  const target = new URL(`${url}/search`);
  target.searchParams.set('q', query);
  target.searchParams.set('limit', limit);
  const observation = { url: target.href, startedAt: new Date().toISOString() };
  report.http.push(observation);
  persist();
  try {
    const response = await fetch(target);
    Object.assign(observation, { status: response.status, statusText: response.statusText,
      body: await response.text(), finishedAt: new Date().toISOString() });
    persist();
    return { response, observation };
  } catch (error) {
    observation.error = String(error.stack || error);
    persist();
    throw error;
  }
}
const locator = row => `${row.repo}/${row.path}@${row.commitSha}:${row.lineStart}-${row.lineEnd}`;
async function cited() {
  const { response, observation } = await search(endpoint);
  assert.ok(response.ok, JSON.stringify(observation));
  const rows = JSON.parse(observation.body).results;
  assert.ok(Array.isArray(rows), 'the real index must return results');
  assert.ok(rows.some(row => row.repo && row.path && row.commitSha),
    'choose a query with actual cited index content');
  const result = advice();
  const state = result.sources.find(source => source.source === 'ground-truth');
  assert.equal(state?.available, true, JSON.stringify(result));
  assert.deepEqual(result.recommendations.map(hit => hit.locator).sort(),
    rows.slice(undefined, Number(limit)).map(locator).sort(),
    'native recommendations must preserve the actual index citation coordinates');
  assert.ok(result.recommendations.every(hit => hit.source === 'ground-truth'));
  assert.equal(result.unanswered.some(source => source.source === 'ground-truth'), false);
  report.cases.push({ case: 'exact-index-citations', state });
}

try {
  mkdirSync(dirname(config), { recursive: true });
  mkdirSync(home);
  report.checkoutRevision = command('git', ['rev-parse', 'HEAD'], root);
  assert.equal(report.checkoutRevision, revision);
  assert.equal(command('git', ['status', '--porcelain'], root), '',
    'commit the candidate and journey before qualification');
  const short = command('git', ['rev-parse', '--short', revision], root);
  report.binaryVersion = command(binary, ['--version']);
  assert.ok(report.binaryVersion.endsWith(`.${short}`), 'candidate must embed the clean source revision');
  select(endpoint);
  await cited();

  // The real server, not a test responder, decides the status of this undeclared route.
  const absentRoute = `${endpoint}/${revision}`;
  const { response, observation } = await search(absentRoute);
  assert.equal(response.ok, false, 'the selected index must refuse an undeclared route');
  select(absentRoute);
  const refused = advice();
  const failure = refused.sources.find(source => source.source === 'ground-truth');
  assert.equal(failure?.available, false, JSON.stringify(refused));
  assert.ok(failure.detail.includes(`${absentRoute}/search`), failure.detail);
  assert.ok(failure.detail.includes(String(observation.status)), failure.detail);
  assert.deepEqual(refused.recommendations, []);
  assert.ok(refused.unanswered.some(source => source.source === 'ground-truth'));
  report.cases.push({ case: 'real-http-refusal', failure });

  select(endpoint);
  await cited();
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  report.finishedAt = new Date().toISOString();
  persist();
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
