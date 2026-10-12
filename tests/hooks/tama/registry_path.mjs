// Real CLI test: a Tama hook registry named explicitly (JEDEN_TAMA_REGISTRY
// or hooks.tamaRegistry) that is not a file is refused naming the path and
// where it was named, instead of being read as "no Tama hooks"; read that way,
// a typo turned off every Tama guard without a word. `/hooks`, run through
// `jeden run`, shows the refusal; an empty value still disables the registry
// and a real registry file is read. HOME and the project are isolated under
// .build. Every command, its exit status and its output are kept in
// report.json.
//
// Run: JEDEN_BIN=target/debug/jeden JEDEN_TEST_SOURCE_REVISION=$(git rev-parse HEAD) \
//   node tests/hooks/tama/registry_path.mjs
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
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const binary = resolve(required('JEDEN_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const runs = join(root, '.build/hooks-tama-registry-path');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  commands: [], verdict: 'failed' };

const home = join(output, 'home');
const project = join(output, 'project');
mkdirSync(home, { recursive: true });
mkdirSync(project, { recursive: true });

function hooks(registry) {
  const env = { ...process.env, HOME: home, JEDEN_TAMA_REGISTRY: registry };
  const args = ['run', '/hooks', '--json', '--cwd', project];
  const result = spawnSync(binary, args, { cwd: project, env, encoding: 'utf8' });
  report.commands.push({ args, registry, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.signal, null, '/hooks must not end by signal');
  assert.ok(result.status !== null && !result.status, result.stderr);
  return JSON.parse(result.stdout).text;
}

try {
  const missing = join(output, 'no-such-registry.json');
  const refused = hooks(missing);
  assert.ok(refused.includes('Tama registry unusable'), `a missing registry was not refused: ${refused}`);
  assert.ok(refused.includes(`the Tama registry ${missing} named by JEDEN_TAMA_REGISTRY is not a file`),
    `the refusal does not name the path and its source: ${refused}`);

  const disabled = hooks('');
  assert.ok(!disabled.includes('Tama registry'), `an empty value did not disable the registry: ${disabled}`);

  const registry = join(output, 'registry.json');
  writeFileSync(registry, JSON.stringify({ managedBy: 'tama', events: {} }));
  const read = hooks(registry);
  assert.ok(!read.includes('unusable') && !read.includes('unreadable'),
    `a real registry was refused: ${read}`);
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
