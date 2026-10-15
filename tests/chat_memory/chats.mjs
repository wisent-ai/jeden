// Real journey: one never-ending chat per agent, across its sessions. A
// session is told a code word; a NEW session of the same chat (another
// process, another session directory, no resume) is asked for it and answers
// it from the chat's view: the agent did not start from nothing. While one
// process writes a chat, a second one is refused naming the chat, its lock
// and how to name another chat, and `jeden chat-memory list` says the chat
// is written now. A session's chat is found from the session. An earlier
// session imported into a new chat comes in as notes, once; the new chat's
// next session answers from it. Names that are not chat names are refused
// before any model call. Everything runs through the real binary and the
// model route the environment names: Brama, or the OpenAI-compatible
// provider JEDEN_MODEL_ENDPOINT names; the report records which.
// Required: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_MODEL,
// JEDEN_TEST_SUMMARY_MODEL, with that route's environment.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { networkInterfaces } from 'node:os';
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
const route = process.env.JEDEN_MODEL_ENDPOINT ? `direct ${process.env.JEDEN_MODEL_ENDPOINT}` : 'brama';
const runs = join(root, '.build/chat-memory');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'chats-'));
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), route, model, summaryModel,
  testSha256: hash(fileURLToPath(import.meta.url)), commands: [], frames: [], observations: {}, verdict: 'failed' };

