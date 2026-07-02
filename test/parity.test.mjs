import test from 'node:test'
import assert from 'node:assert/strict'
import { chmod, mkdtemp, readFile, rm, writeFile, mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { execFile } from 'node:child_process'

import { parseAction } from '../src/protocol.js'
import { createToolRegistry } from '../src/tools.js'
import { loadProjectContext } from '../src/context.js'
import { SessionRecorder, listSessionArtifacts, readSessionArtifact, readSession, listSessions, sessionReplayMessages } from '../src/session.js'
import { loadCustomTools } from '../src/custom-tools.js'
import { toolHookEvent, postToolHookEvent } from '../src/hooks.js'
import { runJeden } from '../src/index.js'
import { systemPrompt } from '../src/policy.js'
import { loadMcpToolAdapters } from '../src/mcp.js'

async function withTempDir(fn) {
  const dir = await mkdtemp(join(tmpdir(), 'jeden-test-'))
  try {
    return await fn(dir)
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
}

async function execFileOk(command, args, options = {}) {
  return await new Promise((resolve, reject) => {
    execFile(command, args, options, (error, stdout, stderr) => {
      if (error) {
        error.stdout = stdout
        error.stderr = stderr
        reject(error)
        return
      }
      resolve({ stdout, stderr })
    })
  })
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

function makeTar(name, text) {
  const content = Buffer.from(text, 'utf8')
  const header = Buffer.alloc(512)
  header.write(name, 0, 'utf8')
  header.write(content.length.toString(8).padStart(11, '0'), 124, 'utf8')
  header.write('0', 156, 'utf8')
  const padding = Buffer.alloc(Math.ceil(content.length / 512) * 512 - content.length)
  return Buffer.concat([header, content, padding, Buffer.alloc(1024)])
}

function makeZip(name, data) {
  const nameBytes = Buffer.from(name, 'utf8')
  const content = Buffer.isBuffer(data) ? data : Buffer.from(data, 'utf8')
  const local = Buffer.alloc(30 + nameBytes.length)
  local.writeUInt32LE(0x04034b50, 0)
  local.writeUInt16LE(20, 4)
  local.writeUInt32LE(content.length, 18)
  local.writeUInt32LE(content.length, 22)
  local.writeUInt16LE(nameBytes.length, 26)
  nameBytes.copy(local, 30)
  const central = Buffer.alloc(46 + nameBytes.length)
  central.writeUInt32LE(0x02014b50, 0)
  central.writeUInt16LE(20, 6)
  central.writeUInt32LE(content.length, 20)
  central.writeUInt32LE(content.length, 24)
  central.writeUInt16LE(nameBytes.length, 28)
  nameBytes.copy(central, 46)
  const eocd = Buffer.alloc(22)
  eocd.writeUInt32LE(0x06054b50, 0)
  eocd.writeUInt16LE(1, 8)
  eocd.writeUInt16LE(1, 10)
  eocd.writeUInt32LE(central.length, 12)
  eocd.writeUInt32LE(local.length + content.length, 16)
  return Buffer.concat([local, content, central, eocd])
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

    const multi = await registry.execute('read_file', { path: 'notes.txt:1-1,4' })
    assert.equal(multi.output.content, 'alpha\ndelta')
    assert.deepEqual(multi.output.ranges.map((range) => [range.startLine, range.endLine]), [[1, 1], [4, 4]])

    const raw = await registry.execute('read_file', { path: 'notes.txt:raw:3+1' })
    assert.equal(raw.output.raw, true)
    assert.equal(raw.output.content, 'charlie')

    await writeFile(join(dir, 'conflict.txt'), 'before\n<<<<<<< ours\nleft\n=======\nright\n>>>>>>> theirs\nafter\n', 'utf8')
    const conflicts = await registry.execute('read_file', { path: 'conflict.txt:conflicts' })
    assert.deepEqual(conflicts.output.conflicts.map((block) => [block.startLine, block.separatorLine, block.endLine]), [[2, 4, 6]])

    await writeFile(file, 'alpha\nbravo\nCHANGED\ndelta\n', 'utf8')
    const staleEdit = await registry.execute('edit_file', {
      path: 'notes.txt',
      expectedSha256: selected.output.sha256,
      ops: [{ op: 'replace', startLine: 3, endLine: 3, content: 'edited' }],
    })

    assert.equal(staleEdit.ok, false)
    assert.match(staleEdit.error, /sha256 mismatch for notes\.txt/)
    assert.equal(await readFile(file, 'utf8'), 'alpha\nbravo\nCHANGED\ndelta\n')

    const current = await registry.execute('read_file', { path: 'notes.txt' })
    const applied = await registry.execute('edit_file', {
      path: 'notes.txt',
      expectedSha256: current.output.sha256,
      ops: [
        { op: 'replace', startLine: 2, endLine: 3, content: 'BRAVO\nCHARLIE' },
        { op: 'insert_after', line: 4, content: 'echo' },
      ],
    })
    assert.equal(applied.ok, true)
    assert.equal(await readFile(file, 'utf8'), 'alpha\nBRAVO\nCHARLIE\ndelta\necho\n')

    const bigLines = ['head', ...new Array(560).fill('x'.repeat(1024)), 'tail']
    await writeFile(join(dir, 'big.txt'), `${bigLines.join('\n')}\n`, 'utf8')
    const bigWhole = await registry.execute('read_file', { path: 'big.txt' })
    assert.equal(bigWhole.ok, false)
    assert.match(bigWhole.error, /use a line selector/)
    const bigSlice = await registry.execute('read_file', { path: 'big.txt:562' })
    assert.equal(bigSlice.ok, true)
    assert.equal(bigSlice.output.content, 'tail')
  })
})

