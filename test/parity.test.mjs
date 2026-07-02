import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile, mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'

import { parseAction } from '../src/protocol.js'
import { createToolRegistry } from '../src/tools.js'
import { loadProjectContext } from '../src/context.js'
import { SessionRecorder, listSessionArtifacts, readSessionArtifact, readSession, listSessions } from '../src/session.js'
import { loadCustomTools } from '../src/custom-tools.js'
import { toolHookEvent, postToolHookEvent } from '../src/hooks.js'
import { runJeden } from '../src/index.js'
import { systemPrompt } from '../src/policy.js'

async function withTempDir(fn) {
  const dir = await mkdtemp(join(tmpdir(), 'jeden-test-'))
  try {
    return await fn(dir)
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
}

async function withIsolatedHome(dir, fn) {
  const previousHome = process.env.HOME
  const previousUserProfile = process.env.USERPROFILE
  const home = join(dir, 'home')
  await mkdir(home, { recursive: true })
  process.env.HOME = home
  process.env.USERPROFILE = home
  try {
    return await fn(home)
  } finally {
    if (previousHome === undefined) delete process.env.HOME
    else process.env.HOME = previousHome
    if (previousUserProfile === undefined) delete process.env.USERPROFILE
    else process.env.USERPROFILE = previousUserProfile
  }
}

async function withHttpServer(handler, fn) {
  const server = createServer(handler)
  await new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => {
      server.off('error', reject)
      resolve()
    })
  })
  try {
    const address = server.address()
    assert.ok(address && typeof address === 'object', 'expected the HTTP server to have a TCP address')
    return await fn(`http://127.0.0.1:${address.port}`)
  } finally {
    await new Promise((resolve, reject) => {
      server.close((error) => {
        if (error) reject(error)
        else resolve()
      })
    })
  }
}

test('parseAction preserves ordered multi-tool requests and default inputs', () => {
  const action = parseAction(`model preface
{"action":"tools","tools":[{"tool":"read_file","input":{"path":"src/protocol.js"}},{"tool":"list_dir"}]}
model suffix`)

  assert.deepEqual(action, {
    action: 'tools',
    tools: [
      { action: 'tool', tool: 'read_file', input: { path: 'src/protocol.js' } },
      { action: 'tool', tool: 'list_dir', input: {} },
    ],
  })
})

test('parseAction rejects a multi-tool action with no executable requests', () => {
  assert.throws(
    () => parseAction('{"action":"tools","tools":[]}'),
    /tools action requires tools/,
  )
})

test('read_file selectors return a line window while edit_file rejects stale sha without mutating', async () => {
  await withTempDir(async (dir) => {
    const file = join(dir, 'notes.txt')
    await writeFile(file, 'alpha\nbravo\ncharlie\ndelta\n', 'utf8')
    const registry = createToolRegistry({ cwd: dir, allowWrite: true })

    const selected = await registry.execute('read_file', { path: 'notes.txt:2+2' })
    assert.equal(selected.ok, true)
    assert.equal(selected.output.path, 'notes.txt')
    assert.equal(selected.output.range, '2+2')
    assert.equal(selected.output.startLine, 2)
    assert.equal(selected.output.endLine, 3)
    assert.equal(selected.output.content, 'bravo\ncharlie')

    await writeFile(file, 'alpha\nbravo\nCHANGED\ndelta\n', 'utf8')
    const staleEdit = await registry.execute('edit_file', {
      path: 'notes.txt',
      expectedSha256: selected.output.sha256,
      ops: [{ op: 'replace', startLine: 3, endLine: 3, content: 'edited' }],
    })

    assert.equal(staleEdit.ok, false)
    assert.match(staleEdit.error, /sha256 mismatch for notes\.txt/)
    assert.equal(await readFile(file, 'utf8'), 'alpha\nbravo\nCHANGED\ndelta\n')
  })
})

