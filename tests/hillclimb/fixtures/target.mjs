import assert from 'node:assert/strict';
import { readFileSync, realpathSync, statSync, writeFileSync } from 'node:fs';
import { basename, dirname, isAbsolute, join, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} must name the dedicated qualification resource; operator state is not a default`);
  return value;
}

// A prepared, real Brama profile is an explicit dependency, never the operator's
// home. Session output is unique to the run; the profile remains reusable.
export function qualificationEnvironment(sourceRoot, output) {
  const home = realpathSync(required('JEDEN_TEST_HOME'));
  const build = realpathSync(join(sourceRoot, '.build'));
  const inside = relative(build, home);
  assert.ok(inside && inside !== '..' && !inside.startsWith(`..${sep}`) && !isAbsolute(inside),
    'JEDEN_TEST_HOME must be a dedicated prepared profile inside this checkout’s ignored .build directory');
  assert.ok(statSync(home).isDirectory(), `JEDEN_TEST_HOME is not a directory: ${home}`);
  return { ...process.env, HOME: home, JEDEN_SESSION_ROOT: join(output, 'sessions') };
}

// The test drives one existing canonical checkout, not a clone, worktree or a
// nested git fixture. The tracked marker and exact seed prevent resetting an
// arbitrary repository. The caller reserves this qualification target exclusively.
export function openQualificationTarget({ sourceRoot, output, command, report }) {
  const repository = realpathSync(required('JEDEN_TEST_REPOSITORY'));
  const origin = required('JEDEN_TEST_REPOSITORY_ORIGIN');
  const source = realpathSync(sourceRoot);
  assert.notEqual(repository, source, 'the product checkout is not a qualification target');
  assert.equal(dirname(repository), dirname(source),
    'the qualification target must have its one canonical checkout directly beside the product checkout');
  const git = (...args) => command('git', args, repository).trimEnd();
  assert.equal(realpathSync(git('rev-parse', '--show-toplevel')), repository);
  assert.equal(realpathSync(git('rev-parse', '--absolute-git-dir')), join(repository, '.git'),
    'the qualification target must own its .git directory, not share a worktree');
  assert.equal(git('symbolic-ref', '--short', 'HEAD'), 'main');
  assert.equal(git('remote', 'get-url', 'origin'), origin, 'the target origin must match the caller’s declaration');
  assert.equal(git('status', '--porcelain', '--untracked-files=all'), '',
    'the dedicated qualification target has existing changes; nothing has been reset');
  assert.equal(git('ls-files', '--others', '-z'), '',
    'the qualification target has untracked or ignored files; nothing has been reset');
  const seeds = new Map([
    ['README.md', fileURLToPath(new URL('README.md', import.meta.url))],
    ['greet.mjs', fileURLToPath(new URL('greet.mjs', import.meta.url))],
  ]);
  const tracked = git('ls-files', '-z').split('\0').filter(Boolean).sort();
  assert.deepEqual(tracked, [...seeds.keys()].sort(), 'only the dedicated fixture seed may be tracked');
  for (const [path, seed] of seeds) {
    assert.deepEqual(readFileSync(join(repository, path)), readFileSync(seed),
      `the target does not match the qualification seed: ${path}`);
  }
  const revision = git('rev-parse', 'HEAD');
  report.fixture = { repository, origin, revision, restored: false };
  return {
    repository, organization: dirname(repository), name: basename(repository),
    restore() {
      // Refuse to rewind a commit, branch or identity another actor changed.
      assert.equal(git('rev-parse', 'HEAD'), revision, 'fixture HEAD changed; automatic restoration is refused');
      assert.equal(git('symbolic-ref', '--short', 'HEAD'), 'main', 'fixture branch changed; restoration is refused');
      assert.equal(git('remote', 'get-url', 'origin'), origin, 'fixture origin changed; restoration is refused');
      const failures = [];
      try {
        report.fixture.observedChanges = git('status', '--porcelain', '--untracked-files=all');
        git('add', '--intent-to-add', '--', '.');
        writeFileSync(join(output, 'fixture.patch'), git('diff', '--binary', '--no-ext-diff', '--no-textconv', revision));
      } catch (error) { failures.push(error); }
      try {
        // The seed was clean and explicitly reserved for this journey. Only
        // generated test work is removed; operator repositories never reach here.
        git('restore', '--source', revision, '--staged', '--worktree', '--', '.');
        git('clean', '-fdx', '--', '.');
        for (const [path, seed] of seeds) {
          assert.deepEqual(readFileSync(join(repository, path)), readFileSync(seed));
        }
        assert.equal(git('status', '--porcelain', '--untracked-files=all'), '');
        assert.equal(git('ls-files', '--others', '-z'), '');
        report.fixture.restored = true;
      } catch (error) { failures.push(error); }
      if (failures.length) throw new AggregateError(failures, 'The qualification target could not be retained and restored completely');
    },
  };
}
