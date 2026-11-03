// Real journey: the GitHub star request Jeden makes once, after the fifth
// successful agent prompt, against a real OpenAI-compatible model provider.
// It counts `jeden run` prompts and an RPC `session/prompt`, reads the record
// Jeden keeps (~/.jeden/star.json in an isolated home) after every step,
// checks that the request is shown on the fifth prompt and on no other, that
// --model-only and slash commands do not count, and exercises every verb and
// refusal of `jeden star`, `/star` and the `star/*` RPC methods.
// Inputs: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_PROVIDER_ENDPOINT,
// JEDEN_TEST_MODEL; JEDEN_TEST_PROVIDER_TOKEN_A when the provider takes a key.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { commands } from '../model/credentials/commands.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const area = join(root, '.build/star', randomUUID());
const home = join(area, 'home');
const workspace = join(area, 'workspace');
const sessions = join(area, 'sessions');
for (const directory of [home, workspace, sessions]) mkdirSync(directory, { recursive: true });
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const sensitive = [];
const redacted = (value) => value === undefined ? undefined : sensitive.reduce(
  (text, secret) => text.replaceAll(secret, '<redacted>'), value);
const report = { testSha256: hash(fileURLToPath(import.meta.url)), commands: [], cases: [], verdict: 'failed' };
const required = (name) => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} must name a real provider journey input`);
  return value;
};
const { invoke, succeeds, refuses } = commands({ root, area, report, redacted });
const record = join(home, '.jeden/star.json');
const kept = () => JSON.parse(readFileSync(record, 'utf8'));
const REQUEST = 'a star on GitHub helps other people find it';

// One RPC conversation with `jeden rpc`: requests are written in order and
// each answer is read by its id.
async function rpc(binary, env, requests) {
  const child = spawn(binary, ['rpc'], { cwd: workspace, env, stdio: ['pipe', 'pipe', 'pipe'] });
  const answers = new Map();
  let buffer = '';
  let stderr = '';
  const waiting = new Map();
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => { stderr += chunk; });
  child.stdout.on('data', (chunk) => {
    buffer += chunk;
    let newline;
    while ((newline = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, newline);
      buffer = buffer.slice(newline + 1);
      if (!line.trim()) continue;
      const frame = JSON.parse(line);
      if (frame.id === undefined || frame.id === null) continue;
      answers.set(frame.id, frame);
      waiting.get(frame.id)?.(frame);
    }
  });
  const exited = new Promise((resolveExit) => child.once('close', (code, signal) => resolveExit({ code, signal })));
  const ask = (request) => new Promise((resolveAnswer, reject) => {
    if (answers.has(request.id)) return resolveAnswer(answers.get(request.id));
    waiting.set(request.id, resolveAnswer);
    exited.then(({ code, signal }) => reject(new Error(
      `jeden rpc exited (${code ?? signal}) before answering ${request.id}: ${redacted(stderr)}`)));
    child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', ...request })}\n`);
  });
  const transcript = [];
  try {
    for (const request of requests) {
      const params = typeof request.params === 'function' ? request.params(transcript) : request.params;
      const answer = await ask({ ...request, params });
      transcript.push({ request: { ...request, params }, answer });
    }
  } finally {
    child.stdin.end(`${JSON.stringify({ jsonrpc: '2.0', id: 'shutdown', method: 'shutdown', params: {} })}\n`);
    const { code, signal } = await exited;
    report.commands.push({ program: binary, args: ['rpc'], cwd: workspace, status: code, signal,
      stderr: redacted(stderr), transcript: JSON.parse(redacted(JSON.stringify(transcript))) });
  }
  return transcript.map(({ answer }) => answer);
}

