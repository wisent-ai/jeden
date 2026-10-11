// Real CLI test: agent discovery refuses a definitions directory entry it
// cannot list, naming it, instead of skipping it, which made every agent it
// held vanish from delegation without a word. `/agents show default` runs
// through the native CLI against an isolated HOME and project under .build.
// The unlistable entry is a named pipe inside the project's .jeden/agents:
// it exists, is not a definition file, and is not a directory that can be
// listed. Once it is removed the same command shows the bundled agent.
// Every command, its exit status and its output are kept in report.json.
//
// Run: JEDEN_BIN=target/debug/jeden JEDEN_TEST_SOURCE_REVISION=$(git rev-parse HEAD) \
//   node tests/agents/unlistable_definitions.mjs
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
const runs = join(root, '.build/agents-unlistable-definitions');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  commands: [], verdict: 'failed' };

const home = join(output, 'home');
const project = join(output, 'project');
const agents = join(project, '.jeden', 'agents');
const pipe = join(agents, 'team');
mkdirSync(home, { recursive: true });
mkdirSync(agents, { recursive: true });
const env = { ...process.env, HOME: home };

function run(program, args, cwd) {
  const result = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, args, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.signal, null, `${program} must not end by signal`);
  return result;
}
function showDefault() {
  return run(binary, ['run', '/agents show default', '--json', '--cwd', project], project);
}

try {
  const made = run('mkfifo', [pipe], project);
  assert.ok(made.status !== null && !made.status, made.stderr);
  const refused = showDefault();
  assert.ok(refused.status, `/agents show accepted an unlistable entry: ${refused.stdout}`);
  assert.ok(refused.stderr.includes(`cannot list agent definitions in ${pipe}`),
    `the unlistable entry was not named: ${refused.stderr}`);

  rmSync(pipe);
  const shown = showDefault();
  assert.ok(shown.status !== null && !shown.status, shown.stderr);
  assert.ok(JSON.parse(shown.stdout).text.includes('"id": "default"'), shown.stdout);
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