function invoke(args, place, extra = {}) {
  const env = { ...place.env, ...extra };
  const result = spawnSync(binary, args, { cwd: place.directory, env, encoding: 'utf8',
    maxBuffer: Number.MAX_SAFE_INTEGER });
  report.commands.push({ args, extra, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  return result;
}
const succeeds = (result) => {
  assert.ok(result.status !== null && !result.status, result.stderr);
  return result.stdout;
};
const refused = (result, sentence) => {
  assert.ok(result.status, `expected a refusal matching ${sentence}, got: ${result.stdout}`);
  assert.match(result.stderr, sentence);
};
// One workspace with its own session root, so its chats live beside it.
function place(name, chatMemory) {
  const directory = join(output, name);
  mkdirSync(join(directory, '.jeden'), { recursive: true });
  writeFileSync(join(directory, '.jeden/config.json'), JSON.stringify({ chatMemory }));
  const env = { ...process.env, JEDEN_SESSION_ROOT: join(directory, 'sessions') };
  delete env.JEDEN_CHAT;
  return { directory, env, chats: join(directory, 'chats') };
}
const ask = (prompt, at, extra) => invoke(['run', prompt, '--cwd', at.directory, '--model', model, '--model-only'],
  at, extra);
const logged = (chat) => readdirSync(join(chat, 'main')).filter((name) => name.endsWith('.jsonl')).sort()
  .flatMap((name) => readFileSync(join(chat, 'main', name), 'utf8').split('\n').filter(Boolean).map(JSON.parse));
const sessions = (at) => readdirSync(join(at.directory, 'sessions')).map((name) => join(at.directory, 'sessions', name))
  .filter((dir) => existsSync(join(dir, 'chat-memory.json')));
const chatOf = (session) => JSON.parse(readFileSync(join(session, 'chat-memory.json'), 'utf8')).chat;
const json = (result) => JSON.parse(succeeds(result));
// A run still going when the journey stops is ended, and its output and exit
// go into the report with the rest.
let running = null;

try {
  assert.equal(spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim(), revision);

  // Names that are not chat names are refused before any model call.
  const escaping = place('escaping', { summaryModel, chat: '../escape' });
  refused(ask('Respond exactly: OK', escaping), /chatMemory\.chat names the chat `\.\.\/escape`, which is not a chat name/);
  const spaced = place('spaced', { summaryModel });
  refused(ask('Respond exactly: OK', spaced, { JEDEN_CHAT: 'two words' }),
    /JEDEN_CHAT names the chat `two words`, which is not a chat name/);
  assert.ok(!existsSync(escaping.chats) && !existsSync(spaced.chats), 'a refused name created a chat');
  refused(invoke(['chat-memory', 'forget', 'agent'], spaced), /jeden chat-memory takes one of/);

  // A session is told a code word; a new session of the same chat answers it.
  const work = place('work', { summaryModel, chat: 'agent' });
  const chat = join(work.chats, 'agent');
  const word = `KOD-${randomBytes(6).toString('hex').toUpperCase()}`;
  const told = `Remember this code word for later: ${word}. Respond exactly: NOTED`;
  assert.match(succeeds(ask(told, work)), /NOTED/);
  const [first] = sessions(work);
  assert.equal(chatOf(first), chat);
  const [opening] = logged(chat);
  assert.equal(opening.text, told, 'the chat log keeps the first message word for word');
  const record = JSON.parse(readFileSync(join(chat, 'chat.json'), 'utf8'));
  assert.equal(record.name, 'agent');
  assert.equal(record.workspace, work.directory);
  const recalled = succeeds(ask('What code word did I ask you to remember? Answer with the code word only.', work));
  report.observations.recalled = recalled;
  assert.ok(recalled.includes(word), `the next session did not answer the code word ${word}: ${recalled}`);
  const [one, two, ...more] = sessions(work);
  assert.ok(one && two && !more.length && one !== two, 'each run is its own session');
  assert.ok(chatOf(one) === chat && chatOf(two) === chat, 'both sessions write to the one chat');
  const shown = json(invoke(['chat-memory', 'show', first, '--json'], work));
  assert.equal(shown.chat.name, 'agent', 'a session names its chat');
  assert.equal(shown.log.messages, logged(chat).length);
  refused(invoke(['chat-memory', 'show', 'no-such-chat'], work),
    /is neither a chat under .* nor a session that wrote to a chat/);

  // While one process writes the chat, a second is refused naming it. The
  // writer is a real run whose model requests pass through a gate in this
  // test: its first request reaches the gate only after the run holds the
  // chat, and the gate forwards it to the real route, unchanged, once the
  // second process was refused. The run's answer comes from the real model.
  const routeVariable = process.env.JEDEN_MODEL_ENDPOINT ? 'JEDEN_MODEL_ENDPOINT' : 'BRAMA_URL';
  const upstream = required(routeVariable).replace(/\/+$/, '');
  const loopback = Object.values(networkInterfaces()).flat()
    .find((item) => item.internal && item.family === 'IPv4')?.address;
  assert.ok(loopback, 'this machine reports no loopback interface for the gate');
  let release;
  const released = new Promise((resolveRelease) => { release = resolveRelease; });
  let arrived;
  const gated = new Promise((resolveArrived) => { arrived = resolveArrived; });
  const gate = createServer(async (request, response) => {
    const body = [];
    for await (const chunk of request) body.push(chunk);
    if (request.method === 'POST') {
      arrived();
      await released;
    }
    const headers = Object.fromEntries(Object.entries(request.headers)
      .filter(([name]) => !['host', 'connection', 'content-length'].includes(name)));
    const forwarded = await fetch(`${upstream}${request.url}`, { method: request.method, headers,
      body: request.method === 'POST' ? Buffer.concat(body) : undefined });
    response.writeHead(forwarded.status, Object.fromEntries(forwarded.headers));
    for await (const chunk of forwarded.body) response.write(chunk);
    response.end();
  });
  // No port named: the system gives the gate an unused one.
  await new Promise((resolveListen) => gate.listen(undefined, loopback, resolveListen));
  const holding = { args: ['run', 'Respond exactly: HELD', '--cwd', work.directory, '--model', model, '--model-only'],
    gate: `${routeVariable} through ${upstream}`, stdout: '', stderr: '' };
  report.commands.push(holding);
  const holder = spawn(binary, holding.args, { cwd: work.directory,
    env: { ...work.env, [routeVariable]: `http://${loopback}:${gate.address().port}` } });
  holder.stdout.on('data', (data) => { holding.stdout += data; });
  holder.stderr.on('data', (data) => { holding.stderr += data; });
  const ended = new Promise((resolveEnd) => holder.on('close', (status, signal) => {
    holding.status = status;
    holding.signal = signal;
    holding.endedAt = new Date().toISOString();
    resolveEnd(status);
  }));
  running = { holder, ended, holding, gate, release };
  await Promise.race([gated, ended.then((status) => {
    throw new Error(`the holding run ended with ${status} before its model request: ${holding.stderr}`);
  })]);
  refused(ask('Respond exactly: OK', work),
    /the chat `agent` is written by another process now \(it holds .*owner\.lock\); one process writes a chat, so give this agent its own with JEDEN_CHAT=<name> or chatMemory\.chat/);
  const listed = json(invoke(['chat-memory', 'list', '--json'], work));
  assert.equal(listed.chats.find((item) => item.name === 'agent')?.writtenNow, true,
    'list says the chat is written now');
  assert.match(succeeds(ask('Respond exactly: OTHER', work, { JEDEN_CHAT: 'other' })), /OTHER/);
  assert.ok(existsSync(join(work.chats, 'other', 'main')), 'JEDEN_CHAT named its own chat');
  release();
  await ended;
  gate.close();
  assert.ok(holding.status !== null && !holding.status, `the holding run failed: ${holding.stderr}`);
  assert.match(holding.stdout, /held/i, 'the holding run answered from the real model');
  const after = json(invoke(['chat-memory', 'list', '--json'], work));
  report.observations.listed = after;
  assert.deepEqual(after.chats.map((item) => item.name), ['agent', 'other']);
  assert.ok(after.chats.every((item) => item.writtenNow === false), 'no chat is written once the processes ended');

  // A resumed session keeps the chat it started in: JEDEN_CHAT names the chat
  // of a new session, so a launcher that sets it for every start leaves a
  // resumed session in its own chat; a JEDEN_CHAT that is no name is refused.
  const resumed = ['--cwd', work.directory, '--model', model, '--model-only', '--resume-session', first];
  refused(invoke(['run', 'Respond exactly: OK', ...resumed], work, { JEDEN_CHAT: '.hidden' }),
    /JEDEN_CHAT names the chat `\.hidden`, which is not a chat name/);
  const before = { agent: logged(chat).length, other: logged(join(work.chats, 'other')).length };
  succeeds(invoke(['run', 'Respond exactly: AGAIN', ...resumed], work, { JEDEN_CHAT: 'other' }));
  assert.ok(logged(chat).length > before.agent, 'the resumed session went on in its own chat');
  assert.equal(logged(join(work.chats, 'other')).length, before.other, 'the resumed session wrote nothing to `other`');

  // An earlier session comes into a new chat as notes, once.
  const imported = json(invoke(['chat-memory', 'import', 'imported', first, '--json'], work));
  report.observations.imported = imported;
  const notes = logged(join(work.chats, 'imported'));
  assert.equal(imported.messages, notes.length);
  assert.ok(notes.every((note) => note.kind === 'note' && note.text.startsWith('[imported from session ')));
  assert.ok(notes.some((note) => note.text.includes('] user: ') && note.text.includes(word)),
    'the import kept the user message with the code word');
  assert.ok(notes.some((note) => note.text.includes('] jeden: ')), 'the import kept the answer');
  refused(invoke(['chat-memory', 'import', 'imported', first], work), /it is not imported twice/);
  refused(invoke(['chat-memory', 'import', 'imported', join(output, 'no-session')], work), /no-session/);
  const fromImport = succeeds(ask('What code word did I ask you to remember? Answer with the code word only.', work,
    { JEDEN_CHAT: 'imported' }));
  report.observations.fromImport = fromImport;
  assert.ok(fromImport.includes(word), `the imported chat did not answer the code word ${word}: ${fromImport}`);
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  if (running && !running.holding.endedAt) running.holder.kill();
  if (running) {
    await running.ended;
    running.release();
    running.gate.close();
  }
  writeFileSync(join(output, 'report.json'), JSON.stringify(report));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
