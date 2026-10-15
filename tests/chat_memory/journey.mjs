// Real journey: a Jeden session with the never-ending chat memory on, as the
// operator's OptChat design lays it out. One real RPC session talks through
// Brama turn after turn until its view has passed the high size and been
// merged down to the low size as many times as the run asks; every view a
// turn was given stays within the high size, and each batch brings it to at
// most the low size at once (the sawtooth). The log keeps every message word
// for word in main/YYYY-MM-DD.jsonl, the tree keeps its lines in
// tree/YYYY-MM-DD.jsonl, and the view is the list of [l, i] pairs in
// view.json. Zooming from the view's first line down to the first message
// gives its exact text, and date(id) gives when it was said. The process
// then ends: the view.json it left is the same file before the next process
// runs, and the next process continues it, never rebuilds it. A rewind
// leaves a note in the log. A chatMemory section without its summary model,
// or still naming a view size, refuses the run with the setting.
// Required: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_MODEL,
// JEDEN_TEST_SUMMARY_MODEL (the cheap model), JEDEN_TEST_MAX_STEPS and
// JEDEN_TEST_BATCHES (how many merge batches the run goes through), with the
// environment of the model route: Brama, or the OpenAI-compatible provider
// JEDEN_MODEL_ENDPOINT names; the report records which.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startRpc } from '../hooks/rpc.mjs';

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
// The step limit an RPC session's turns run under, and how many batches the
// run waits for: the operator's numbers as the run is given them.
const maxSteps = Number(required('JEDEN_TEST_MAX_STEPS'));
const wanted = Number(required('JEDEN_TEST_BATCHES'));
assert.ok(Number.isInteger(wanted) && wanted > Number.MIN_VALUE, 'JEDEN_TEST_BATCHES is a whole number of batches');
const route = process.env.JEDEN_MODEL_ENDPOINT ? `direct ${process.env.JEDEN_MODEL_ENDPOINT}` : 'brama';
// The design's numbers as Jeden declares them.
const declared = JSON.parse(readFileSync(join(root, 'rust/chat_memory/numeric-provenance.json'), 'utf8'));
const summaryBytes = declared.summary_bytes_max.value;
const high = declared.view_bytes_high.value;
const low = declared.view_bytes_low.value;
const messageLevel = declared.message_level.value;
const runs = join(root, '.build/chat-memory');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), route, model, summaryModel, summaryBytes, high, low,
  wanted,
  testSha256: hash(fileURLToPath(import.meta.url)), commands: [], frames: [], observations: {}, verdict: 'failed' };

function invoke(program, args, cwd, env) {
  const result = spawnSync(program, args, { cwd, env, encoding: 'utf8', maxBuffer: Number.MAX_SAFE_INTEGER });
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
  // The chat is the one the configuration names, never one the caller set.
  const env = { ...process.env, JEDEN_SESSION_ROOT: sessions };
  delete env.JEDEN_CHAT;
  return { directory, env, sessions };
}
const jeden = (args, place) => invoke(binary, args, place.directory, place.env);
const show = (session, place) => JSON.parse(succeeds(jeden(['chat-memory', 'show', session, '--json'], place)));
const lines = (directory) => readdirSync(directory).filter((name) => name.endsWith('.jsonl')).sort()
  .flatMap((name) => readFileSync(join(directory, name), 'utf8').split('\n').filter(Boolean)
    .map((line) => ({ day: name.replace(/\.jsonl$/, ''), record: JSON.parse(line) })));
// A view line's `id+n` name, as the view and a zoom write it.
const named = (line) => {
  const found = /^(\d+)\+(\d+)\|/.exec(line);
  assert.ok(found, `${line} is not a view line`);
  const [, id, n] = found;
  return { id, n };
};

