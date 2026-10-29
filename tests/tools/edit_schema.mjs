// Real CLI test: `jeden tools --json` offers the model the `edit_file` ops it
// accepts, one declared list, so a model is never left to guess an op name or
// to send a range edit without its end. The binary runs against an isolated
// HOME and project under .build; every command, its exit status and its output
// are kept in the run's report.json.
//
// Run: JEDEN_BIN=target/debug/jeden JEDEN_TEST_SOURCE_REVISION=$(git rev-parse HEAD) \
//   node tests/tools/edit_schema.mjs
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
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = resolve(required('JEDEN_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const runs = join(root, '.build/tools-edit-schema');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  commands: [], verdict: 'failed' };

const home = join(output, 'home');
const project = join(output, 'project');
mkdirSync(home, { recursive: true });
mkdirSync(project, { recursive: true });
const env = { ...process.env, HOME: home };

function invoke(args) {
  const result = spawnSync(binary, args, { cwd: project, env, encoding: 'utf8' });
  report.commands.push({ args, cwd: project, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  return result;
}

try {
  const listed = invoke(['tools', '--json']);
  assert.equal(listed.status, 0, listed.stderr);
  const tools = JSON.parse(listed.stdout);
  const edit = tools.find((tool) => tool.name === 'edit_file');
  assert.ok(edit, 'jeden tools --json lists no edit_file tool');
  const text = JSON.stringify(edit.input);
  for (const op of ['replace', 'delete', 'insert_before', 'insert_after']) {
    assert.ok(text.includes(`"${op}"`), `edit_file schema ${text} does not declare the op ${op}`);
  }
  assert.ok(text.includes('"enum"'), `edit_file schema ${text} declares no closed list of ops`);
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
}