test('read_image returns dimensions and capped base64', async () => {
  await withTempDir(async (dir) => {
    const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAQAAADoP0S7AAAADElEQVR42mP8z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC', 'base64')
    await writeFile(join(dir, 'tiny.png'), png)
    const registry = createToolRegistry({ cwd: dir })
    const image = await registry.execute('read_image', { path: 'tiny.png', maxBytes: 12 })
    assert.equal(image.ok, true)
    assert.equal(image.output.mimeType, 'image/png')
    assert.equal(image.output.width, 2)
    assert.equal(image.output.height, 3)
    assert.equal(image.output.bytes, png.length)
    assert.equal(image.output.truncated, true)
    assert.equal(image.output.base64, png.subarray(0, 12).toString('base64'))
  })
})

test('read_document extracts JSON notebooks and basic PDF text', async () => {
  await withTempDir(async (dir) => {
    await writeFile(join(dir, 'data.json'), '{"z":1,"a":{"ok":true}}', 'utf8')
    await writeFile(join(dir, 'data.csv'), 'name,count\nalpha,1\n"beta, quoted",2\n', 'utf8')
    await writeFile(join(dir, 'feed.xml'), '<rss><channel><title>Local News</title><item><title>One</title><link>https://example.com/one</link></item><item><title>Two</title></item></channel></rss>', 'utf8')
    await writeFile(join(dir, 'analysis.ipynb'), JSON.stringify({ cells: [{ cell_type: 'markdown', source: ['# Title\n', 'Body'] }, { cell_type: 'code', source: ['print("ok")\n'] }] }), 'utf8')
    await writeFile(join(dir, 'paper.pdf'), `%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R >> endobj
4 0 obj << /Length 44 >> stream
BT /F1 12 Tf 72 720 Td (Hello PDF text) Tj ET
endstream endobj
trailer << /Root 1 0 R >>
%%EOF`, 'utf8')
    const registry = createToolRegistry({ cwd: dir })

    const json = await registry.execute('read_document', { path: 'data.json' })
    assert.equal(json.output.text, '{\n  "z": 1,\n  "a": {\n    "ok": true\n  }\n}')
    const csv = await registry.execute('read_document', { path: 'data.csv' })
    assert.equal(csv.output.text, '| name | count |\n| --- | --- |\n| alpha | 1 |\n| beta, quoted | 2 |')
    const xml = await registry.execute('read_document', { path: 'feed.xml' })
    assert.equal(xml.output.text, '# Local News\n- One — https://example.com/one\n- Two')
    const notebook = await registry.execute('read_document', { path: 'analysis.ipynb' })
    assert.match(notebook.output.text, /# %% \[markdown\] cell:1\n# Title\nBody/)
    assert.match(notebook.output.text, /# %% \[code\] cell:2\nprint\("ok"\)/)
    const pdf = await registry.execute('read_document', { path: 'paper.pdf' })
    assert.equal(pdf.output.text, 'Hello PDF text')
  })
})

test('read_sqlite lists tables and reads rows', async () => {
  await withTempDir(async (dir) => {
    await execFileOk('sqlite3', [
      join(dir, 'app.db'),
      'CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, active INTEGER NOT NULL); INSERT INTO users (name, active) VALUES (\'Ada\', 1), (\'Grace\', 0); CREATE TABLE "user data" ("account id" TEXT PRIMARY KEY, "display name" TEXT NOT NULL); INSERT INTO "user data" VALUES (\'acct-1\', \'Quoted Name\'); CREATE TABLE composite (tenant TEXT, slug TEXT, value TEXT, PRIMARY KEY (tenant, slug)); INSERT INTO composite VALUES (\'t1\', \'s1\', \'v1\');',
    ])
    const registry = createToolRegistry({ cwd: dir })

    const tables = await registry.execute('read_sqlite', { path: 'app.db' })
    assert.equal(tables.ok, true)
    assert.deepEqual(tables.output.tables, [{ name: 'composite', type: 'table', rows: 1 }, { name: 'user data', type: 'table', rows: 1 }, { name: 'users', type: 'table', rows: 2 }])

    const sample = await registry.execute('read_sqlite', { path: 'app.db', table: 'users', where: 'active = 1', order: 'id DESC' })
    assert.equal(sample.ok, true)
    assert.deepEqual(sample.output.schema.map((column) => column.name), ['id', 'name', 'active'])
    assert.deepEqual(sample.output.rows, [{ id: 1, name: 'Ada', active: 1 }])

    const row = await registry.execute('read_sqlite', { path: 'app.db', table: 'users', key: '2' })
    assert.equal(row.ok, true)
    assert.deepEqual(row.output.row, { id: 2, name: 'Grace', active: 0 })

    const quotedRow = await registry.execute('read_sqlite', { path: 'app.db', table: 'user data', key: 'acct-1' })
    assert.equal(quotedRow.ok, true)
    assert.deepEqual(quotedRow.output.row, { 'account id': 'acct-1', 'display name': 'Quoted Name' })

    const compositeKey = await registry.execute('read_sqlite', { path: 'app.db', table: 'composite', key: 't1' })
    assert.equal(compositeKey.ok, false)
    assert.match(compositeKey.error, /single-column primary key/)

    const query = await registry.execute('read_sqlite', { path: 'app.db', query: 'SELECT count(*) AS total FROM users' })
    assert.equal(query.ok, true)
    assert.deepEqual(query.output.rows, [{ total: 2 }])

    const denied = await registry.execute('read_sqlite', { path: 'app.db', query: 'DELETE FROM users' })
    assert.equal(denied.ok, false)
    assert.match(denied.error, /SELECT or WITH/)
  })
})

test('read_archive lists and reads tar and zip entries', async () => {
  await withTempDir(async (dir) => {
    await writeFile(join(dir, 'bundle.tar'), makeTar('docs/readme.txt', 'tar text'), 'utf8')
    await writeFile(join(dir, 'bundle.zip'), makeZip('src/index.js', 'zip text'))
    await writeFile(join(dir, 'paper.zip'), makeZip('paper.pdf', `%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R >> endobj
4 0 obj << /Length 44 >> stream
BT /F1 12 Tf 72 720 Td (Archive PDF text) Tj ET
endstream endobj
trailer << /Root 1 0 R >>
%%EOF`))
    const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAQAAADoP0S7AAAADElEQVR42mP8z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC', 'base64')
    await writeFile(join(dir, 'image.zip'), makeZip('tiny.png', png))
    const registry = createToolRegistry({ cwd: dir })

    const tarList = await registry.execute('read_archive', { path: 'bundle.tar' })
    assert.deepEqual(tarList.output.entries, [{ name: 'docs/readme.txt', type: 'file', bytes: 8 }])
    const tarEntry = await registry.execute('read_archive', { path: 'bundle.tar', entry: 'docs/readme.txt' })
    assert.equal(tarEntry.output.content, 'tar text')

    const zipList = await registry.execute('read_archive', { path: 'bundle.zip' })
    assert.deepEqual(zipList.output.entries, [{ name: 'src/index.js', type: 'file', bytes: 8 }])
    const zipEntry = await registry.execute('read_archive', { path: 'bundle.zip', entry: 'src/index.js' })
    assert.equal(zipEntry.output.content, 'zip text')
    const zipBinary = await registry.execute('read_archive', { path: 'bundle.zip', entry: 'src/index.js', mode: 'binary', maxBytes: 4 })
    assert.equal(zipBinary.output.base64, Buffer.from('zip ').toString('base64'))
    assert.equal(zipBinary.output.truncated, true)
    const zipDocument = await registry.execute('read_archive', { path: 'paper.zip', entry: 'paper.pdf', mode: 'document' })
    assert.equal(zipDocument.output.text, 'Archive PDF text')
    const zipImage = await registry.execute('read_archive', { path: 'image.zip', entry: 'tiny.png', mode: 'image', maxBytes: 12 })
    assert.equal(zipImage.output.mimeType, 'image/png')
    assert.equal(zipImage.output.width, 2)
    assert.equal(zipImage.output.height, 3)
    assert.equal(zipImage.output.base64, png.subarray(0, 12).toString('base64'))
  })
})

test('search_files and grep_regex accept multiple paths', async () => {
  await withTempDir(async (dir) => {
    await mkdir(join(dir, 'left'), { recursive: true })
    await mkdir(join(dir, 'right'), { recursive: true })
    await mkdir(join(dir, 'skip'), { recursive: true })
    await writeFile(join(dir, 'left', 'a.txt'), 'alpha needle\n', 'utf8')
    await writeFile(join(dir, 'right', 'b.txt'), 'beta needle\n', 'utf8')
    await writeFile(join(dir, 'skip', 'c.txt'), 'skip needle\n', 'utf8')
    await writeFile(join(dir, 'right', 'multi.txt'), 'one\nalpha\nbeta\nthree\n', 'utf8')
    await mkdir(join(dir, 'empty-dir'), { recursive: true })
    await writeFile(join(dir, 'right', 'image.bin'), Buffer.from([0, 1, 2, 3]))
    await writeFile(join(dir, '.secret.txt'), 'hidden needle\n', 'utf8')
    const registry = createToolRegistry({ cwd: dir })

    const literal = await registry.execute('search_files', { paths: ['left', 'right'], query: 'needle', limit: 10 })
    assert.deepEqual(literal.output.matches.map((match) => match.path), ['left/a.txt', 'right/b.txt'])
    const insensitiveFile = await registry.execute('search_text', { path: 'left/a.txt', query: 'NEEDLE' })
    assert.deepEqual(insensitiveFile.output.matches.map((match) => match.line), [1])
    const sensitiveFile = await registry.execute('search_text', { path: 'left/a.txt', query: 'NEEDLE', caseSensitive: true })
    assert.deepEqual(sensitiveFile.output.matches, [])
    const insensitiveTree = await registry.execute('search_files', { paths: ['left', 'right'], query: 'NEEDLE', limit: 10 })
    assert.deepEqual(insensitiveTree.output.matches.map((match) => match.path), ['left/a.txt', 'right/b.txt'])

    const regex = await registry.execute('grep_regex', { paths: ['right'], expr: 'beta\\s+needle', limit: 10 })
    assert.deepEqual(regex.output.matches.map((match) => match.path), ['right/b.txt'])
    const insensitiveRegex = await registry.execute('grep_regex', { paths: ['right'], expr: 'BETA\\s+NEEDLE', limit: 10 })
    assert.deepEqual(insensitiveRegex.output.matches.map((match) => match.path), ['right/b.txt'])
    const sensitiveRegex = await registry.execute('grep_regex', { paths: ['right'], expr: 'BETA\\s+NEEDLE', caseSensitive: true, limit: 10 })
    assert.deepEqual(sensitiveRegex.output.matches, [])
    const multiline = await registry.execute('grep_regex', { paths: ['right'], expr: 'alpha\\nbeta', multiline: true, limit: 10 })
    assert.deepEqual(multiline.output.matches.map((match) => [match.path, match.line, match.text]), [['right/multi.txt', 2, 'alpha beta']])

    const hiddenDefault = await registry.execute('search_files', { query: 'hidden', limit: 10 })
    assert.deepEqual(hiddenDefault.output.matches, [])
    const hiddenSearch = await registry.execute('search_files', { query: 'hidden', hidden: true, limit: 10 })
    assert.deepEqual(hiddenSearch.output.matches.map((match) => match.path), ['.secret.txt'])

    const globbed = await registry.execute('glob_paths', { patterns: ['**/*.bin', 'empty-dir'], gitignore: false, limit: 10 })
    assert.deepEqual(globbed.output.matches, [
      { path: 'empty-dir', type: 'dir' },
      { path: 'right/image.bin', type: 'file' },
    ])
    const fileRoot = await registry.execute('glob_paths', { path: 'left/a.txt', patterns: '**', gitignore: false })
    assert.deepEqual(fileRoot.output.matches, [{ path: 'left/a.txt', type: 'file' }])
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

test('run_process supports child env overrides', async () => {
  await withTempDir(async (dir) => {
    const registry = createToolRegistry({ cwd: dir, allowCommand: true })
    const result = await registry.execute('run_process', {
      command: process.execPath,
      args: ['-e', 'process.stdout.write(`${process.env.JEDEN_ENV_TEST || "missing"}:${process.env.JEDEN_ENV_REMOVE || "removed"}`)'],
      env: { JEDEN_ENV_TEST: 'ok', JEDEN_ENV_REMOVE: null },
    })
    assert.equal(result.ok, true)
    assert.equal(result.output.code, 0)
    assert.equal(result.output.stdout, 'ok:removed')
  })
})

test('custom tools preserve capability metadata and jail readText', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedHome(dir, async () => {
      await mkdir(join(dir, '.jeden', 'tools'), { recursive: true })
      await writeFile(join(dir, 'inside.txt'), 'inside', 'utf8')
      await writeFile(join(dir, '..', 'outside.txt'), 'outside', 'utf8')
      await writeFile(join(dir, '.jeden', 'tools', 'metadata-probe.mjs'), `
export default (api) => ({
  name: 'custom_metadata_probe',
  description: 'Checks custom capability metadata and readText jail',
  permission: 'write',
  input: { target: 'string optional' },
  async execute(input) {
    return api.readText(input.target || 'inside.txt')
  },
})
`, 'utf8')

      const builtInToolNames = createToolRegistry({ cwd: dir }).list().map((tool) => tool.name)
      const loaded = await loadCustomTools({ cwd: dir, builtInToolNames })
      assert.deepEqual(loaded.errors, [])
      const registry = createToolRegistry({ cwd: dir, customTools: loaded.tools })
      assert.equal(toolHookEvent('custom_metadata_probe'), 'pre_tool_use:edit')

      const inside = await registry.execute('custom_metadata_probe', {})
      assert.equal(inside.output, 'inside')
      const outside = await registry.execute('custom_metadata_probe', { target: '../outside.txt' })
      assert.equal(outside.ok, false)
      assert.match(outside.error, /path escapes cwd/)
    })
  })
})


test('fetch_readable_url strips scripts, styles, tags, and basic HTML entities', async () => {
  await withTempDir(async (dir) => {
    await withHttpServer((request, response) => {
      if (request.url === '/data.json') {
        response.writeHead(200, { 'content-type': 'application/json' })
        response.end('{"name":"Jeden","nested":{"ok":true}}')
        return
      }
      if (request.url === '/feed.xml') {
        response.writeHead(200, { 'content-type': 'application/rss+xml' })
        response.end('<rss><channel><title>News</title><item><title>First</title><link>https://example.com/first</link></item><item><title>Second</title></item></channel></rss>')
        return
      }
      if (request.url === '/table.tsv') {
        response.writeHead(200, { 'content-type': 'text/tab-separated-values' })
        response.end('name\tcount\nalpha\t1\nbeta\t2\n')
        return
      }
      if (request.url === '/paper.pdf') {
        response.writeHead(200, { 'content-type': 'application/pdf' })
        response.end(`%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R >> endobj
4 0 obj << /Length 42 >> stream
BT /F1 12 Tf 72 720 Td (Remote PDF text) Tj ET
endstream endobj
trailer << /Root 1 0 R >>
%%EOF`)
        return
      }
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
      assert.ok(result.output.sourceBytes > result.output.bytes)
      assert.match(result.output.sha256, /^[a-f0-9]{64}$/)

      const raw = await registry.execute('fetch_url', { url: `${origin}/page`, maxBytes: 1000, timeoutMs: 1000 })
      assert.equal(raw.output.status, 200)
      assert.equal(raw.output.truncated, false)
      assert.ok(raw.output.bytes > result.output.bytes)
      assert.match(raw.output.sha256, /^[a-f0-9]{64}$/)

      const json = await registry.execute('fetch_readable_url', { url: `${origin}/data.json` })
      assert.equal(json.output.text, '{\n  "name": "Jeden",\n  "nested": {\n    "ok": true\n  }\n}')

      const feed = await registry.execute('fetch_readable_url', { url: `${origin}/feed.xml` })
      assert.equal(feed.output.text, '# News\n- First — https://example.com/first\n- Second')

      const table = await registry.execute('fetch_readable_url', { url: `${origin}/table.tsv` })
      assert.equal(table.output.text, '| name | count |\n| --- | --- |\n| alpha | 1 |\n| beta | 2 |')

      const pdf = await registry.execute('fetch_readable_url', { url: `${origin}/paper.pdf` })
      assert.equal(pdf.output.text, 'Remote PDF text')
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

test('pre-tool hook can replace tool input before execution', async () => {
  await withTempDir(async (dir) => {
    await writeFile(join(dir, 'a.txt'), 'wrong', 'utf8')
    await writeFile(join(dir, 'b.txt'), 'right', 'utf8')
    let calls = 0
    const chat = async ({ messages }) => {
      calls += 1
      if (calls === 1) return JSON.stringify({ action: 'tool', tool: 'read_file', input: { path: 'a.txt' } })
      const toolMessage = JSON.parse(messages.at(-1).content)
      assert.equal(toolMessage.result.output.path, 'b.txt')
      assert.equal(toolMessage.result.output.content, 'right')
      return JSON.stringify({ action: 'final', text: 'hook changed input' })
    }
    const hookRunner = {
      async run(event) {
        if (event === 'pre_tool_use:read') return { decision: 'pass', toolInput: { path: 'b.txt' } }
        return { decision: 'pass' }
      },
    }

    const result = await runJeden({ task: 'Read a file', cwd: dir, chat, hookRunner, maxSteps: 2 })
    assert.equal(result.text, 'hook changed input')
  })
})

test('delegate_task parses child JSON run output', async () => {
  await withTempDir(async (dir) => {
    const fakeNode = join(dir, 'fake-node.mjs')
    await writeFile(fakeNode, '#!/usr/bin/env node\nprocess.stdout.write(JSON.stringify({ ok: true, text: "delegated ok", sessionPath: "/tmp/session" }))\n', 'utf8')
    await chmod(fakeNode, 0o755)
    const previous = process.env.JEDEN_NODE
    process.env.JEDEN_NODE = fakeNode
    try {
      const registry = createToolRegistry({ cwd: dir, allowCommand: true })
      const result = await registry.execute('delegate_task', { task: 'child task', maxSteps: 1 })
      assert.equal(result.ok, true)
      assert.equal(result.output.code, 0)
      assert.deepEqual(result.output.delegated, { ok: true, text: 'delegated ok', sessionPath: '/tmp/session' })
    } finally {
      if (previous === undefined) delete process.env.JEDEN_NODE
      else process.env.JEDEN_NODE = previous
    }
  })
})

test('runJeden executes read-only multi-tool actions in parallel', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedHome(dir, async () => {
      await mkdir(join(dir, '.jeden', 'tools'), { recursive: true })
      await writeFile(join(dir, '.jeden', 'tools', 'delay.mjs'), `
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
export default () => [
  { name: 'slow_a', description: 'Slow read A', input: {}, async execute() { await wait(220); return 'A' } },
  { name: 'slow_b', description: 'Slow read B', input: {}, async execute() { await wait(220); return 'B' } },
]
`, 'utf8')
      let calls = 0
      const chat = async ({ messages }) => {
        calls += 1
        if (calls === 1) {
          return JSON.stringify({ action: 'tools', tools: [{ tool: 'slow_a', input: {} }, { tool: 'slow_b', input: {} }] })
        }
        const toolMessage = JSON.parse(messages.at(-1).content)
        assert.deepEqual(toolMessage.result.map((entry) => entry.result.output), ['A', 'B'])
        return JSON.stringify({ action: 'final', text: 'parallel ok' })
      }

      const started = Date.now()
      const result = await runJeden({ task: 'Run slow reads', cwd: dir, chat, maxSteps: 2 })
      const elapsed = Date.now() - started
      assert.equal(result.text, 'parallel ok')
      assert.ok(elapsed < 380, `expected parallel execution, got ${elapsed}ms`)
    })
  })
})

test('resume replay preserves structured prior messages', async () => {
  await withTempDir(async (dir) => {
    const previous = {
      events: [
        { type: 'user', data: { task: 'old task' } },
        { type: 'assistant_raw', data: { content: JSON.stringify({ action: 'tool', tool: 'read_file', input: { path: 'a.txt' } }) } },
        { type: 'tool_result', data: { result: { ok: true, output: { content: 'old file' } } } },
        { type: 'assistant_raw', data: { content: JSON.stringify({ action: 'final', text: 'old final' }) } },
        { type: 'final', data: { text: 'old final' } },
      ],
    }
    const replay = sessionReplayMessages(previous)
    assert.deepEqual(replay.map((message) => message.role), ['user', 'assistant', 'user', 'assistant'])

    let captured = null
    const chat = async ({ messages }) => {
      captured = messages.map((message) => ({ ...message }))
      return JSON.stringify({ action: 'final', text: 'resume ok' })
    }
    const result = await runJeden({ task: 'new task', cwd: dir, chat, priorMessages: replay, maxSteps: 1 })
    assert.equal(result.text, 'resume ok')
    assert.deepEqual(captured.slice(1).map((message) => message.role), ['user', 'assistant', 'user', 'assistant', 'user'])
    assert.equal(captured.at(-1).content, 'new task')
    assert.match(captured[3].content, /old file/)
  })
})

test('native MCP tools are listed and callable by runJeden', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedHome(dir, async () => {
      const serverFile = join(dir, 'mcp-server.mjs')
      await writeFile(serverFile, `
let buffer = Buffer.alloc(0)
let toolCalls = 0
function send(message) {
  const body = Buffer.from(JSON.stringify(message), 'utf8')
  process.stdout.write('Content-Length: ' + body.length + '\\r\\n\\r\\n')
  process.stdout.write(body)
}
function handle(message) {
  if (message.method === 'initialize') {
    send({ jsonrpc: '2.0', id: message.id, result: { protocolVersion: '2024-11-05', capabilities: {}, serverInfo: { name: 'fake', version: '1' } } })
    return
  }
  if (message.method === 'tools/list') {
    send({ jsonrpc: '2.0', id: message.id, result: { tools: [{ name: 'echo', description: 'Echo text through MCP', inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] } }] } })
    return
  }
  if (message.method === 'tools/call') {
    toolCalls += 1
    send({ jsonrpc: '2.0', id: message.id, result: { content: [{ type: 'text', text: message.params.arguments.text + ':' + toolCalls }] } })
  }
}
process.stdin.on('data', (chunk) => {
  buffer = Buffer.concat([buffer, chunk])
  for (;;) {
    const headerEnd = buffer.indexOf('\\r\\n\\r\\n')
    if (headerEnd === -1) break
    const header = buffer.subarray(0, headerEnd).toString('utf8')
    const line = header.split('\\r\\n')[0]
    const length = Number(line.slice('Content-Length:'.length).trim())
    const bodyStart = headerEnd + 4
    const bodyEnd = bodyStart + length
    if (buffer.length < bodyEnd) break
    const message = JSON.parse(buffer.subarray(bodyStart, bodyEnd).toString('utf8'))
    buffer = buffer.subarray(bodyEnd)
    handle(message)
  }
})
`, 'utf8')
      await mkdir(join(dir, '.jeden'), { recursive: true })
      await writeFile(join(dir, '.jeden', 'mcp.json'), JSON.stringify({ mcpServers: { local: { command: process.execPath, args: [serverFile] } } }), 'utf8')

      const adapters = await loadMcpToolAdapters({ cwd: dir })
      assert.deepEqual(adapters.errors, [])
      assert.deepEqual(adapters.tools.map((tool) => tool.name), ['mcp__local__echo'])

      let calls = 0
      const chat = async ({ messages, tools }) => {
        calls += 1
        if (calls === 1) {
          const nativeTool = tools.find((tool) => tool.function.name === 'mcp__local__echo')
          assert.ok(nativeTool)
          assert.deepEqual(nativeTool.function.parameters.required, ['text'])
          return JSON.stringify({ action: 'tool', tool: 'mcp__local__echo', input: { text: 'first' } })
        }
        const toolMessage = JSON.parse(messages.at(-1).content)
        if (calls === 2) {
          assert.equal(toolMessage.result.output.content[0].text, 'first:1')
          return JSON.stringify({ action: 'tool', tool: 'mcp__local__echo', input: { text: 'second' } })
        }
        assert.equal(toolMessage.result.output.content[0].text, 'second:2')
        return JSON.stringify({ action: 'final', text: 'native mcp ok' })
      }

      const result = await runJeden({ task: 'Call native MCP', cwd: dir, chat, maxSteps: 3 })
      assert.equal(result.text, 'native mcp ok')
    })
  })
})
