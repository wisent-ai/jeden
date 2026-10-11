// Real filesystem refusals through the owning session's RPC handoff operation.
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdirSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

export async function exerciseHandoffRefusals({ rpc, prompt, session, output, retainedBefore, report }) {
  const artifactDirectory = join(session.sessionPath, 'artifacts');
  for (const scenario of [
    { name: 'artifact-directory-refusal', path: artifactDirectory,
      operation: 'handoff could not create artifact directory',
      obstruct: (path) => writeFileSync(path, 'not a directory'),
      probe: (path) => mkdirSync(path, { recursive: true }) },
    { name: 'brief-write-refusal', path: join(artifactDirectory, 'handoff.md'),
      operation: 'handoff could not write brief',
      obstruct: (path) => mkdirSync(path),
      probe: (path) => writeFileSync(path, '') },
  ]) {
    const saved = join(output, `saved-artifact-${randomUUID()}`);
    renameSync(scenario.path, saved);
    const evidence = { name: scenario.name, verdict: 'pending', path: scenario.path,
      operation: scenario.operation };
    report.cases.push(evidence);
    const failures = [];
    try {
      scenario.obstruct(scenario.path);
      let cause;
      try { scenario.probe(scenario.path); } catch (error) { cause = error; }
      assert.ok(cause?.errno, `${scenario.name} requires an actual filesystem refusal`);
      Object.assign(evidence, { cause: cause.message, errno: cause.errno });
      await assert.rejects(prompt(session, '/handoff Preserve the original session and its work.'),
        (error) => error.message.includes(scenario.operation)
          && error.message.includes(scenario.path)
          && error.message.includes(`os error ${Math.abs(cause.errno)}`),
        `${scenario.name} must identify the operation, actual artifact path and OS cause`);
      const retained = await rpc('session/completion/get', { sessionId: session.sessionId });
      for (const request of retainedBefore.completion.requests) {
        assert.deepEqual(retained.completion.requests.find((candidate) => candidate.id === request.id), request,
          `${scenario.name} must leave the original requests owned by the active session`);
      }
      evidence.retained = retained;
    } catch (error) {
      failures.push(error);
    } finally {
      for (const restore of [
        () => rmSync(scenario.path, { recursive: true, force: true }),
        () => renameSync(saved, scenario.path),
      ]) {
        try { restore(); } catch (error) { failures.push(error); }
      }
    }
    if (failures.length) {
      evidence.verdict = 'failed';
      evidence.errors = failures.map(String);
      throw new AggregateError(failures, `${scenario.name}: ${evidence.errors.join('; ')}`);
    }
    evidence.verdict = 'passed';
  }
}
