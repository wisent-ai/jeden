// Real CLI configuration and model calls; no model response is simulated.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
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
const model = required('JEDEN_TEST_MODEL');
const maxSteps = required('JEDEN_TEST_MAX_STEPS');
const threshold = Number(required('JEDEN_TEST_SECRET_MIN_LENGTH'));
const secret = 'qx';
assert.ok(Number.isSafeInteger(threshold) && threshold > secret.length,
  'declare a minimum byte length above the test secret length');
const cases = [
  { name: 'default', minimum: null, reset: false, values: [], redact: true },
  { name: 'threshold', minimum: threshold, reset: false, values: [], redact: false },
  { name: 'explicit', minimum: threshold, reset: false, values: [secret], redact: true },
  { name: 'reset', minimum: threshold, reset: true, values: [], redact: true },
];
const runs = join(root, '.build/secret-discovery');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  model, threshold, commands: [], cases: [], verdict: 'failed' };
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
  for (const entry of cases) {
    const directory = join(output, entry.name);
    const home = join(directory, 'home');
    const workspace = join(directory, 'workspace');
    const config = join(workspace, '.jeden');
    mkdirSync(home, { recursive: true });
    mkdirSync(config, { recursive: true });
    const replacement = `redacted-${randomUUID()}`;
    writeFileSync(join(config, 'config.json'), JSON.stringify({ secrets: { replacement, values: entry.values } }));
    const env = { ...process.env, HOME: home, JEDEN_QUALIFICATION_SECRET: secret };
    delete env.JEDEN_MODEL_ENDPOINT;
    if (entry.minimum !== null) succeeds(invoke(binary,
      ['config', 'set', 'secrets.minLength', String(entry.minimum)], workspace, env));
    if (entry.reset) succeeds(invoke(binary, ['config', 'reset', 'secrets.minLength'], workspace, env));
    const setting = JSON.parse(succeeds(invoke(binary,
      ['config', 'get', 'secrets.minLength', '--json'], workspace, env)));
    assert.equal(setting.value, entry.reset ? null : entry.minimum);
    for (const invalid of ['-1', '0.5', 'not-a-length']) {
      const refused = invoke(binary, ['config', 'set', 'secrets.minLength', invalid], workspace, env);
      assert.ok(refused.status !== null && refused.status, 'invalid threshold must refuse');
      const after = JSON.parse(succeeds(invoke(binary,
        ['config', 'get', 'secrets.minLength', '--json'], workspace, env)));
      assert.equal(after.value, setting.value, 'refused setting must preserve persisted policy');
    }
    const prompt = `Return a final answer containing only the exact value between these delimiters, without the delimiters: <candidate>${secret}</candidate>. Do not use tools.`;
    const result = invoke(binary, ['run', prompt, '--cwd', workspace, '--model', model,
      '--model-only', '--max-steps', maxSteps, '--json'], workspace, env);
    succeeds(result);
    const paths = transcripts(home);
    const events = paths.flatMap((path) => {
      const exportPath = join(dirname(path), 'qualification-export.json');
      succeeds(invoke(binary, ['export', dirname(path), exportPath], workspace, env));
      return JSON.parse(readFileSync(exportPath, 'utf8')).events;
    });
    const expected = entry.redact ? replacement : secret;
    assert.ok(events.some((event) => event.type === 'final' && event.data.text.trim() === expected),
      'the real model must answer with the protected or explicitly unprotected candidate');
    assert.ok(events.some((event) => JSON.stringify(event.data).includes(prompt)),
      'local transcript must retain the original input rather than rewriting history');
    report.cases.push({ name: entry.name, paths, expected, verdict: 'passed' });
  }
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
