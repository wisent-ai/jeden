// Real CLI test: `jeden` refuses to start while a configuration layer that
// applies here cannot be read, does not parse, has a root that is not an
// object, or holds values the settings cannot take, naming the file and the
// cause. Read as empty, such a layer dropped every setting in it (the model
// route, task limits, the Tama registry, disabled extensions) without a word.
// Each case runs the built binary against an isolated HOME and project under
// .build, so the operator's own configuration is never read or touched; once
// the layer is valid the same command answers with its setting. Every command,
// its exit status and its output are kept in the run's report.json.
//
// Run: JEDEN_BIN=target/debug/jeden JEDEN_TEST_SOURCE_REVISION=$(git rev-parse HEAD) \
//   node tests/cli/config_unreadable_layer.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
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
const runs = join(root, '.build/config-unreadable-layer');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  commands: [], verdict: 'failed' };

const home = join(output, 'home');
const project = join(output, 'project');
const projectLayer = join(project, '.jeden', 'config.json');
const userLayer = join(home, '.jeden', 'config.yml');
mkdirSync(dirname(projectLayer), { recursive: true });
mkdirSync(dirname(userLayer), { recursive: true });
const env = { ...process.env, HOME: home };
delete env.JEDEN_MODEL;
delete env.JEDEN_LANGUAGE;

function invoke(args) {
  const result = spawnSync(binary, args, { cwd: project, env, encoding: 'utf8' });
  report.commands.push({ args, cwd: project, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  return result;
}
function refuses(words) {
  const result = invoke(['config', 'get', 'model']);
  assert.ok(result.status !== null && result.status, `expected a refusal naming ${words}`);
  assert.ok(result.stderr.includes(words), `refusal ${result.stderr} does not say ${words}`);
  assert.equal(result.stdout, '', 'the refused command answered before refusing');
}

try {
  writeFileSync(projectLayer, '{"model": "half-written');
  refuses(`configuration ${projectLayer} is not valid`);

  writeFileSync(projectLayer, '["a list, not settings"]');
  refuses(`configuration ${projectLayer} is not valid: its root must be an object`);

  writeFileSync(projectLayer, '{"models": "not a list of models"}');
  refuses(`configuration ${projectLayer} does not hold valid settings`);

  // A layer path that exists but cannot be read as a file.
  rmSync(projectLayer);
  mkdirSync(projectLayer);
  refuses(`configuration ${projectLayer} cannot be read`);
  rmSync(projectLayer, { recursive: true });

  writeFileSync(userLayer, '- a yaml list\n- not settings\n');
  refuses(`configuration ${userLayer} is not valid: its root must be an object`);

  writeFileSync(userLayer, 'model: user-route\n');
  writeFileSync(projectLayer, '{"model": "project-route"}');
  const answered = invoke(['config', 'get', 'model']);
  assert.ok(answered.status !== null && !answered.status, answered.stderr);
  assert.ok(answered.stdout.includes('project-route'), `the project layer was not read: ${answered.stdout}`);
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
