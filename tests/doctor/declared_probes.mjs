// Real CLI test: `jeden doctor` reads the LSP, browser and collab
// subsystems from what their capabilities declare, not from words inside
// capability ids. A project command whose name carries "collab", "browser"
// and "lsp" (`/collab-browser-lsp-notes`) is not counted by any of those
// probes, while the collab probe reads the commands the build declares under
// the collab service (`/collab`, `/join`, `/leave`).
//
// The binary runs against an isolated HOME and project under .build, so the
// operator's own files are never read or touched. Every command, its exit
// status and its output are kept in the run's report.json.
//
// Run: JEDEN_BIN=target/debug/jeden JEDEN_TEST_SOURCE_REVISION=$(git rev-parse HEAD) \
//   node tests/doctor/declared_probes.mjs
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
const runs = join(root, '.build/doctor-declared-probes');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  commands: [], verdict: 'failed' };
const save = () => writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '\t'));

const home = join(output, 'home');
const project = join(output, 'project');
const command = 'collab-browser-lsp-notes';
mkdirSync(home, { recursive: true });
mkdirSync(join(project, '.jeden', 'commands'), { recursive: true });
writeFileSync(join(project, '.jeden', 'commands', `${command}.md`), 'Notes for this run.\n');
const env = { ...process.env, HOME: home };

try {
  const args = ['doctor', '--cwd', project];
  const result = spawnSync(binary, args, { cwd: project, env, encoding: 'utf8' });
  report.commands.push({ args, cwd: project, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.signal, null, 'doctor must not end by signal');
  const doctor = JSON.parse(result.stdout);
  const probe = (subsystem) => {
    const found = doctor.probes.find((candidate) => candidate.subsystem === subsystem);
    assert.ok(found, `doctor reported no ${subsystem} probe: ${result.stdout}`);
    return found;
  };
  // The capabilities a probe read: its evidence, or, for an unavailable
  // probe, which carries none, the handlers its detail names.
  const read = (subsystem) => {
    const found = probe(subsystem);
    if (found.evidence) {
      assert.ok(Array.isArray(found.evidence.descriptors), `${subsystem} evidence names no descriptors`);
      return found.evidence.descriptors;
    }
    assert.equal(found.state, 'unavailable', `${subsystem} probe carries no evidence: ${JSON.stringify(found)}`);
    const [, named] = found.detail.split('unavailable handlers: ');
    assert.ok(named, `${subsystem} probe names no handlers: ${found.detail}`);
    return named.split(', ');
  };
  for (const subsystem of ['lsp', 'browser', 'collab']) {
    assert.ok(!read(subsystem).includes(`slash/${command}`),
      `the ${subsystem} probe counted /${command} by a word in its name: ${JSON.stringify(probe(subsystem))}`);
  }
  const collab = read('collab');
  for (const declared of ['slash/collab', 'slash/join', 'slash/leave']) {
    assert.ok(collab.includes(declared), `the collab probe does not read ${declared}: ${JSON.stringify(probe('collab'))}`);
  }
  for (const descriptor of read('lsp')) {
    assert.equal(descriptor, 'tool/lsp', `the lsp probe read ${descriptor}`);
  }
  report.verdict = 'passed';
} finally {
  save();
}
