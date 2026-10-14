// Qualifies source isolation against the real fleet memory database and a real file.
// Inputs: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_CONTEXT_LIMIT.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
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
const limit = required('JEDEN_TEST_CONTEXT_LIMIT');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const runs = join(root, '.build/context-independent');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const workspace = join(output, 'workspace');
mkdirSync(join(workspace, '.jeden'), { recursive: true });
const query = `independent${randomUUID().replaceAll('-', '')}`;
const fixture = join(workspace, 'entry.rs');
const fixtureText = `pub fn ${query}() {}`;
writeFileSync(fixture, `${fixtureText}\n`);
writeFileSync(join(workspace, '.jeden/config.json'), JSON.stringify({
  context: { advisor: { roots: workspace, fileExtensions: 'rs' } },
}));
const hash = path => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = {
  declaredSourceRevision: revision, binarySha256: hash(binary),
  testSha256: hash(fileURLToPath(import.meta.url)), workspace,
  commands: [], verdict: 'failed',
};
const persist = () => writeFileSync(join(output, 'report.json'), JSON.stringify(report));
persist();

function invoke(program, args, cwd = workspace) {
  const record = { program, args, cwd };
  report.commands.push(record);
  persist();
  return new Promise((resolveResult, reject) => {
    const child = spawn(program, args, { cwd, env: process.env, stdio: ['ignore', 'pipe', 'pipe'] });
    record.pid = child.pid;
    const stdout = [];
    const stderr = [];
    child.stdout.on('data', chunk => stdout.push(chunk));
    child.stderr.on('data', chunk => stderr.push(chunk));
    child.on('error', error => { record.error = error.message; persist(); reject(error); });
    child.on('close', (status, signal) => {
      Object.assign(record, { status, signal });
      record.stdoutPath = join(output, `${report.commands.indexOf(record)}.stdout`);
      record.stderrPath = join(output, `${report.commands.indexOf(record)}.stderr`);
      const result = { ...record, stdout: Buffer.concat(stdout).toString(), stderr: Buffer.concat(stderr).toString() };
      writeFileSync(record.stdoutPath, result.stdout);
      writeFileSync(record.stderrPath, result.stderr);
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
  assert.equal(report.checkoutRevision, revision);
  assert.equal(succeeds(await invoke('git', ['status', '--porcelain'], root)).trim(), '',
    'qualify the clean candidate revision');
  const version = succeeds(await invoke(binary, ['--version'], root)).trim();
  report.binaryVersion = version;
  const shortRevision = succeeds(await invoke('git', ['rev-parse', '--short', revision], root)).trim();
  assert.ok(version.endsWith(`.${shortRevision}`), `binary does not embed the candidate: ${version}`);
  const result = await invoke(binary, ['context', 'recommend', query, '--limit', limit,
    '--source', 'files,memory,transcripts', '--json', '--cwd', workspace]);
  const advice = JSON.parse(succeeds(result));
  const statuses = Object.fromEntries(advice.sources.map(source => [source.source, source]));
  assert.equal(statuses.files.available, true, JSON.stringify(advice));
  assert.equal(statuses.transcripts.available, true, JSON.stringify(advice));
  const file = advice.recommendations.find(hit => hit.source === 'files');
  assert.ok(file?.locator.startsWith(`${fixture}:`), JSON.stringify(advice));
  assert.equal(file.snippet, fixtureText);
  assert.match(result.stderr, /context advisor source files/);
  assert.match(result.stderr, /context advisor source memory/);
  assert.match(result.stderr, /context advisor source transcripts/);
  assert.equal(statuses.memory.available, false,
    `the registry answered; run this journey during a real unanswering registry operation: ${JSON.stringify(advice)}`);
  assert.ok(statuses.memory.detail.includes('stado database resolve jeden'), statuses.memory.detail);
  assert.ok(statuses.memory.detail.includes('Stado resolver state')
    || statuses.memory.detail.includes('resolver-declared'), statuses.memory.detail);
  assert.ok(advice.unanswered.some(source => source.source === 'memory'
    && source.detail === statuses.memory.detail), JSON.stringify(advice));
  assert.match(result.stderr, /blad czekania: context advisor source memory/);
  report.registryOutcome = 'unavailable';
  report.sources = advice.sources;
  const probe = await invoke(binary, ['context', 'sources', '--json', '--cwd', workspace]);
  const inspection = JSON.parse(succeeds(probe));
  const probedFiles = inspection.sources.find(source => source.source === 'files');
  const probedMemory = inspection.sources.find(source => source.source === 'memory');
  assert.equal(probedFiles?.available, true, JSON.stringify(inspection));
  assert.equal(probedMemory?.available, false,
    `the real registry answered during the source probe: ${JSON.stringify(inspection)}`);
  assert.ok(probedMemory.detail.includes('stado database resolve jeden'), probedMemory.detail);
  assert.match(probe.stderr, /blad czekania: context advisor probe memory/);
  report.probe = inspection.sources;
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  persist();
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