test('loadProjectContext expands @ imports relative to the context file', async () => {
  await withTempDir(async (dir) => {
    await mkdir(join(dir, 'partials'), { recursive: true })
    await writeFile(join(dir, 'AGENTS.md'), 'Local rules\n@partials/detail.md\nDone\n', 'utf8')
    await writeFile(join(dir, 'partials', 'detail.md'), 'Use deterministic tests.\nKeep assertions semantic.\n', 'utf8')

    const contextFiles = await loadProjectContext({ cwd: dir })
    const projectContext = contextFiles.find((file) => file.path === 'AGENTS.md')

    assert.ok(projectContext, 'expected AGENTS.md to be loaded from the requested cwd')
    assert.match(projectContext.content, /^Local rules\n@partials\/detail\.md\nDone/m)
    assert.match(projectContext.content, /# Imported partials\/detail\.md\nUse deterministic tests\.\nKeep assertions semantic\./)
  })
})

test('session artifact readers list sanitized artifact names and read their contents', async () => {
  await withTempDir(async (root) => {
    const recorder = new SessionRecorder({ root, cwd: root, id: 'session-a' })
    const writtenPath = await recorder.writeArtifact('analysis/report.txt', 'ranked output')
    await recorder.writeArtifact('z-last.txt', 'later output')

    const sessions = await listSessions({ root })
    assert.equal(sessions.length, 1)
    assert.equal(sessions[0].id, 'session-a')
    assert.equal(sessions[0].cwd, root)

    const transcript = await readSession({ idOrPath: 'session-a', root })
    assert.deepEqual(transcript.events.map((event) => event.type), ['artifact', 'artifact'])
    assert.equal(transcript.events[0].data.name, 'analysis_report.txt')
    assert.equal(transcript.events[0].data.path, writtenPath)

    const listed = await listSessionArtifacts({ idOrPath: 'session-a', root })
    assert.equal(listed.id, 'session-a')
    assert.deepEqual(listed.artifacts.map((artifact) => artifact.name), ['analysis_report.txt', 'z-last.txt'])
    assert.equal(listed.artifacts[0].bytes, Buffer.byteLength('ranked output'))

    const artifact = await readSessionArtifact({ idOrPath: 'session-a', name: 'analysis_report.txt', root })
    assert.equal(artifact.id, 'session-a')
    assert.equal(artifact.name, 'analysis_report.txt')
    assert.equal(artifact.content, 'ranked output')
  })
})

test('system prompt enforces dedicated tool policy', () => {
  const prompt = systemPrompt(createToolRegistry().list())
  assert.match(prompt, /Use glob_paths\/list_dir for file discovery/)
  assert.match(prompt, /Use grep_regex\/search_files for content search/)
  assert.match(prompt, /do not use run_command\/run_process for grep, find, ls, or globbing/)
  assert.match(prompt, /Use read_file ranges\/selectors for targeted reads/)
})

test('hook helpers classify tools from capability metadata', () => {
  assert.equal(toolHookEvent('list_dir'), 'pre_tool_use:read')
  assert.equal(postToolHookEvent('list_dir'), null)

  assert.equal(toolHookEvent('write_file'), 'pre_tool_use:edit')
  assert.equal(postToolHookEvent('write_file'), null)

  assert.equal(toolHookEvent('save_artifact'), 'pre_tool_use:edit')
  assert.equal(postToolHookEvent('save_artifact'), null)

  assert.equal(toolHookEvent('run_command'), 'pre_tool_use:bash')
  assert.equal(postToolHookEvent('run_command'), 'post_tool_use:bash')
})

test('custom tool api.exec is denied unless command execution is allowed', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedHome(dir, async () => {
      await mkdir(join(dir, '.jeden', 'tools'), { recursive: true })
      await writeFile(join(dir, '.jeden', 'tools', 'exec-probe.mjs'), `
export default (api) => ({
  name: 'custom_exec_probe',
  description: 'Runs a deterministic child process through custom-tool api.exec',
  input: {},
  async execute() {
    return api.exec(process.execPath, ['-e', 'process.stdout.write("custom-ok")'], { timeoutMs: 1000 })
  },
})
`, 'utf8')

      const builtInToolNames = createToolRegistry({ cwd: dir }).list().map((tool) => tool.name)

      const denied = await loadCustomTools({ cwd: dir, builtInToolNames, allowCommand: false })
      assert.deepEqual(denied.errors, [])
      const deniedRegistry = createToolRegistry({ cwd: dir, customTools: denied.tools })
      const deniedResult = await deniedRegistry.execute('custom_exec_probe', {})
      assert.equal(deniedResult.ok, false)
      assert.match(deniedResult.error, /custom tool exec requires --allow-command/)

      const allowed = await loadCustomTools({ cwd: dir, builtInToolNames, allowCommand: true })
      assert.deepEqual(allowed.errors, [])
      const allowedRegistry = createToolRegistry({ cwd: dir, customTools: allowed.tools })
      const allowedResult = await allowedRegistry.execute('custom_exec_probe', {})
      assert.equal(allowedResult.ok, true)
      assert.equal(allowedResult.output.code, 0)
      assert.equal(allowedResult.output.timedOut, false)
      assert.equal(allowedResult.output.stdout, 'custom-ok')
      assert.equal(allowedResult.output.stderr, '')
    })
  })
})

