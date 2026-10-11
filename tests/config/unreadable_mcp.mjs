// Real CLI test: an mcp.json that exists but does not parse, or cannot be
// read as a file, is never read as "no servers". `jeden doctor` reports the
// MCP subsystem unavailable with the file and the cause; once the file is a
// valid empty declaration the same probe no longer names it. Read as empty,
// a typo in mcp.json made every server it declares, and every server it
// disables, vanish without a word.
//
// The binary runs against an isolated HOME and project under .build, so the
// operator's own files are never read or touched. Every command, its exit
// status and its output are kept in the run's report.json.
//
// Run: JEDEN_BIN=target/debug/jeden JEDEN_TEST_SOURCE_REVISION=$(git rev-parse HEAD) \
//   node tests/config/unreadable_mcp.mjs
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
const runs = join(root, '.build/config-unreadable-mcp');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  commands: [], verdict: 'failed' };

const home = join(output, 'home');
const project = join(output, 'project');
const userMcp = join(home, '.jeden', 'mcp.json');
const projectMcp = join(project, '.jeden', 'mcp.json');
mkdirSync(dirname(userMcp), { recursive: true });
mkdirSync(dirname(projectMcp), { recursive: true });
const env = { ...process.env, HOME: home };

function mcpProbe() {
  const result = spawnSync(binary, ['doctor', '--cwd', project], { cwd: project, env, encoding: 'utf8' });
  report.commands.push({ args: ['doctor', '--cwd', project], cwd: project, status: result.status,
    signal: result.signal, stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.signal, null, 'doctor must not end by signal');
  const doctor = JSON.parse(result.stdout);
  const probe = doctor.probes.find((candidate) => candidate.subsystem === 'mcp');
  assert.ok(probe, `doctor reported no mcp probe: ${result.stdout}`);
  return probe;
}
function refused(words) {
  const probe = mcpProbe();
  assert.equal(probe.state, 'unavailable', `mcp probe ${JSON.stringify(probe)} is not unavailable`);
  assert.ok(probe.detail.includes(words), `mcp probe detail ${probe.detail} does not say ${words}`);
}

try {
  writeFileSync(userMcp, '{"mcpServers": {"half-written"');
  refused(`${userMcp} is not valid JSON`);

  writeFileSync(userMcp, '{"mcpServers": {}}');
  writeFileSync(projectMcp, '{"disabledServers": [');
  refused(`${projectMcp} is not valid JSON`);

  // A path that exists but cannot be read as a file.
  rmSync(projectMcp);
  mkdirSync(projectMcp);
  refused(`${projectMcp} cannot be read`);
  rmSync(projectMcp, { recursive: true });

  writeFileSync(projectMcp, '{"mcpServers": {}, "disabledServers": []}');
  const probe = mcpProbe();
  assert.ok(!probe.detail.includes(projectMcp) && !probe.detail.includes(userMcp),
    `a valid mcp.json is still refused: ${probe.detail}`);
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
