// A context source that never answers no longer holds its caller: the real
// `jeden rpc` advisor is asked through the actual transport, its ground-truth
// source is pointed at a listener on this machine's loopback interface that
// this journey opens and never answers, and the request is cancelled once the
// source's GET is in flight. The answer must name the source as unanswered
// with its caller's cancellation as the cause, a second cancel of the
// finished id must be refused, and the same connection must answer a later
// request.
// Inputs: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_CONTEXT_LIMIT.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { networkInterfaces } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { startRpc } from '../hooks/rpc.mjs';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
}
const binary = resolve(required('JEDEN_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const limit = JSON.parse(required('JEDEN_TEST_CONTEXT_LIMIT'));
assert.ok(Number.isSafeInteger(limit), 'JEDEN_TEST_CONTEXT_LIMIT must be a whole number');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const runs = join(root, '.build/context-cancellation');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const workspace = join(output, 'workspace');
mkdirSync(join(workspace, '.jeden'), { recursive: true });
const hash = path => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = {
  startedAt: new Date().toISOString(), binary, declaredSourceRevision: revision,
  binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  workspace, commands: [], frames: [], cases: [], verdict: 'failed',
};
const persist = () => writeFileSync(join(output, 'report.json'), JSON.stringify(report));
persist();

function git(args) {
  const result = spawnSync('git', args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ program: 'git', args, status: result.status, stderr: result.stderr });
  assert.ok(result.status !== null && !result.status, `git ${args.join(' ')}: ${result.stderr}`);
  return result.stdout.trim();
}

// This machine's IPv4 loopback address, as the operating system reports its
// internal interface.
const loopback = Object.values(networkInterfaces())
  .flat()
  .find(entry => entry.internal && entry.family === 'IPv4')?.address;
assert.ok(loopback, `no internal IPv4 interface: ${JSON.stringify(networkInterfaces())}`);

// The source that never answers: connections are accepted and held, nothing
// is ever written back. Each accepted socket is kept so the journey can
// close it at the end.
const held = [];
let connected;
const firstConnection = new Promise(resolveConnection => { connected = resolveConnection; });
const server = createServer(socket => {
  held.push(socket);
  socket.on('error', error => report.cases.push({ case: 'held-socket-error', error: error.message }));
  connected();
});
await new Promise((resolveListen, reject) => {
  server.once('error', reject);
  server.listen({ host: loopback }, resolveListen);
});
const url = `http://${loopback}:${server.address().port}`;
writeFileSync(join(workspace, '.jeden/config.json'), JSON.stringify({
  context: { advisor: { roots: workspace, groundTruthUrl: url } },
}));

const transport = startRpc({ binary, cwd: workspace, env: process.env, report });
try {
  assert.equal(git(['rev-parse', 'HEAD']), revision, 'qualify the declared source revision');
  assert.equal(git(['status', '--porcelain']), '', 'commit the candidate and test before qualification');
  const query = `cancellationjourney${randomUUID().replaceAll('-', '')}`;
  const fixture = join(workspace, 'cancellation-recovery.md');
  const fixtureText = `${query} confirms the advisor can read files after cancellation.`;
  writeFileSync(fixture, fixtureText);
  const asked = transport.request('context/recommend', { query, limit, sources: 'ground-truth', cwd: workspace });
  await Promise.race([
    firstConnection,
    asked.response.then(answer => {
      throw new Error(`context/recommend answered before the source connected: ${JSON.stringify(answer)}`);
    }),
  ]);
  report.cases.push({ case: 'ground-truth-request-in-flight', url });
  const acknowledged = await transport.rpc('context/cancel', { requestId: asked.id });
  assert.equal(acknowledged.cancellationRequested, true, JSON.stringify(acknowledged));
  assert.equal(acknowledged.requestId, asked.id);

  const advice = await asked.response;
  const status = advice.sources.find(source => source.source === 'ground-truth');
  assert.equal(status?.available, false, JSON.stringify(advice));
  assert.ok(status.detail.includes('cancelled by its caller'), status.detail);
  assert.ok(status.detail.includes(url), status.detail);
  assert.ok(advice.unanswered.some(source => source.source === 'ground-truth'), JSON.stringify(advice));
  assert.ok(transport.command.stderr.includes(`czekam: GET ${url}`),
    `the held request must name its wait: ${transport.command.stderr}`);
  assert.ok(transport.command.stderr.includes('context advisor source ground-truth'),
    `the source must name its wait: ${transport.command.stderr}`);
  report.cases.push({ case: 'cancelled-source-answers-unanswered', verdict: 'passed', detail: status.detail });

  await assert.rejects(transport.rpc('context/cancel', { requestId: asked.id }),
    'cancelling a request that already answered must be refused');
  report.cases.push({ case: 'finished-request-cancel-refused', verdict: 'passed' });

  const recovered = await transport.rpc('context/recommend', { query, limit, sources: 'files', cwd: workspace });
  const files = recovered.sources.find(source => source.source === 'files');
  assert.equal(files?.available, true, JSON.stringify(recovered));
  const match = recovered.recommendations.find(hit => hit.source === 'files');
  assert.ok(match?.locator.startsWith(`${fixture}:`), JSON.stringify(recovered));
  assert.equal(match.snippet, fixtureText);
  report.cases.push({ case: 'connection-reads-file-after-cancellation', verdict: 'passed', advice: recovered });

  // Malformed parameters are refused by name, never replaced.
  await assert.rejects(transport.rpc('context/recommend', { query: { text: query }, cwd: workspace }),
    /invalid_params: context\/recommend query must be a string/);
  await assert.rejects(transport.rpc('context/recommend', { query, limit: 'many', cwd: workspace }),
    /invalid_params: context\/recommend limit must be a whole number/);
  await assert.rejects(transport.rpc('context/sources', { cwd: [workspace] }),
    /invalid_params: context cwd must be a path string/);
  report.cases.push({ case: 'malformed-parameters-refused', verdict: 'passed' });
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  await transport.end();
  for (const socket of held) socket.destroy();
  server.close();
  report.finishedAt = new Date().toISOString();
  persist();
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
