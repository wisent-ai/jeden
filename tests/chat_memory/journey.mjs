// Real journey: a Jeden session with the never-ending chat memory on.
// Real turns of one session through Brama, each resumed from the session the
// previous run left, go on until the view the operator declared has been
// outgrown and the summary tree has a parent: the log keeps every message
// word for word, the cheap model he named writes the summaries, the view
// stays within its size, a zoom answers a message word for word, a date
// recall answers the messages of a time, and the kept view carries over to
// the next run. A chatMemory section without its summary model refuses the
// run with the setting it lacks.
// Required: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_MODEL,
// JEDEN_TEST_SUMMARY_MODEL (the cheap Brama route) and JEDEN_TEST_VIEW_BYTES
// (the operator's view size), with the normal Brama environment.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
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
const model = required('JEDEN_TEST_MODEL');
const summaryModel = required('JEDEN_TEST_SUMMARY_MODEL');
const viewBytes = Number(required('JEDEN_TEST_VIEW_BYTES'));
assert.ok(Number.isInteger(viewBytes), 'JEDEN_TEST_VIEW_BYTES is a whole number of bytes');
assert.ok(!process.env.JEDEN_MODEL_ENDPOINT, 'the journey requires the real Brama route');
// The summary size is the operator's number Jeden declares; the journey's
// prompts are made longer than it, so the cheap model has to write.
const summaryBytes = JSON.parse(readFileSync(join(root, 'rust/chat_memory/numeric-provenance.json'), 'utf8'))
  .summary_bytes_max.value;
const runs = join(root, '.build/chat-memory');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), model, summaryModel, viewBytes, summaryBytes,
  testSha256: hash(fileURLToPath(import.meta.url)), commands: [], observations: {}, verdict: 'failed' };

function invoke(program, args, cwd, env) {
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
function project(name, chatMemory) {
  const directory = join(output, name);
  mkdirSync(join(directory, '.jeden'), { recursive: true });
  writeFileSync(join(directory, '.jeden/config.json'), JSON.stringify({ chatMemory }));
  const sessions = join(directory, 'sessions');
  mkdirSync(sessions, { recursive: true });
  return { directory, env: { ...process.env, JEDEN_SESSION_ROOT: sessions }, sessions };
}
const jeden = (args, place) => invoke(binary, args, place.directory, place.env);

try {
  assert.equal(succeeds(invoke('git', ['rev-parse', 'HEAD'], root, process.env)).trim(), revision);

  // A chatMemory section that leaves out the summary model refuses the run.
  const lacking = project('lacking', { viewBytes });
  const refused = jeden(['run', 'Respond exactly: OK', '--cwd', lacking.directory, '--model', model,
    '--model-only'], lacking);
  assert.ok(refused.status, 'a chat memory without its summary model must refuse the run');
  assert.match(refused.stderr, /chatMemory\.summaryModel is required/);

  // Real turns of one session, each prompt longer than one summary, until the
  // view has outgrown its size and two nodes have merged.
  let filler = '';
  while (Buffer.byteLength(filler) <= summaryBytes) {
    filler += 'The operator ordered this, word for word, and it must be kept. ';
  }
  const chat = project('chat', { summaryModel, viewBytes });
  const orders = [];
  let session;
  let shown;
  while (!shown?.nodes.some((node) => node.children?.length)) {
    const order = `ORDER-${orders.length}`;
    orders.push(order);
    const args = ['run', `${filler} ${order}. Respond exactly: ${order}`, '--cwd', chat.directory,
      '--model', model, '--model-only'];
    if (session) args.push('--resume-session', session);
    succeeds(jeden(args, chat));
    [session] = readdirSync(chat.sessions).map((name) => join(chat.sessions, name));
    shown = JSON.parse(succeeds(jeden(['chat-memory', 'show', session, '--json'], chat)));
    assert.ok(shown.view.bytes <= viewBytes, 'the view stays within the operator\'s size');
  }
  report.observations.orders = orders;
  report.observations.shown = shown;
  assert.ok(shown.log.messages >= orders.length + orders.length, 'the log keeps every prompt and answer');
  assert.ok(shown.view.frontier.length, 'the outgrown view folded messages into the tree');
  assert.ok(shown.nodes.some((node) => node.by === 'model'), 'the cheap model wrote summaries');
  assert.ok(shown.nodes.every((node) => Buffer.byteLength(node.summary) <= shown.summaryBytes));

  const leaves = shown.nodes.filter((node) => !node.children?.length);
  const oldest = Math.min(...leaves.map((node) => node.first));
  const leaf = leaves.find((node) => node.first === oldest);
  const zoomed = JSON.parse(succeeds(jeden(['chat-memory', 'zoom', session, leaf.id, '--json'], chat)));
  report.observations.zoomed = zoomed;
  assert.equal(zoomed.message.index, leaf.first);
  assert.ok(String(zoomed.message.content).includes('ORDER-'), 'a zoom answers the message word for word');

  const dated = JSON.parse(succeeds(jeden(['chat-memory', 'dates', session, leaf.from, leaf.to, '--json'], chat)));
  assert.ok(dated.messages.some((message) => message.index === leaf.first),
    'a date recall answers the messages of that time');

  // The kept view carries over: one more run resumes from it, never restarts it.
  succeeds(jeden(['run', 'Respond exactly: CONTINUED', '--cwd', chat.directory, '--model', model, '--model-only',
    '--resume-session', session], chat));
  const after = JSON.parse(succeeds(jeden(['chat-memory', 'show', session, '--json'], chat)));
  report.observations.after = after.view;
  assert.ok(after.log.messages > shown.log.messages, 'the resumed run appended to the same log');
  assert.ok(after.view.tailStart >= shown.view.tailStart, 'the resumed run continued the kept view');
  assert.ok(after.view.bytes <= viewBytes);

  // A turn that calls a tool keeps the call and its result whole, and the
  // next turn, given them back from the memory, is accepted by the provider.
  succeeds(jeden(['run', 'Call list_dir on . once, then respond exactly: LISTED', '--cwd', chat.directory,
    '--model', model, '--resume-session', session], chat));
  const everything = JSON.parse(succeeds(jeden(['chat-memory', 'dates', session, leaf.from,
    String(Number.MAX_SAFE_INTEGER), '--json'], chat)));
  report.observations.toolTurn = everything.messages.filter((message) => message.tool_call_id || message.tool_calls);
  assert.ok(everything.messages.some((message) => message.tool_calls),
    'the assistant turn keeps the tool calls it made');
  assert.ok(everything.messages.some((message) => message.role === 'tool' && message.tool_call_id),
    'the tool result keeps the call it answers');
  succeeds(jeden(['run', 'Respond exactly: AFTER-TOOLS', '--cwd', chat.directory, '--model', model, '--model-only',
    '--resume-session', session], chat));
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
