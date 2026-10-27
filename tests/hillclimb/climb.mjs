// Real test of `jeden hillclimb` through the built binary.
//
//   1. Every invocation refusal exits with the usage status an unknown
//      command gets, before any model is asked: no write and command grants,
//      a repository scope on a directory without .git, an organization scope
//      with no repository under it, an unknown scope, --rounds that is not a
//      whole number above zero, and an option hillclimb does not take.
//   2. A ledger holding a line that is not a round is refused naming the
//      line, and the ledger is left as it was.
//   3. One real repository round adds working --help and must be independently
//      accepted. An organization round then adds --goodbye in the admitted
//      checkout, without changing an excluded checkout or the parent.
//      A process success with a failed round never qualifies as passed.
// Every command, its exit status and output go to the run's report.json.
//
// Usage: JEDEN_BIN=<built jeden> JEDEN_TEST_SOURCE_REVISION=<source HEAD> node tests/hillclimb/climb.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { duringOwnedClimb } from './ownership.mjs';
import { openQualificationTarget, qualificationEnvironment } from './fixtures/target.mjs';

const declaredBinary = process.env.JEDEN_BIN;
if (!declaredBinary) {
  throw new Error('JEDEN_BIN must name the built jeden this test runs; the test proves that binary, not whichever is on PATH');
}
const binary = resolve(declaredBinary);
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const runs = join(root, '.build/hillclimb');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const report = {
  binary,
  commands: [],
  outcome: 'failed: the real journey has not completed',
};
let environment;
let fixture;
const failures = [];

function jeden(args, cwd) {
  assert.ok(environment, 'The isolated qualification environment has not been established');
  const result = spawnSync(binary, args, { cwd, env: environment, encoding: 'utf8' });
  report.commands.push({ program: binary, args, cwd, exit_status: result.status, signal: result.signal,
    error: result.error?.stack, stdout: result.stdout, stderr: result.stderr });
  if (result.error) throw result.error;
  return result;
}

function command(program, args, cwd) {
  assert.ok(environment, 'The isolated qualification environment has not been established');
  const result = spawnSync(program, args, { cwd, env: environment, encoding: 'utf8' });
  report.commands.push({ program, args, cwd, exit_status: result.status, signal: result.signal,
    error: result.error?.stack, stdout: result.stdout, stderr: result.stderr });
  if (result.error) throw result.error;
  assert.ok(result.status !== null && !result.status, `${program} ${args.join(' ')}: ${result.stderr}`);
  return result.stdout;
}

function git(cwd, ...args) { return command('git', args, cwd); }

