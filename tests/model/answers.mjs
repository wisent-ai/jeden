// Real CLI and Brama journey. The scenario supplies prompts, not model responses.
// Required: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_ANSWER_SCENARIO.
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
const scenarioPath = resolve(required('JEDEN_TEST_ANSWER_SCENARIO'));
const scenario = JSON.parse(readFileSync(scenarioPath, 'utf8'));
assert.ok(scenario.model && Array.isArray(scenario.cases));
assert.ok(!process.env.JEDEN_MODEL_ENDPOINT, 'the journey requires the real Stado-selected Brama route');
assert.ok(!process.env.BRAMA_URL, 'an endpoint override must not replace the directory fixture');
assert.deepEqual(scenario.cases.map((entry) => entry.name).sort(), ['limited', 'recovered', 'repeated']);
const runs = join(root, '.build/answer-recovery');
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
function succeeds(result) {
  assert.ok(result.status !== null && !result.status, result.stderr);
  return result.stdout;
}
function transcripts(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? transcripts(path) : entry.name === 'transcript.jsonl' ? [path] : [];
  });
}
try {
  assert.equal(succeeds(invoke('git', ['rev-parse', 'HEAD'], root, process.env)).trim(), revision);
  assert.equal(succeeds(invoke('git', ['status', '--porcelain'], root, process.env)).trim(), '',
    'commit the candidate before qualification');
  for (const entry of scenario.cases) {
    const directory = join(output, entry.name);
    const sessions = join(directory, 'sessions');
    const workspace = join(directory, 'workspace');
    mkdirSync(sessions, { recursive: true });
    mkdirSync(workspace, { recursive: true });
    const promptPath = resolve(dirname(scenarioPath), entry.promptFile);
    const prompt = readFileSync(promptPath, 'utf8');
    const args = ['run', prompt, '--cwd', workspace, '--model', scenario.model, '--model-only', '--json'];
    if (entry.name === 'limited') {
      assert.ok(entry.maxSteps, 'limited scenario requires an explicit caller limit');
      args.push('--max-steps', String(entry.maxSteps));
    }
    const env = { ...process.env, JEDEN_SESSION_ROOT: sessions };
    const result = invoke(binary, args, workspace, env);
    const paths = transcripts(sessions);
    const events = paths.flatMap((path) => {
      const exported = succeeds(invoke(binary, ['export', dirname(path)], workspace, env));
      writeFileSync(join(dirname(path), 'qualification-export.json'), exported);
      return JSON.parse(exported).events;
    });
    const violations = events.filter((event) => event.data?.rule === 'model-answer').map((event) => event.data);
    assert.ok(violations.length, 'the real model did not exercise answer correction');
    if (entry.name === 'recovered') {
      const [first, second] = violations;
      assert.equal(first?.outcome, 'requested');
      assert.equal(second?.outcome, 'requested', 'changing failures must continue beyond the former correction count');
      assert.ok(violations.every((entry) => !entry.repeatedFailure && !entry.cutOff));
      assert.ok(entry.expectedText, 'recovered scenario requires the expected final answer');
      succeeds(result);
      assert.ok(events.some((event) => event.type === 'final' && event.data.text.includes(entry.expectedText)));
    } else {
      assert.ok(result.status !== null && result.status, 'unrecoverable real answer must refuse');
      const refusal = violations.findLast((entry) => entry.outcome === 'rejected');
      assert.ok(refusal, 'the observed refusal must be retained');
      assert.ok(events.some((event) => event.type === 'run_error' && event.data.message));
      if (entry.name === 'repeated') {
        assert.equal(refusal.repeatedFailure, true);
        assert.ok(events.some((event) => event.type === 'run_error'
          && event.data.message.includes('same failure recurred after correction')));
      } else {
        assert.equal(refusal.repeatedFailure, false);
        assert.equal(refusal.step, Number(entry.maxSteps));
      }
    }
    report.cases.push({ name: entry.name, promptSha256: hash(promptPath), paths, violations, verdict: 'passed' });
  }
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
