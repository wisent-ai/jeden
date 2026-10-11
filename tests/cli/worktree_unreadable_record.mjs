// Real CLI test: `jeden worktree list` and `jeden worktree clear` refuse when
// a job record they would read does not parse, naming the record, instead of
// skipping it. Skipped, the workspace that record claimed looked orphaned and
// `clear --yes` removed it from under the running job. The store is an
// isolated JEDEN_TASK_STORE under .build; a git worktree made for the run must
// still exist after the refused clear. Every command, its exit status and its
// output are kept in the run's report.json.
//
// Run: JEDEN_BIN=target/debug/jeden JEDEN_TEST_SOURCE_REVISION=$(git rev-parse HEAD) \
//   node tests/cli/worktree_unreadable_record.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
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
const runs = join(root, '.build/worktree-unreadable-record');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  commands: [], verdict: 'failed' };

function invoke(program, args, cwd, env = process.env) {
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
function refuses(result, words) {
  assert.ok(result.status !== null && result.status, `expected a refusal naming ${words}`);
  assert.ok(result.stderr.includes(words), `refusal ${result.stderr} does not say ${words}`);
}

const repo = join(output, 'repo');
const store = join(output, 'store');
const kept = join(output, 'kept-worktree');
try {
  mkdirSync(repo, { recursive: true });
  mkdirSync(join(store, 'jobs'), { recursive: true });
  succeeds(invoke('git', ['init', '--quiet'], repo));
  succeeds(invoke('git', ['-c', 'user.name=test', '-c', 'user.email=test@example.invalid',
    'commit', '--quiet', '--allow-empty', '-m', 'init'], repo));
  succeeds(invoke('git', ['worktree', 'add', '--quiet', kept], repo));
  const record = join(store, 'jobs', 'broken.json');
  writeFileSync(record, '{"id": "half-written');
  const env = { ...process.env, JEDEN_TASK_STORE: store };
  const words = `the job record ${record} is not a job record`;

  refuses(invoke(binary, ['worktree', 'list'], repo, env), words);
  refuses(invoke(binary, ['worktree', 'clear', '--dry-run'], repo, env), words);
  refuses(invoke(binary, ['worktree', 'clear', '--yes'], repo, env), words);
  assert.ok(existsSync(kept), `the refused clear removed ${kept}`);

  rmSync(record);
  JSON.parse(succeeds(invoke(binary, ['worktree', 'list', '--json'], repo, env)));
  succeeds(invoke('git', ['worktree', 'remove', kept], repo));
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