try {
  assert.equal(succeeds(invoke('git', ['rev-parse', 'HEAD'], root, process.env)).trim(), revision);

  // A chatMemory section without the summary model, or one still naming a
  // view size the design fixes, refuses the run naming the setting.
  const lacking = project('lacking', {});
  const refused = jeden(['run', 'Respond exactly: OK', '--cwd', lacking.directory, '--model', model,
    '--model-only'], lacking);
  assert.ok(refused.status, 'a chat memory without its summary model must refuse the run');
  assert.match(refused.stderr, /chatMemory\.summaryModel is required/);
  const sized = project('sized', { summaryModel, viewBytes: high });
  const stale = jeden(['run', 'Respond exactly: OK', '--cwd', sized.directory, '--model', model, '--model-only'],
    sized);
  assert.ok(stale.status, 'a chat memory still naming viewBytes must refuse the run');
  assert.match(stale.stderr, /chatMemory\.viewBytes is no longer read/);

  // One real RPC session, turn after turn: each prompt a paragraph that
  // just fits one line, each answer that paragraph back, so the view grows
  // by about two lines a turn and its batches come within a few hundred turns.
  const chat = project('chat', { summaryModel, chat: 'journey' });
  const ask = '\nRespond with exactly the paragraph above and nothing else.';
  const paragraph = (turn) => {
    let text = `ORDER-${turn}: the operator ordered this, word for word, and it must be kept.`;
    while (Buffer.byteLength(`user: ${text} Kept.${ask}`) <= summaryBytes) text += ' Kept.';
    return text;
  };
  const sizes = [];
  const batches = [];
  let session;
  let firstPrompt;
  const client = startRpc({ binary, cwd: chat.directory, env: chat.env, report });
  try {
    const opened = await client.rpc('session/new', { options: { cwd: chat.directory, model, maxSteps } });
    session = opened.sessionPath;
    let previous;
    while (batches.length < wanted) {
      const prompt = `${paragraph(sizes.length)}${ask}`;
      firstPrompt ??= prompt;
      await client.rpc('session/prompt', { sessionId: opened.sessionId, prompt });
      const size = show(session, chat).view.bytes;
      // The view a turn is given never passes the high size: past it, the
      // turn merges it down before it is sent.
      assert.ok(size <= high, `turn ${sizes.length}: the view took ${size} bytes, over ${high}`);
      if (previous !== undefined && size < previous) {
        // A batch: one turn takes the view from near the high size to at most
        // the low size at once.
        assert.ok(size <= low, `turn ${sizes.length}: the view shrank to ${size}, not to at most ${low}`);
        batches.push(sizes.length);
      }
      sizes.push(size);
      previous = size;
    }
    report.observations.sizes = sizes;
    report.observations.batches = batches;
  } finally {
    await client.end();
  }

  // The layout the design names, in the chat beside the session root.
  const memory = join(chat.directory, 'chats', 'journey');
  const logged = lines(join(memory, 'main'));
  const built = lines(join(memory, 'tree'));
  report.observations.logged = logged.length;
  report.observations.built = built.length;
  logged.forEach(({ day, record }, at) => {
    assert.equal(record.i, at, 'messages are numbered in order');
    assert.equal(record.size, Buffer.byteLength(record.text), 'a message records its size');
    assert.ok(record.date.startsWith(`${day}T`) && record.date.endsWith('Z') && !Number.isNaN(Date.parse(record.date)),
      `message ${record.i} sits in the file of the day it was said`);
  });
  const [firstLogged] = logged;
  assert.equal(firstLogged.record.kind, 'user');
  assert.equal(firstLogged.record.text, firstPrompt, 'the log keeps the first message word for word');
  assert.ok(built.every(({ record }) => record.size === Buffer.byteLength(record.text)));
  assert.ok(built.some(({ record }) => record.l > messageLevel), 'the batches merged lines into parents');
  const pairs = JSON.parse(readFileSync(join(memory, 'view.json'), 'utf8'));
  assert.ok(Array.isArray(pairs) && pairs.every(([l, i, ...more]) => Number.isInteger(l) && Number.isInteger(i)
    && !more.length), 'view.json is the view as [l, i] pairs');

  // Zoom from the view's first line down to the first message.
  const shown = show(session, chat);
  let { id, n } = named(shown.view.text.split('\n').find((line) => line.includes('|')));
  const path = [];
  let opened = JSON.parse(succeeds(jeden(['chat-memory', 'zoom', session, id, n, '--json'], chat)));
  path.push(opened);
  while (opened.lines) {
    const [left, right, ...more] = opened.lines;
    assert.ok(left && right && !more.length, `${id}+${n} opens into two lines`);
    ({ id, n } = named(left));
    opened = JSON.parse(succeeds(jeden(['chat-memory', 'zoom', session, id, n, '--json'], chat)));
    path.push(opened);
  }
  report.observations.zoom = path;
  assert.equal(opened.message.i, firstLogged.record.i, 'the zoom ends at the first message');
  assert.equal(opened.message.text, firstPrompt, 'zoom gives the first message word for word');
  const dated = JSON.parse(succeeds(jeden(['chat-memory', 'date', session, id, '--json'], chat)));
  assert.equal(dated.date, firstLogged.record.date, 'date gives when the first message was said');

  // The process ended; its view is kept as it was, and the next process
  // continues it: the earlier lines stay, in place, unless that turn batched.
  const kept = readFileSync(join(memory, 'view.json'), 'utf8');
  const resumed = ['--cwd', chat.directory, '--model', model, '--model-only', '--resume-session', session];
  succeeds(jeden(['run', 'Respond exactly: CONTINUED', ...resumed], chat));
  const after = show(session, chat);
  report.observations.after = after.view;
  const before = JSON.parse(kept);
  if (after.view.bytes >= shown.view.bytes) {
    assert.ok(before.every((pair, at) => JSON.stringify(after.view.pairs[at]) === JSON.stringify(pair)),
      'the next process continued the kept view instead of rebuilding it');
  } else {
    assert.ok(after.view.bytes <= low, 'a view that shrank on resume shrank by a batch');
  }

  // A rewind keeps the log as it is and leaves a note naming what was taken back.
  const checkpointed = succeeds(jeden(['run', '/checkpoint memory-journey', ...resumed], chat));
  const found = /Checkpoint (\S+) created/.exec(checkpointed);
  assert.ok(found, `the checkpoint names no id: ${checkpointed}`);
  const [, checkpoint] = found;
  succeeds(jeden(['run', 'Respond exactly: BEFORE-REWIND', ...resumed], chat));
  report.observations.rewound = succeeds(jeden(['run', `/rewind ${checkpoint}`, ...resumed], chat));
  succeeds(jeden(['run', 'Respond exactly: AFTER-REWIND', ...resumed], chat));
  const final = lines(join(memory, 'main')).map(({ record }) => record);
  assert.ok(final.some((record) => record.text.includes('BEFORE-REWIND')), 'the log lost the turn taken back');
  assert.ok(final.some((record) => record.kind === 'note' && record.text.startsWith('rewind:')),
    'the rewind left no note');
  assert.ok(final.some((record) => record.text.includes('AFTER-REWIND')), 'the log lost the turn after the rewind');
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
