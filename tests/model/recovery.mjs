// Run the real CLI against real Brama routes, never a simulated model server.
// JEDEN_TEST_RECOVERY_SCENARIO names JSON with model, promotions (route objects),
// promptFile and expectedText. The prompt must exceed each route before the last
// promotion; the last route must really accept it. Use enough distinct routes
// to exercise the removed recovery ceiling. A missing real setup is a failure.
// JEDEN_BIN and JEDEN_TEST_SOURCE_REVISION bind the report to the candidate.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
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
const scenarioPath = resolve(required('JEDEN_TEST_RECOVERY_SCENARIO'));
const scenario = JSON.parse(readFileSync(scenarioPath, 'utf8'));
assert.ok(scenario.model && scenario.expectedText && scenario.promptFile);
assert.ok(Array.isArray(scenario.promotions) && scenario.promotions.length);
const prompt = readFileSync(resolve(dirname(scenarioPath), scenario.promptFile), 'utf8');
const runs = join(root, '.build/model-recovery');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), scenarioSha256: hash(scenarioPath),
  testSha256: hash(fileURLToPath(import.meta.url)), commands: [], cases: [], verdict: 'failed' };
function invoke(program, args, cwd, env) {
  const result = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, args, cwd, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  return result;
}
function transcripts(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? transcripts(path) : entry.name === 'transcript.jsonl' ? [path] : [];
  });
}
try {
  const source = invoke('git', ['rev-parse', 'HEAD'], root, process.env);
  assert.ok(source.status !== null && !source.status, source.stderr);
  assert.equal(source.stdout.trim(), revision);
  const state = invoke('git', ['status', '--porcelain'], root, process.env);
  assert.ok(state.status !== null && !state.status, state.stderr);
  assert.equal(state.stdout.trim(), '', 'commit the candidate before qualification');
  for (const success of [true, false]) {
    const directory = join(output, success ? 'promoted' : 'exhausted');
    const home = join(directory, 'home');
    const workspace = join(directory, 'workspace');
    const config = join(workspace, '.jeden');
    mkdirSync(home, { recursive: true });
    mkdirSync(config, { recursive: true });
    const promotions = [...scenario.promotions];
    if (!success) promotions.pop();
    writeFileSync(join(config, 'config.json'), JSON.stringify({ modelRouting: {
      contextPromotions: promotions, fallbacks: [],
    } }));
    const env = { ...process.env, HOME: home };
    delete env.JEDEN_MODEL_ENDPOINT;
    const result = invoke(binary, ['run', prompt, '--cwd', workspace,
      '--model', scenario.model, '--model-only', '--json'], workspace, env);
    const paths = transcripts(home);
    const events = paths.flatMap((path) => {
      const exported = invoke(binary, ['export', dirname(path)], workspace, env);
      assert.ok(exported.status !== null && !exported.status, exported.stderr);
      writeFileSync(join(dirname(path), 'qualification-export.json'), exported.stdout);
      return JSON.parse(exported.stdout).events;
    });
    const changes = events.filter((event) => event.type === 'model_route_result'
      && event.data?.result?.reason === 'context promotion').map((event) => event.data.result.to);
    assert.deepEqual(changes, promotions, 'every declared promotion must really be consumed, in order');
    if (success) {
      assert.ok(result.status !== null && !result.status, result.stderr);
      assert.ok(result.stdout.includes(scenario.expectedText), 'the accepting real model did not answer');
    } else {
      assert.ok(result.status !== null && result.status, 'exhausted real routes must refuse');
      assert.ok(events.some((event) => event.type === 'run_error' && event.data?.message),
        'the terminal model failure must be retained');
    }
    report.cases.push({ success, paths, promotions, verdict: 'passed' });
  }
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
