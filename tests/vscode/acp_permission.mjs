// Real journey: the VS Code extension's ACP client, built from this checkout,
// drives the real `jeden acp` agent through a turn that writes a file. Under
// the default approval mode (always-ask) the agent asks the client's
// permission for the write; the client must accept the request whole — the
// session id it opened, options each carrying optionId, name and kind — and
// never answer it with a refusal for a missing field. Choosing the agent's
// allow option lets the write land, which is checked on disk.
//
// The client refuses a request that lacks a field rather than filling it in;
// this journey proves that refusal does not touch the agent's real requests.
// Every step, the requests the client was shown and its log are kept in
// report.json under .build.
//
// Run: npm --prefix packages/vscode-jeden run build && \
//   JEDEN_BIN=target/debug/jeden JEDEN_TEST_SOURCE_REVISION=$(git rev-parse HEAD) \
//   node tests/vscode/acp_permission.mjs
// The signed-in HOME of the runner supplies the model route the turn needs.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = resolve(required('JEDEN_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const built = join(root, 'packages/vscode-jeden/dist/src');
assert.ok(existsSync(join(built, 'acp/client.js')), `build the client first: npm --prefix packages/vscode-jeden run build (looked in ${built})`);
const { JedenAcpClient } = await import(pathToFileURL(join(built, 'acp/client.js')).href);
const { StdioAcpTransport } = await import(pathToFileURL(join(built, 'acp/transport.js')).href);
const { RedactingLogger } = await import(pathToFileURL(join(built, 'logging.js')).href);

const runs = join(root, '.build/vscode-acp-permission');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const project = join(output, 'project');
mkdirSync(project, { recursive: true });
const target = join(project, 'written-by-turn.txt');
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  permissions: [], inputs: [], log: [], verdict: 'failed' };

const logger = new RedactingLogger({ appendLine: (line) => report.log.push(line) }, () => true);
const shown = [];
const client = new JedenAcpClient({
  transport: () => new StdioAcpTransport({ command: binary, args: ['acp'], cwd: project }, logger),
  logger,
  autoReconnect: () => false,
  reconnectLimit: () => report.permissions.length,
  interaction: {
    async requestPermission(request) {
      report.permissions.push(request);
      shown.push(request);
      const allow = request.options.find((option) => option.kind.startsWith('allow'));
      assert.ok(allow, `the agent offered no allow option: ${JSON.stringify(request.options)}`);
      return allow.optionId;
    },
    async requestInput(request) {
      report.inputs.push(request);
      return undefined;
    },
  },
});

try {
  await client.connect();
  const session = await client.newSession(project);
  await client.prompt(`Write the single line "written" to the file ${target} with your file writing tool, then stop.`);
  assert.ok(shown.length, 'the agent asked no permission for a write under always-ask');
  for (const request of shown) {
    assert.equal(request.sessionId, session.id, 'the permission request names another session');
    for (const option of request.options) {
      assert.ok(option.optionId && option.name && option.kind, `an option lost a field: ${JSON.stringify(option)}`);
    }
  }
  assert.ok(!report.log.some((line) => line.includes('acp.client_request.error')),
    `the client refused one of the agent's own requests: ${report.log.join('\n')}`);
  assert.ok(existsSync(target), `the allowed write did not land at ${target}`);
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
  await client.dispose();
  client.removeAllListeners();
}
