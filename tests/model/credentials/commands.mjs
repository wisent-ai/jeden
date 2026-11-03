import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';

// Keep every real CLI invocation, exit status and redacted response in the
// journey's report, including the command that first refuses.
export function commands({ root, area, report, redacted }) {
  function invoke(program, args, env, input) {
    const result = spawnSync(program, args, { cwd: root, env, input, encoding: 'utf8' });
    report.commands.push({ program, args: args.map(redacted), cwd: root, status: result.status,
      signal: result.signal, error: redacted(result.error?.message),
      stdout: redacted(result.stdout), stderr: redacted(result.stderr) });
    writeFileSync(join(area, 'report.json'), JSON.stringify(report, (_, value) =>
      typeof value === 'string' ? redacted(value) : value, '  '));
    assert.ifError(result.error);
    assert.equal(result.signal, null, 'the operation must finish, not be interrupted');
    assert.notEqual(result.status, null, 'the operation must return an exit status');
    return result;
  }
  function succeeds(result) {
    assert.ok(!result.status, result.stderr || result.stdout);
    return result.stdout;
  }
  function refuses(result, cause) {
    assert.ok(result.status, `expected a refusal, got ${result.stdout}`);
    assert.ok(`${result.stdout}\n${result.stderr}`.includes(cause),
      `refusal omitted ${cause}: ${result.stderr}`);
  }
  function list(binary, env) {
    return JSON.parse(succeeds(invoke(binary, ['token', 'list', '--json'], env))).tokens;
  }
  return { invoke, succeeds, refuses, list };
}