try {
  const binary = resolve(required('JEDEN_BIN'));
  const revision = required('JEDEN_TEST_SOURCE_REVISION');
  const endpoint = required('JEDEN_TEST_PROVIDER_ENDPOINT').replace(/\/+$/, '');
  const model = required('JEDEN_TEST_MODEL');
  const token = process.env.JEDEN_TEST_PROVIDER_TOKEN_A?.trim();
  if (token) sensitive.push(token);
  report.revision = revision;
  report.binarySha256 = hash(binary);
  report.endpoint = endpoint;
  report.model = model;
  assert.equal(succeeds(invoke('git', ['rev-parse', 'HEAD'], process.env)).trim(), revision);

  // No Brama route, no Stado: Jeden calls the named provider directly.
  const env = { ...process.env, HOME: home, JEDEN_SESSION_ROOT: sessions, JEDEN_MODEL_ENDPOINT: endpoint,
    PATH: '/usr/bin:/bin' };
  for (const name of ['BRAMA_URL', 'BRAMA_TOKEN', 'WISENT_APP_AGENT_AUTH_SECRET', 'JEDEN_MODEL_KEY',
    'JEDEN_DATABASE_URL', 'JEDEN_LEASE']) delete env[name];
  if (token) env.JEDEN_MODEL_KEY = token;
  const prompt = (text) => invoke(binary, ['run', text, '--model', model, '--cwd', workspace], env);
  const star = (...words) => JSON.parse(succeeds(invoke(binary, ['star', ...words, '--json'], env)));

  // 1. Nothing recorded yet: counting from zero, nothing written by a read.
  let status = star();
  assert.equal(status.stage, 'counting');
  assert.equal(status.prompts, 0);
  assert.equal(status.threshold, 5);
  assert.equal(status.repository, 'https://github.com/wisent-ai/jeden');
  assert.equal(status.file, record);
  assert.ok(!existsSync(record), 'reading the status must not write a record');
  report.cases.push({ name: 'fresh status reads zero and writes nothing', status });

  // 2. Refusals of the command itself, each an exact sentence and exit 2.
  const unknown = invoke(binary, ['star', 'later'], env);
  assert.equal(unknown.status, 2);
  refuses(unknown, 'star: unknown action "later"; usage: jeden star [status|dismiss|reset] [--json]');
  const extra = invoke(binary, ['star', 'status', 'now'], env);
  assert.equal(extra.status, 2);
  refuses(extra, 'star: unexpected argument "now"; usage: jeden star [status|dismiss|reset] [--json]');
  report.cases.push({ name: 'unknown action and extra argument are usage refusals' });

  // 3. A model-only call and a slash command are not agent prompts.
  succeeds(invoke(binary, ['run', 'Respond exactly: OK', '--model', model, '--model-only', '--cwd', workspace], env));
  succeeds(invoke(binary, ['run', '/star', '--cwd', workspace], env));
  assert.ok(!existsSync(record), '--model-only and a slash command must not count');
  report.cases.push({ name: '--model-only and /star do not count' });

  // 4. Four real prompts count without the request; the fifth shows it once.
  for (let sent = 1; sent <= 4; sent += 1) {
    const answer = prompt(`Respond with the single word: ${['one', 'two', 'three', 'four'][sent - 1]}`);
    succeeds(answer);
    assert.ok(!answer.stderr.includes(REQUEST), `prompt ${sent} must not show the request`);
    assert.deepEqual(kept(), { prompts: sent });
  }
  const fifth = prompt('Respond with the single word: five');
  const fifthAnswer = succeeds(fifth);
  assert.ok(fifth.stderr.includes(REQUEST), `the fifth prompt must show the request: ${fifth.stderr}`);
  assert.ok(fifth.stderr.includes('https://github.com/wisent-ai/jeden'));
  assert.ok(!fifthAnswer.includes(REQUEST), 'the request must stay off standard output');
  const shown = kept();
  const slash = JSON.parse(succeeds(invoke(binary, ['run', '/star status --json', '--cwd', workspace], env)));
  assert.equal(slash.prompts, 6);
  assert.equal(slash.stage, 'shown');
  assert.equal(star().stage, 'shown');
  report.cases.push({ name: 'fifth real prompt shows the request on stderr, once', record: shown });

  // 5. A sixth prompt counts and shows nothing.
  const sixth = prompt('Respond with the single word: six');
  succeeds(sixth);
  assert.ok(!sixth.stderr.includes(REQUEST), 'the request is shown once');
  assert.equal(kept().prompts, 6);
  assert.equal(kept().shownAt, shown.shownAt);
  report.cases.push({ name: 'sixth prompt counts without a second request' });

  // 6. /star inside a session reads the same record.
  const slash = JSON.parse(succeeds(invoke(binary, ['run', '/star status --json', '--cwd', workspace], env)));
  assert.equal(JSON.parse(slash.text ?? slash).prompts ?? JSON.parse(slash.text).prompts, 6);
  report.cases.push({ name: '/star reads the same record' });

  // 7. Reset restarts the count; RPC prompts count and the fifth carries
  //    starRequest, which is what Jeden Desktop shows.
  status = star('reset');
  assert.equal(status.stage, 'counting');
  assert.deepEqual(kept(), { prompts: 0 });
  succeeds(invoke(binary, ['run', 'Respond with the single word: a', '--model', model, '--cwd', workspace], env));
  succeeds(invoke(binary, ['run', 'Respond with the single word: b', '--model', model, '--cwd', workspace], env));
  succeeds(invoke(binary, ['run', 'Respond with the single word: c', '--model', model, '--cwd', workspace], env));
  succeeds(invoke(binary, ['run', 'Respond with the single word: d', '--model', model, '--cwd', workspace], env));
  assert.equal(kept().prompts, 4);
  const [opened, slashed, prompted, again, refusedParams, refusedMethod, statusAnswer] = await rpc(binary, env, [
    { id: 'open', method: 'session/new', params: { cwd: workspace, model } },
    { id: 'slash', method: 'session/prompt', params: (seen) => ({ sessionId: seen[0].answer.result.sessionId,
      requestId: 'slash', prompt: '/star status' }) },
    { id: 'fifth', method: 'session/prompt', params: (seen) => ({ sessionId: seen[0].answer.result.sessionId,
      requestId: 'fifth', prompt: 'Respond with the single word: e' }) },
    { id: 'sixth', method: 'session/prompt', params: (seen) => ({ sessionId: seen[0].answer.result.sessionId,
      requestId: 'sixth', prompt: 'Respond with the single word: f' }) },
    { id: 'params', method: 'star/status', params: { extra: true } },
    { id: 'method', method: 'star/later', params: {} },
    { id: 'status', method: 'star/status', params: {} },
  ]);
  assert.ok(opened.result?.sessionId, JSON.stringify(opened));
  assert.ok(slashed.result, JSON.stringify(slashed));
  assert.equal(slashed.result.starRequest, undefined, 'a session control does not count');
  assert.ok(prompted.result, JSON.stringify(prompted));
  assert.equal(prompted.result.starRequest?.prompts, 5, JSON.stringify(prompted.result));
  assert.equal(prompted.result.starRequest.repository, 'https://github.com/wisent-ai/jeden');
  assert.ok(prompted.result.starRequest.text.includes(REQUEST));
  assert.equal(again.result.starRequest, undefined, 'RPC shows the request once');
  assert.equal(refusedParams.error?.code ?? refusedParams.error?.data?.code, refusedParams.error?.code);
  assert.match(JSON.stringify(refusedParams.error), /invalid_params/);
  assert.match(JSON.stringify(refusedParams.error), /star\/status takes no parameters/);
  assert.match(JSON.stringify(refusedMethod.error), /method_not_found/);
  assert.match(JSON.stringify(refusedMethod.error), /star answers star\/status, star\/dismiss and star\/reset/);
  assert.equal(statusAnswer.result.stage, 'shown');
  assert.equal(statusAnswer.result.prompts, 6);
  report.cases.push({ name: 'RPC prompt carries starRequest on the fifth prompt only; star/* refusals',
    starRequest: prompted.result.starRequest });

  // 8. Dismiss through RPC, as Jeden Desktop does; it holds after a reset only
  //    until the count reaches five again.
  const [dismissed] = await rpc(binary, env, [{ id: 'star-dismiss', method: 'star/dismiss', params: {} }]);
  assert.equal(dismissed.result.stage, 'dismissed');
  assert.equal(typeof kept().dismissedAt, 'number');
  const afterDismiss = prompt('Respond with the single word: seven');
  succeeds(afterDismiss);
  assert.ok(!afterDismiss.stderr.includes(REQUEST));
  assert.equal(star().stage, 'dismissed');
  report.cases.push({ name: 'star/dismiss stops the request', record: kept() });

  // 9. A record that is not a star record is refused naming the file and the
  //    way out; a prompt still answers and says why it was not counted.
  writeFileSync(record, '{"prompts":"many"}\n');
  const broken = invoke(binary, ['star', '--json'], env);
  assert.equal(broken.status, 1);
  refuses(broken, `${record} is not a star record`);
  refuses(broken, '`jeden star reset` writes a new one');
  const uncounted = prompt('Respond with the single word: eight');
  succeeds(uncounted);
  assert.ok(uncounted.stderr.includes('jeden: this prompt is not counted toward the GitHub star request:'),
    uncounted.stderr);
  assert.equal(readFileSync(record, 'utf8'), '{"prompts":"many"}\n', 'a broken record is not overwritten by a prompt');
  status = star('reset');
  assert.deepEqual(kept(), { prompts: 0 });
  report.cases.push({ name: 'broken record refused; prompt still answers; reset recovers' });

  report.verdict = 'passed';
} catch (error) {
  report.error = redacted(String(error.stack || error));
  throw error;
} finally {
  writeFileSync(join(area, 'report.json'), JSON.stringify(report, (_, value) =>
    typeof value === 'string' ? redacted(value) : value, '  '));
  console.log(`${report.verdict}: ${join(area, 'report.json')}`);
}
