// Qualify the actual native staging operation on a release worker.
// This builds real binaries. It does not qualify signing, sandboxing or model calls.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { closeSync, mkdirSync, mkdtempSync, openSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return value;
}
const source = realpathSync(required('WISENT_SOURCE_DIR'));
const sourceRevision = required('WISENT_SOURCE_COMMIT');
const releaseOutput = realpathSync(required('WISENT_OUTPUT_DIR'));
const input = realpathSync(required('WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR'));
const successExit = Number(required('JEDEN_TEST_SUCCESS_EXIT'));
assert.ok(Number.isInteger(successExit));
const parent = join(releaseOutput, 'release-tests');
mkdirSync(parent, { recursive: true });
const output = mkdtempSync(join(parent, 'run-'));
const candidate = join(output, 'candidate');
mkdirSync(candidate);
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const lock = digest(readFileSync(join(source, 'Cargo.lock')));
const provenance = readFileSync(join(input, 'provenance.json'));
const report = { source_revision: sourceRevision, test_sha256: digest(readFileSync(fileURLToPath(import.meta.url))), started_at: new Date().toISOString(), commands: [], cases: [], verdict: 'failed' };
const environment = { ...process.env, WISENT_OUTPUT_DIR: candidate };
const stage = ['run', '--quiet', '--locked', '--manifest-path', join(source, 'tools', 'Cargo.toml'), '--', 'release', 'stage', '--bin', 'jeden', '--bin', 'jeden-sandbox-helper'];
function command(program, args, env = environment) {
  const directory = join(output, 'commands', randomUUID());
  mkdirSync(directory, { recursive: true });
  const stdout = join(directory, 'stdout');
  const stderr = join(directory, 'stderr');
  const out = openSync(stdout, 'w');
  const err = openSync(stderr, 'w');
  let result;
  try { result = spawnSync(program, args, { cwd: source, env, stdio: ['ignore', out, err] }); }
  finally { closeSync(out); closeSync(err); }
  report.commands.push({ program, args, status: result.status, signal: result.signal, error: result.error?.message, stdout, stderr });
  return { ...result, stdout: readFileSync(stdout, 'utf8'), stderr: readFileSync(stderr, 'utf8') };
}
function success(program, args, env) {
  const result = command(program, args, env);
  assert.equal(result.status, successExit, `${program} ${args.join(' ')}: ${result.stderr}`);
  return result;
}
try {
  report.stado_version = success('stado', ['--version']).stdout.trim();
  success('cargo', stage);
  const executable = join(candidate, 'bin', 'jeden');
  const helper = join(candidate, 'bin', 'jeden-sandbox-helper');
  assert.ok(statSync(executable).isFile());
  assert.ok(statSync(helper).isFile());
  const version = success(executable, ['--version']).stdout.trim();
  const packageText = readFileSync(join(source, 'Cargo.toml'), 'utf8');
  const declared = packageText.match(/^version\s*=\s*"([^"]+)"/m);
  assert.ok(declared, 'Cargo.toml has no version');
  const [, expectedVersion] = declared;
  assert.ok(version.split(/\s+/).includes(expectedVersion), `staged CLI reported ${version}, expected ${expectedVersion}`);
  const binaryDigest = digest(readFileSync(executable));
  const helperDigest = digest(readFileSync(helper));
  report.cases.push({ name: 'stage-and-execute', verdict: 'passed', version, binary_sha256: binaryDigest, helper_sha256: helperDigest });
  const missingOutput = { ...environment };
  delete missingOutput.WISENT_OUTPUT_DIR;
  const missingInput = { ...environment };
  delete missingInput.WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR;
  for (const [name, env] of [['missing-output', missingOutput], ['missing-private-input', missingInput]]) {
    const result = command('cargo', stage, env);
    assert.ok(result.status !== null && result.status !== successExit, `${name} was accepted`);
    assert.equal(digest(readFileSync(executable)), binaryDigest, `${name} replaced the staged CLI`);
    assert.equal(digest(readFileSync(helper)), helperDigest, `${name} replaced the staged helper`);
    assert.equal(digest(readFileSync(join(source, 'Cargo.lock'))), lock);
    assert.deepEqual(readFileSync(join(input, 'provenance.json')), provenance);
    report.cases.push({ name, verdict: 'passed', status: result.status, staged_files_unchanged: true, lock_unchanged: true, input_unchanged: true });
  }
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack ?? error);
} finally {
  rmSync(join(candidate, 'bin'), { recursive: true, force: true });
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '\t'));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
if (report.verdict !== 'passed') throw new Error(report.error);
