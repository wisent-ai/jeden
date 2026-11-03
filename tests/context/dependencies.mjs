// Exercises the real source-all advisor, including the fleet memory database.
// No provider, network server or child process is simulated. An isolated
// SQLite database in the ignored build directory is the real memory store.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { chmodSync, constants, mkdirSync, mkdtempSync, readFileSync, statSync, unlinkSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { constants as system } from 'node:os';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
}
const binary = resolve(required('JEDEN_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const limit = required('JEDEN_TEST_CONTEXT_LIMIT');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const runs = join(root, '.build/context-dependencies');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const workspace = join(output, 'workspace');
const home = join(output, 'home');
mkdirSync(home);
const databaseFile = join(output, 'memory.sqlite3');
const journeyEnv = { ...process.env, HOME: home,
  JEDEN_DATABASE_URL: `sqlite://${databaseFile}?mode=rwc` };
mkdirSync(join(workspace, '.jeden'), { recursive: true });
const query = `advisorjourney${randomUUID().replaceAll('-', '')}`;
const fixture = join(workspace, 'entry.rs');
const fixtureText = `pub fn ${query}() {}`;
writeFileSync(fixture, `${fixtureText}\n`);
writeFileSync(join(workspace, '.jeden/config.json'), JSON.stringify({
  context: { advisor: { roots: workspace, fileExtensions: 'rs' } },
}));
const expectedMemory = []; // The new repo scope has never written a memory.
const hash = path => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = {
  startedAt: new Date().toISOString(), binary, declaredSourceRevision: revision,
  binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  query, workspace, databaseFile, commands: [], cases: [], verdict: 'failed',
};
const persist = () => writeFileSync(join(output, 'report.json'), JSON.stringify(report));
persist();

function invoke(program, args, cwd = workspace) {
  const record = { program, args, cwd, startedAt: new Date().toISOString() };
  report.commands.push(record);
  const stdoutPath = join(output, `${report.commands.length}.stdout`);
  const stderrPath = join(output, `${report.commands.length}.stderr`);
  return new Promise((resolveResult, reject) => {
    const child = spawn(program, args, { cwd, env: journeyEnv, stdio: ['ignore', 'pipe', 'pipe'] });
    record.pid = child.pid;
    persist();
    const stdout = [];
    const stderr = [];
    child.stdout.on('data', chunk => stdout.push(chunk));
    child.stderr.on('data', chunk => stderr.push(chunk));
    child.on('error', error => { record.error = error.message; reject(error); });
    child.on('close', (status, signal) => {
      Object.assign(record, { status, signal, finishedAt: new Date().toISOString(), stdoutPath, stderrPath });
      const result = { ...record, stdout: Buffer.concat(stdout).toString(), stderr: Buffer.concat(stderr).toString() };
      writeFileSync(stdoutPath, result.stdout);
      writeFileSync(stderrPath, result.stderr);
      persist();
      resolveResult(result);
    });
  });
}
function succeeds(result) {
  assert.ok(result.status !== null && !result.status, `${result.program}: ${result.stderr}`);
  assert.equal(result.signal, null);
  return result.stdout;
}

try {
  report.checkoutRevision = succeeds(await invoke('git', ['rev-parse', 'HEAD'], root)).trim();
  assert.equal(report.checkoutRevision, revision, 'qualify the declared source revision');
  report.binaryVersion = succeeds(await invoke(binary, ['--version'], root)).trim();
  assert.equal(succeeds(await invoke('git', ['status', '--porcelain'], root)).trim(), '',
    'commit the candidate and test before qualification');
  const shortRevision = succeeds(await invoke('git', ['rev-parse', '--short', revision], root)).trim();
  assert.ok(report.binaryVersion.endsWith(`.${shortRevision}`),
    `the binary must embed the clean candidate revision, not merely a declared revision: ${report.binaryVersion}`);
  const args = ['context', 'recommend', query, '--limit', limit, '--source', 'all', '--json', '--cwd', workspace];
  const results = await Promise.all([invoke(binary, args), invoke(binary, args)]);
  for (const result of results) {
    const advice = JSON.parse(succeeds(result));
    assert.equal(advice.query, query);
    const statuses = Object.fromEntries(advice.sources.map(source => [source.source, source]));
    assert.equal(statuses.memory.available, true, `the real memory consumer failed: ${statuses.memory.detail}`);
    assert.equal(statuses.memory.returned, expectedMemory.length, 'a new repo scope must not expose another workspace memory');
    assert.ok(statuses.memory.detail.includes(`repo:${workspace}`), statuses.memory.detail);
    assert.equal(statuses.files.available, true, statuses.files.detail);
    const match = advice.recommendations.find(hit => hit.source === 'files');
    assert.ok(match?.locator.startsWith(`${fixture}:`), JSON.stringify(advice));
    assert.equal(match.snippet, fixtureText);
    assert.deepEqual(advice.recommendations.filter(hit => hit.source === 'memory'), expectedMemory);
    assert.equal(advice.unanswered.some(source => source.source === 'memory'), false,
      'a successful empty memory search must not be reported as unanswered');
    assert.equal(statuses.transcripts.available, true, statuses.transcripts.detail);
    assert.equal(typeof statuses['ground-truth'].available, 'boolean');
    assert.match(result.stderr, /czekam: .*rodzaj: baza;/, 'the database operation must identify its wait');
    report.cases.push({ pid: result.pid, case: 'concurrent-source-all', verdict: 'passed' });
  }
  const refused = await invoke(binary, ['context', 'recommend', query, '--limit', limit, '--source', 'not-a-source', '--json', '--cwd', workspace]);
  assert.ok(refused.status !== null && refused.status, 'a source typo must be refused');
  assert.match(refused.stderr, /unknown source\(s\): not-a-source/);
  assert.equal(refused.stdout.trim(), '', 'a source typo must not return narrowed advice');
  report.cases.push({ case: 'unknown-source-refusal', verdict: 'passed' });
  const probe = await invoke(binary, ['context', 'sources', '--json', '--cwd', workspace]);
  const inspection = JSON.parse(succeeds(probe));
  for (const source of inspection.sources) {
    assert.ok(probe.stderr.includes(`context advisor probe ${source.source};`),
      `source inspection hid its pending operation: ${source.source}`);
    assert.equal(typeof source.available, 'boolean');
    if (!source.available) {
      assert.ok(source.detail.trim(), `source refusal omitted its cause: ${source.source}`);
      assert.ok(probe.stderr.includes(`blad czekania: context advisor probe ${source.source};`), probe.stderr);
    }
  }
  const memoryProbe = inspection.sources.find(source => source.source === 'memory');
  assert.equal(memoryProbe?.available, true, JSON.stringify(inspection));
  assert.ok(statSync(databaseFile).isFile(),
    'a real memory query must create its isolated persistent database file');
  report.cases.push({ case: 'real-source-inspection', verdict: 'passed', result: inspection });
  const originalMode = statSync(fixture).mode;
  const originalBytes = readFileSync(fixture);
  let denial;
  const companion = join(workspace, 'readable.rs');
  const companionText = `pub fn ${query}_readable() {}`;
  let companionCreated = false;
  try {
    chmodSync(fixture, originalMode & ~(constants.S_IRUSR | constants.S_IRGRP | constants.S_IROTH));
    try { readFileSync(fixture); } catch (error) { denial = error; }
    assert.equal(denial?.errno, -system.errno.EACCES,
      'qualification requires an actual filesystem permission refusal, not a privileged readable fixture');
    const refusedRead = await invoke(binary, ['context', 'recommend', query,
      '--limit', limit, '--source', 'files', '--json', '--cwd', workspace]);
    const advice = JSON.parse(succeeds(refusedRead));
    const source = advice.sources.find(source => source.source === 'files');
    assert.equal(source?.available, false, JSON.stringify(advice));
    assert.ok(source.detail.includes(fixture) && source.detail.includes(`os error ${-denial.errno}`), source.detail);
    assert.ok(advice.unanswered.some(source => source.source === 'files'), JSON.stringify(advice));
    report.cases.push({ case: 'unreadable-files-source', verdict: 'passed', refusal: source });
    writeFileSync(companion, `${companionText}\n`);
    companionCreated = true;
    const partialRead = await invoke(binary, ['context', 'recommend', query,
      '--limit', limit, '--source', 'files', '--json', '--cwd', workspace]);
    const partial = JSON.parse(succeeds(partialRead));
    const partialSource = partial.sources.find(source => source.source === 'files');
    assert.equal(partialSource?.available, false, JSON.stringify(partial));
    assert.ok(partialSource.detail.includes(fixture)
      && partialSource.detail.includes(`os error ${-denial.errno}`), partialSource.detail);
    assert.ok(partial.recommendations.some(hit => hit.source === 'files'
      && hit.locator.startsWith(`${companion}:`) && hit.snippet === companionText),
    'a failed file must not discard the readable source recommendation');
    assert.ok(partialRead.stderr.includes('blad czekania: context advisor source files;'), partialRead.stderr);
    const partialProbe = await invoke(binary, ['context', 'sources', '--json', '--cwd', workspace]);
    const partialInspection = JSON.parse(succeeds(partialProbe));
    const inspectedFiles = partialInspection.sources.find(source => source.source === 'files');
    assert.equal(inspectedFiles?.available, false, JSON.stringify(partialInspection));
    assert.ok(inspectedFiles.detail.includes(fixture)
      && inspectedFiles.detail.includes(`os error ${-denial.errno}`), inspectedFiles.detail);
    assert.ok(partialProbe.stderr.includes('blad czekania: context advisor probe files;'), partialProbe.stderr);
    report.cases.push({ case: 'partial-files-source-refusal', verdict: 'passed', refusal: partialSource, probe: inspectedFiles });
  } finally {
    chmodSync(fixture, originalMode);
    if (companionCreated) unlinkSync(companion);
  }
  const restoredRead = await invoke(binary, ['context', 'recommend', query,
    '--limit', limit, '--source', 'files', '--json', '--cwd', workspace]);
  const restored = JSON.parse(succeeds(restoredRead));
  assert.equal(restored.sources.find(source => source.source === 'files')?.available, true, JSON.stringify(restored));
  assert.equal(restored.recommendations.find(hit => hit.source === 'files')?.snippet, fixtureText);
  assert.deepEqual(readFileSync(fixture), originalBytes, 'advisor changed the recovered source');
  report.cases.push({ case: 'restored-files-source', verdict: 'passed' });
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  report.finishedAt = new Date().toISOString();
  persist();
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