try {
  environment = qualificationEnvironment(root, output);
  fixture = openQualificationTarget({ sourceRoot: root, output, command, report });
  const repository = fixture.repository;
  report.revision = git(root, 'rev-parse', 'HEAD').trim();
  assert.equal(report.revision, process.env.JEDEN_TEST_SOURCE_REVISION,
    'JEDEN_TEST_SOURCE_REVISION must identify the actual source HEAD');
  report.sourcePatch = git(root, 'diff', 'HEAD', '--binary');
  report.binarySha256 = createHash('sha256').update(readFileSync(binary)).digest('hex');
  const version = jeden(['--version'], root);
  assert.ok(version.status !== null && !version.status, version.stderr);
  report.binaryVersion = version.stdout.trim();
  report.sourceFiles = Object.fromEntries([
    'rust/hillclimb/mod.rs', 'rust/hillclimb/execution.rs', 'rust/hillclimb/status/mod.rs',
    'rust/hillclimb/ledger.rs', 'rust/hillclimb/target.rs', 'rust/hillclimb/prompt.txt',
    'tests/hillclimb/climb.mjs', 'tests/hillclimb/ownership.mjs', 'tests/hillclimb/fixtures/greet.mjs',
    'tests/hillclimb/fixtures/target.mjs', 'tests/hillclimb/fixtures/README.md',
  ].map((path) => [path, createHash('sha256').update(readFileSync(join(root, path))).digest('hex')]));

  const plain = join(output, 'plain');
  mkdirSync(plain, { recursive: true });
  const unknown = jeden(['no-such-command'], output);
  assert.ok(unknown.status, 'an unknown command was accepted');
  const usage = unknown.status;
  const grants = ['--allow-write', '--allow-command'];
  const refusals = [
    [['hillclimb'], repository, ['--allow-write', '--allow-command']],
    [['hillclimb', ...grants], plain, [plain]],
    [['hillclimb', '--scope', 'organization', ...grants], plain, [plain]],
    [['hillclimb', '--scope', 'galaxy', ...grants], repository, ['--scope']],
    [['hillclimb', '--rounds', 'none', ...grants], repository, ['--rounds']],
    [['hillclimb', '--sideways', ...grants], repository, ['--sideways']],
    [['hillclimb', '--only', fixture.name, ...grants], repository, ['--only']],
    [['hillclimb', '--scope', 'organization', '--only', 'no-such-repo', ...grants], fixture.organization, ['no-such-repo', fixture.organization]],
  ];
  for (const [args, cwd, diagnostics] of refusals) {
    const run = jeden(args, cwd);
    assert.equal(run.status, usage, `jeden ${args.join(' ')} exited ${run.status}: ${run.stderr}`);
    for (const value of diagnostics) assert.ok(run.stderr.includes(value), `the refusal must identify ${value}: ${run.stderr}`);
  }

  // One round is what the real climb below runs, measured as the size of a one-ledger set.
  const ledger = join(output, 'ledger.jsonl');
  const once = new Set([ledger]).size;

  const broken = join(output, 'broken.jsonl');
  writeFileSync(broken, 'not a round\n');
  const unreadable = jeden(['hillclimb', '--ledger', broken, '--rounds', String(once), ...grants], repository);
  assert.ok(unreadable.status && unreadable.status !== usage, `a broken ledger: ${unreadable.stderr}`);
  assert.ok(unreadable.stderr.includes(broken), 'the refusal identifies the persisted ledger it could not parse');
  assert.equal(readFileSync(broken, 'utf8'), 'not a round\n', 'the broken ledger is left as it was');

  const helpText = 'Usage: node greet.mjs NAME';
  const roundArgs = ['hillclimb', '--ledger', ledger, '--rounds', String(once), '--json', ...grants,
    `add --help to greet.mjs that prints exactly ${JSON.stringify(helpText)} without changing greeting behavior. Do not commit, push, change git metadata or create remotes.`];
  const climbed = await duringOwnedClimb({
    binary, args: roundArgs, cwd: repository, env: environment, ledger, report,
    probe: () => {
      const sourceBefore = readFileSync(join(repository, 'greet.mjs'), 'utf8');
      const ledgerBefore = existsSync(ledger) ? readFileSync(ledger, 'utf8') : undefined;
      const contender = jeden(roundArgs, repository);
      assert.ok(contender.status && contender.status !== usage, contender.stderr);
      assert.ok(contender.stderr.includes(ledger), 'the refusal names the actual contested ledger');
      assert.ok(contender.stderr.includes(`${ledger}.lock`), 'the refusal identifies the actual owned lock');
      assert.equal(readFileSync(join(repository, 'greet.mjs'), 'utf8'), sourceBefore);
      if (ledgerBefore === undefined) assert.equal(existsSync(ledger), false);
      else assert.equal(readFileSync(ledger, 'utf8'), ledgerBefore);
    },
  });
  assert.ok(climbed.status !== null && !climbed.status, `the real repository round failed: ${climbed.stderr}`);
  const result = JSON.parse(climbed.stdout);
  assert.equal(result.rounds.length, once);
  assert.equal(result.accepted, once, JSON.stringify(result));
  const lines = readFileSync(ledger, 'utf8').trim().split('\n');
  assert.equal(lines.length, once, 'the ledger holds the one round');
  const [round] = lines.map((line) => JSON.parse(line));
  assert.equal(round.ending, 'accepted', JSON.stringify(round));
  assert.ok(existsSync(round.receipt), 'the independent acceptance receipt exists');
  assert.equal(command(process.execPath, ['greet.mjs', '--help'], repository).trim(), helpText);

  const excludedBefore = {
    status: git(root, 'status', '--porcelain', '--untracked-files=all'),
    patch: git(root, 'diff', 'HEAD', '--binary'),
  };
  const parentSource = join(fixture.organization, 'greet.mjs');
  const parentBefore = existsSync(parentSource) ? readFileSync(parentSource) : undefined;
  const organization = jeden(['hillclimb', '--scope', 'organization', '--only', fixture.name,
    '--ledger', ledger, '--rounds', String(once), '--json', ...grants,
    'add greet.mjs --goodbye NAME that prints exactly Goodbye, followed by a space and NAME; preserve the existing greeting and help. Do not commit, push, change git metadata or create remotes.'], fixture.organization);
  assert.ok(organization.status !== null && !organization.status, `the real organization round failed: ${organization.stderr}`);
  const organizationResult = JSON.parse(organization.stdout);
  assert.equal(organizationResult.accepted, once, JSON.stringify(organizationResult));
  const [organizationRound] = organizationResult.rounds;
  assert.equal(organizationRound.repository, fixture.name);
  assert.equal(organizationRound.ending, 'accepted');
  assert.equal(organizationRound.round, round.round + once, 'the next owner continues the recorded numbering');
  assert.deepEqual(readFileSync(ledger, 'utf8').trim().split('\n').map((line) => JSON.parse(line)),
    [round, organizationRound], 'ownership is released after exit and the next invocation appends without replacing history');
  assert.equal(command(process.execPath, ['greet.mjs', '--goodbye', 'Reader'], repository).trim(), 'Goodbye, Reader');
  assert.equal(git(root, 'status', '--porcelain', '--untracked-files=all'), excludedBefore.status,
    'the excluded product checkout keeps its existing index and working-tree state');
  assert.equal(git(root, 'diff', 'HEAD', '--binary'), excludedBefore.patch,
    'organization execution does not change excluded source');
  if (parentBefore === undefined) assert.equal(existsSync(parentSource), false);
  else assert.deepEqual(readFileSync(parentSource), parentBefore, 'execution does not change the organization parent');
  report.outcome = 'passed: repository and organization rounds were accepted and their CLI behaviors were observed';
} catch (error) {
  failures.push(error);
} finally {
  if (fixture) {
    try { fixture.restore(); } catch (error) { failures.push(error); }
  }
  if (failures.length) report.outcome = 'failed: the qualification journey or fixture restoration failed';
  report.failures = failures.map((error) => String(error));
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, '\t')}\n`);
  console.log(`${report.outcome}\nreport: ${join(output, 'report.json')}`);
}
if (failures.length) throw new AggregateError(failures, 'The real CLI hillclimb journey failed');