test('fetch_readable_url strips scripts, styles, tags, and basic HTML entities', async () => {
  await withTempDir(async (dir) => {
    await withHttpServer((request, response) => {
      response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
      response.end(`<!doctype html>
<html>
  <head>
    <style>body { color: red; }</style>
    <script>globalThis.leaked = "script text";</script>
  </head>
  <body>
    <h1>Tom &amp; Jerry</h1>
    <p>5 &lt; 7 &gt; 3 &quot;yes&quot; &#39;ok&#39;&nbsp;done</p>
    <div>Visible <strong>text</strong></div>
  </body>
</html>`)
    }, async (origin) => {
      const registry = createToolRegistry({ cwd: dir })
      const result = await registry.execute('fetch_readable_url', { url: `${origin}/page` })

      assert.equal(result.ok, true)
      assert.equal(result.output.status, 200)
      assert.equal(result.output.ok, true)
      assert.equal(result.output.contentType, 'text/html; charset=utf-8')
      assert.equal(result.output.truncated, false)
      assert.equal(result.output.text, 'Tom & Jerry 5 < 7 > 3 "yes" \'ok\' done Visible text')
    })
  })
})

test('runJeden round-trips ask_user through the provided callback', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedHome(dir, async () => {
      const asked = []
      let calls = 0
      const chat = async ({ messages, tools }) => {
        calls += 1
        if (calls === 1) {
          assert.ok(tools.some((tool) => tool.function.name === 'ask_user'), 'expected ask_user to be advertised to the model')
          return JSON.stringify({
            action: 'tool',
            tool: 'ask_user',
            input: { question: 'Pick a color', options: ['red', 42] },
          })
        }

        const toolMessage = JSON.parse(messages.at(-1).content)
        assert.deepEqual(toolMessage, {
          type: 'tool_result',
          result: { ok: true, output: { answer: 'blue' } },
        })
        return JSON.stringify({ action: 'final', text: `model saw ${toolMessage.result.output.answer}` })
      }

      const result = await runJeden({
        task: 'Ask the user for a color.',
        cwd: dir,
        chat,
        askUser: async (request) => {
          asked.push(request)
          return 'blue'
        },
        maxSteps: 2,
      })

      assert.equal(result.text, 'model saw blue')
      assert.equal(result.steps, 2)
      assert.equal(calls, 2)
      assert.deepEqual(asked, [{ question: 'Pick a color', options: ['red', '42'] }])
    })
  })
})
