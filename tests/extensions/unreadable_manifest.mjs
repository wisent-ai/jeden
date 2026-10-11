// Real CLI test: an extension package whose package.json exists but does not
// parse, or declares an extension that is not a module file, is refused by
// name; `/extensions` answers the manifest path and the cause instead of
// listing the registry as if the package had declared nothing. Skipped, every
// extension the package declared silently did not load. A package that
// declares an existing module is listed. HOME and the project are isolated
// under .build; every command, its exit status and its output are kept in
// report.json.
//
// Run: JEDEN_BIN=target/debug/jeden JEDEN_TEST_SOURCE_REVISION=$(git rev-parse HEAD) \
//   node tests/extensions/unreadable_manifest.mjs
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
const runs = join(root, '.build/extensions-unreadable-manifest');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  commands: [], verdict: 'failed' };

const home = join(output, 'home');
const project = join(output, 'project');
const extensionPackage = join(project, '.jeden', 'extensions', 'example');
const manifest = join(extensionPackage, 'package.json');
mkdirSync(home, { recursive: true });
mkdirSync(extensionPackage, { recursive: true });
const env = { ...process.env, HOME: home };

function extensions() {
  const args = ['run', '/extensions', '--json', '--cwd', project];
  const result = spawnSync(binary, args, { cwd: project, env, encoding: 'utf8' });
  report.commands.push({ args, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.signal, null, '/extensions must not end by signal');
  return result;
}
function refused(words) {
  const result = extensions();
  assert.ok(result.status, `/extensions accepted a broken package: ${result.stdout}`);
  assert.ok(result.stderr.includes(words), `/extensions did not say ${words}: ${result.stderr}`);
}

try {
  writeFileSync(manifest, '{"jeden": {"extensions": [');
  refused(`${manifest} is not JSON`);

  writeFileSync(manifest, JSON.stringify({ jeden: { extensions: ['missing.mjs'] } }));
  refused(`${manifest} declares the extension missing.mjs, which is not a module file`);

  writeFileSync(join(extensionPackage, 'declared.mjs'), 'export default function () {}\n');
  writeFileSync(manifest, JSON.stringify({ jeden: { extensions: ['declared.mjs'] } }));
  const accepted = extensions();
  assert.ok(accepted.status !== null && !accepted.status, accepted.stderr);
  assert.ok(!JSON.parse(accepted.stdout).text.includes(manifest),
    `a valid package is still refused: ${accepted.stdout}`);
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
