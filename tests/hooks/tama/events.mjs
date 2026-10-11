// Real CLI test: how Jeden runs the events of Tama's shared hook registry.
// A registration on post_tool_use:any runs after every Jeden tool (matcher
// ^.+$), not after a tool literally named "any": a real turn on a real model
// calls list_dir, and the hook's own command leaves its mark in the project.
// A registration on an event Jeden has no event for (before_provider_request)
// is named by `/hooks` and by `jeden doctor` as not running here, never
// dropped without a word. The registry is a real registry file of Tama's
// format; HOME and the project are isolated under .build. Every command, its
// exit status and its output are kept in report.json.
//
// Run: JEDEN_BIN=target/debug/jeden JEDEN_TEST_SOURCE_REVISION=$(git rev-parse HEAD) \
//   JEDEN_TEST_TOOL_ROUTE=<provider/model a tool turn runs on through Brama> \
//   node tests/hooks/tama/events.mjs
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
const route = required('JEDEN_TEST_TOOL_ROUTE');
const runs = join(root, '.build/hooks-tama-events');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  commands: [], verdict: 'failed' };

const home = join(output, 'home');
const project = join(output, 'project');
mkdirSync(home, { recursive: true });
mkdirSync(project, { recursive: true });
const registry = join(output, 'registry.json');
// The hook's command writes the tool it ran after (JEDEN_HOOK_TOOL), one
// line per tool call.
const marks = join(output, 'after-every-tool.log');
writeFileSync(registry, JSON.stringify({
  managedBy: 'tama',
  events: {
    'post_tool_use:any': { blocking: false, hooks: [{ id: 'after-every-tool', type: 'command',
      command: `printf '%s\\n' "$JEDEN_HOOK_TOOL" >> '${marks}'` }] },
    before_provider_request: { blocking: true, hooks: [{ id: 'before-each-request', type: 'command', command: '/usr/bin/true' }] },
  },
}));
// The isolated HOME holds no Jeden configuration; the turn reaches Brama
// through the process's own environment, as a session started by hand does.
const env = { ...process.env, HOME: home, JEDEN_TAMA_REGISTRY: registry };

function invoke(args) {
  const result = spawnSync(binary, args, { cwd: project, env, encoding: 'utf8' });
  report.commands.push({ args, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.signal, null, `${args.join(' ')} must not end by signal`);
  return result;
}

try {
  const listed = invoke(['run', '/hooks', '--json', '--cwd', project]);
  assert.ok(listed.status !== null && !listed.status, listed.stderr);
  const text = JSON.parse(listed.stdout).text;
  assert.match(text, /post_tool_use:any -> PostToolUse \[\^\.\+\$\]/,
    `post_tool_use:any must match every Jeden tool: ${text}`);
  assert.ok(text.includes('Registry events Jeden has no event for; these hooks do not run in Jeden:'),
    `an event Jeden cannot run must be named: ${text}`);
  assert.ok(text.includes('before_provider_request: before-each-request'), text);

  // doctor exits unsuccessfully when another probe is unavailable in the
  // isolated home; its report is read either way.
  const doctor = invoke(['doctor', '--json', '--cwd', project]);
  const probe = JSON.parse(doctor.stdout).probes.find((entry) => entry.subsystem === 'hooks');
  assert.ok(probe, 'jeden doctor reports no hooks probe');
  assert.equal(probe.state, 'degraded');
  assert.deepEqual(probe.evidence.unmapped,
    [{ event: 'before_provider_request', hooks: ['before-each-request'] }]);

  // A real turn that calls a tool runs the any-tool hook after it.
  const turn = invoke(['run', 'Call list_dir on . once, then respond exactly: LISTED', '--cwd', project,
    '--model', route]);
  assert.ok(turn.status !== null && !turn.status, turn.stderr);
  const marked = readFileSync(marks, 'utf8');
  report.marks = marked;
  assert.ok(marked.includes('list_dir'), `the any-tool hook did not run after list_dir: ${marked}`);
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
