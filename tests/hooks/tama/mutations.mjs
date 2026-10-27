// Real CLI journey: Jeden's own file mutations reach Tama's write gate as the
// files they leave. A real model turn calls Jeden's `edit` with an anchored
// move and a removal, `move_file` and `delete_file`; the registry runs the
// actual installed `tama hook pre-write-edit` before each one and a recorder
// keeps every payload the hooks were handed. Admitted moves land under their
// destination with the planned content, removals leave no file, a sibling is
// untouched, and a move into a Python name is refused by Tama's own rule
// before anything changes. HOME and the project are isolated under .build;
// every command, exit status, output, hook payload and observed file state is
// kept in report.json. Nothing here builds a binary or simulates a provider.
//
// Run: JEDEN_BIN=target/release/jeden TAMA_BIN=$(command -v tama) \
//   JEDEN_TEST_SOURCE_REVISION=$(git rev-parse HEAD) \
//   JEDEN_TEST_TOOL_ROUTE=<provider/model a tool turn runs on> \
//   node tests/hooks/tama/mutations.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const binary = resolve(required('JEDEN_BIN'));
const tama = resolve(required('TAMA_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const route = required('JEDEN_TEST_TOOL_ROUTE');
const runs = join(root, '.build/hooks-tama-mutations');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const fileHash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
// Jeden's snapshot tag: the content hash's first sixteen bits, upper-case.
const tagWidth = Buffer.byteLength('FFFF');
const tag = (path) => fileHash(path).toUpperCase().split('', tagWidth).join('');
const report = { revision, binary, binarySha256: fileHash(binary), tama, tamaSha256: fileHash(tama),
  testSha256: fileHash(fileURLToPath(import.meta.url)), commands: [], cases: [], verdict: 'failed' };

const home = join(output, 'home');
const project = join(output, 'project');
mkdirSync(join(project, 'source'), { recursive: true });
mkdirSync(join(project, 'target'), { recursive: true });
mkdirSync(home, { recursive: true });
const payloads = join(output, 'pre-tool-payloads.jsonl');
const registry = join(output, 'registry.json');
writeFileSync(registry, JSON.stringify({
  managedBy: 'tama',
  events: {
    'pre_tool_use:edit': { blocking: true, hooks: [
      { id: 'payload-recorder', type: 'command', command: `cat >> '${payloads}'; printf '\\n' >> '${payloads}'` },
      { id: 'pre-write-edit', type: 'command', command: `'${tama}' hook pre-write-edit` },
    ] },
  },
}));
const env = { ...process.env, HOME: home, JEDEN_TAMA_REGISTRY: registry };

function invoke(program, args, cwd) {
  const result = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, args, cwd, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.signal, null, `${args.join(' ')} must not end by signal`);
  return result;
}
function turn(name, instruction) {
  const result = invoke(binary, ['run', instruction, '--cwd', project, '--model', route,
    '--allow-write'], project);
  assert.ok(result.status !== null && !result.status, result.stderr);
  report.cases.push({ name, stdout: result.stdout });
  return result.stdout;
}
const recorded = () => readFileSync(payloads, 'utf8').split('\n').filter(Boolean).map((line) => JSON.parse(line));
const state = (name, paths) => {
  const observed = Object.fromEntries(paths.map((path) => [path, existsSync(join(project, path))
    ? { exists: true, sha256: fileHash(join(project, path)), text: readFileSync(join(project, path), 'utf8') }
    : { exists: false }]));
  report.cases.push({ name: `${name}-state`, observed });
  return observed;
};

try {
  const commit = invoke('git', ['rev-parse', 'HEAD'], root).stdout.trim();
  assert.equal(commit, revision, 'qualify the declared checkout');
  assert.equal(invoke('git', ['status', '--porcelain'], root).stdout.trim(), '',
    'commit the candidate and journey before qualification');
  const short = invoke('git', ['rev-parse', '--short', revision], root).stdout.trim();
  report.binaryVersion = invoke(binary, ['--version'], root).stdout.trim();
  assert.ok(report.binaryVersion.endsWith(`.${short}`), `candidate binary must embed ${short}`);

  const kept = 'source/kept.rs';
  const moved = 'source/moved.rs';
  const obsolete = 'source/obsolete.rs';
  const sibling = 'source/sibling.rs';
  writeFileSync(join(project, kept), '// kept\n');
  writeFileSync(join(project, moved), '// moved module\n// renamed below\n');
  writeFileSync(join(project, obsolete), '// superseded module\n');
  writeFileSync(join(project, sibling), '// untouched sibling\n');
  const siblingBefore = fileHash(join(project, sibling));
  // The moved file's last line, counted from its text, is the one cut.
  const movedLines = readFileSync(join(project, moved), 'utf8').trimEnd().split('\n').length;

  const patch = ['*** Begin Patch', `[${moved}#${tag(join(project, moved))}]`, `DEL ${movedLines}`,
    'MV target/moved.rs', `[${obsolete}#${tag(join(project, obsolete))}]`, 'REM', '*** End Patch'].join('\n');
  turn('anchored-move-and-removal', `Call the edit tool exactly once with this patch argument, unchanged, then respond exactly: EDITED\n${patch}`);
  const after = state('anchored-move-and-removal', [moved, 'target/moved.rs', obsolete, sibling, kept]);
  assert.equal(after[moved].exists, false);
  assert.equal(after['target/moved.rs'].text, '// moved module\n');
  assert.equal(after[obsolete].exists, false);
  assert.equal(after[sibling].sha256, siblingBefore);
  const sections = recorded().filter((payload) => payload.tool === 'edit');
  assert.ok(sections.some((payload) => payload.tool_name === 'edit' && payload.tool_input.path === moved
    && payload.tool_input.patch.includes(`CUT ${movedLines}.=${movedLines}`)
    && payload.tool_input.patch.includes('MV target/moved.rs')),
  `the move was not presented as its section: ${JSON.stringify(sections)}`);
  assert.ok(sections.some((payload) => payload.tool_input.path === obsolete
    && payload.tool_input.patch.includes('\nREM\n')), 'the removal was not presented as REM');

  const keptExpected = fileHash(join(project, kept));
  turn('move-file', `Call move_file once with from "${kept}", to "target/kept.rs" and expectedSha256 "${keptExpected}", then respond exactly: MOVED`);
  assert.equal(state('move-file', ['target/kept.rs', kept])['target/kept.rs'].text, '// kept\n');
  const removedExpected = fileHash(join(project, 'target/kept.rs'));
  turn('delete-file', `Call delete_file once with path "target/kept.rs" and expectedSha256 "${removedExpected}", then respond exactly: DELETED`);
  assert.equal(state('delete-file', ['target/kept.rs'])['target/kept.rs'].exists, false);

  const pythonPatch = ['*** Begin Patch', `[${sibling}#${tag(join(project, sibling))}]`, 'MV target/sibling.py',
    '*** End Patch'].join('\n');
  const refused = turn('move-into-python', `Call the edit tool exactly once with this patch argument, unchanged, then report the tool's result verbatim\n${pythonPatch}`);
  const unchanged = state('move-into-python', [sibling, 'target/sibling.py']);
  assert.equal(unchanged[sibling].sha256, siblingBefore);
  assert.equal(unchanged['target/sibling.py'].exists, false);
  assert.ok(refused.includes('Python source is not written'), `Tama's refusal did not reach the turn: ${refused}`);
  report.payloads = recorded();
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report));
}
