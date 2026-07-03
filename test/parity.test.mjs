import test from 'node:test'
import assert from 'node:assert/strict'
import { chmod, mkdtemp, readFile, rm, writeFile, mkdir, utimes } from 'node:fs/promises'
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
import { buildCapabilityManifest, buildDoctorReport, createLocalMemoryBackend, loadMemoryRecords, modelRouterConfig, runJeden, selfRepairPermissions } from '../src/index.js'
import { systemPrompt } from '../src/policy.js'
import { closeMcpClients, loadMcpToolAdapters } from '../src/mcp.js'
import { claudeProjectPath, formatConversationList, listConversationJsonls, recallConversationFromJsonl, resolveConversationJsonl } from '../src/conversation-recall.js'

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

function conversationJsonl(events) {
  return `${events.map((event) => JSON.stringify(event)).join('\n')}\n`
}

async function makeClaudeProject(home, cwd) {
  const projectDir = claudeProjectPath({ cwd, home })
  await mkdir(projectDir, { recursive: true })
  return projectDir
}


async function withIsolatedMemory(dir, fn, memoryFile = join(dir, 'memory.jsonl')) {
  const previousMemoryFile = process.env.JEDEN_MEMORY_FILE
  process.env.JEDEN_MEMORY_FILE = memoryFile
  try {
    return await withIsolatedHome(dir, () => fn(memoryFile))
  } finally {
    if (previousMemoryFile === undefined) delete process.env.JEDEN_MEMORY_FILE
    else process.env.JEDEN_MEMORY_FILE = previousMemoryFile
  }
}

function makeInMemoryRecorder(dir, id = 'test-session') {
  const events = []
  return {
    id,
    events,
    async ensure() {},
    async record(type, data) {
      events.push({ type, data })
    },
    artifactDir() {
      return join(dir, 'artifacts')
    },
    path() {
      return join(dir, 'sessions', id)
    },
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

function expectedSnapshot(path, sha256) {
  return `${path}#${sha256.slice(0, 4).toUpperCase()}`
}

function expectedSnapshotHeader(path, sha256) {
  return `[${expectedSnapshot(path, sha256)}]`
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

test('recall claudeProjectPath encodes provided cwd verbatim under isolated HOME', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedHome(dir, async (home) => {
      const providedCwd = `${dir}/workspaces/../literal project`

      assert.equal(
        claudeProjectPath({ cwd: providedCwd }),
        join(home, '.claude', 'projects', providedCwd.replaceAll('/', '-')),
      )
    })
  })
})

test('recallConversationFromJsonl emits text-only transcript blocks and filters Claude noise', () => {
  const jsonlPath = '/tmp/claude-project/session-123.jsonl'
  const raw = conversationJsonl([
    { type: 'system', message: { content: 'system event hidden' } },
    {
      type: 'user',
      message: {
        content: [
          { type: 'text', text: '\nHello from user\n<system-reminder>system hidden</system-reminder>\nStop hook feedback: hook hidden\n[python3 $HOME/.shared-hooks/hook.py]\nBLOCKED by hook\nBypass: hook hidden\n<task-notification>task hidden</task-notification>\nVisible user text' },
          { type: 'tool_result', content: 'tool result hidden' },
          { type: 'image', source: { data: 'image hidden' } },
        ],
      },
    },
    {
      type: 'assistant',
      message: {
        content: [
          { type: 'text', text: 'Assistant answer' },
          { type: 'tool_use', id: 'toolu_hidden', name: 'Read', input: { file_path: 'secret.txt' } },
          { type: 'text', text: 'Second assistant line\n<tool-use-id>task tool id hidden</tool-use-id>\n<summary>task summary hidden</summary>\nDone' },
          { type: 'tool_result', content: 'assistant tool result hidden' },
          { type: 'image', source: { data: 'assistant image hidden' } },
        ],
      },
    },
    { type: 'user', message: { content: [{ type: 'tool_result', content: 'tool-only user hidden' }] } },
  ])

  assert.equal(recallConversationFromJsonl(raw, { jsonlPath }), `# Conversation transcript: ${jsonlPath}
# (text-only, no tool_use / tool_result / images / hooks)

[USER]
Hello from user
Visible user text

[ASSISTANT]
Assistant answer
Second assistant line
Done
`)
})

test('recall resolveConversationJsonl supports uuid and direct filename fallback', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedHome(dir, async (home) => {
      const cwd = join(dir, 'fixture-project')
      await mkdir(cwd, { recursive: true })
      const projectDir = await makeClaudeProject(home, cwd)
      const uuid = '11111111-2222-4333-8444-555555555555'
      const uuidPath = join(projectDir, `${uuid}.jsonl`)
      const directName = 'manual-session.jsonl'
      const directPath = join(projectDir, directName)
      await writeFile(uuidPath, conversationJsonl([{ type: 'user', message: { content: 'uuid transcript' } }]), 'utf8')
      await writeFile(directPath, conversationJsonl([{ type: 'user', message: { content: 'direct transcript' } }]), 'utf8')

      assert.equal(await resolveConversationJsonl({ cwd, home, session: uuid }), uuidPath)
      assert.equal(await resolveConversationJsonl({ cwd, home, session: directName }), directPath)
    })
  })
})

test('recall listConversationJsonls returns latest ten and formatConversationList emits long-list rows', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedHome(dir, async (home) => {
      const cwd = join(dir, 'fixture-project')
      await mkdir(cwd, { recursive: true })
      const projectDir = await makeClaudeProject(home, cwd)
      const baseTime = Date.now() - 60_000
      for (let index = 0; index < 12; index += 1) {
        const file = join(projectDir, `session-${String(index).padStart(2, '0')}.jsonl`)
        await writeFile(file, `payload-${index}\n`, 'utf8')
        await chmod(file, 0o644)
        const mtime = new Date(baseTime + index * 1000)
        await utimes(file, mtime, mtime)
      }
      await writeFile(join(projectDir, 'ignored.txt'), 'not a transcript\n', 'utf8')

      const entries = await listConversationJsonls({ cwd, home })
      assert.deepEqual(
        entries.map((entry) => entry.name),
        ['session-11.jsonl', 'session-10.jsonl', 'session-09.jsonl', 'session-08.jsonl', 'session-07.jsonl', 'session-06.jsonl', 'session-05.jsonl', 'session-04.jsonl', 'session-03.jsonl', 'session-02.jsonl'],
      )

      const rows = formatConversationList(entries).trimEnd().split('\n')
      assert.equal(rows.length, 10)
      for (const [index, entry] of entries.entries()) {
        const columns = rows[index].trim().split(/\s+/)
        assert.equal(columns[0], '-rw-r--r--')
        assert.match(columns[1], /^\d+$/)
        assert.match(columns[4], /^\d+$/)
        assert.match(columns[5], /^[A-Z][a-z]{2}$/)
        assert.match(columns[6], /^\d{1,2}$/)
        assert.ok(/^\d{2}:\d{2}$/.test(columns[7]) || /^\d{4}$/.test(columns[7]))
        assert.match(rows[index], new RegExp(`\\s${entry.size}\\s+`))
        assert.ok(rows[index].endsWith(entry.path), `expected row to end with ${entry.path}: ${rows[index]}`)
      }
    })
  })
})

test('recall_conversation CLI reads and lists fixture project without touching real HOME', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedHome(dir, async (home) => {
      const cwd = join(dir, 'fixture-project')
      await mkdir(cwd, { recursive: true })
      const projectDir = await makeClaudeProject(home, cwd)
      const olderPath = join(projectDir, 'older.jsonl')
      const latestPath = join(projectDir, 'latest.jsonl')
      const olderRaw = conversationJsonl([{ type: 'user', message: { content: 'older transcript' } }])
      const latestRaw = conversationJsonl([{ type: 'assistant', message: { content: [{ type: 'text', text: 'latest transcript' }, { type: 'tool_use', name: 'Hidden', input: {} }] } }])
      await writeFile(olderPath, olderRaw, 'utf8')
      await writeFile(latestPath, latestRaw, 'utf8')
      await chmod(olderPath, 0o644)
      await chmod(latestPath, 0o644)
      const now = Date.now()
      await utimes(olderPath, new Date(now - 10_000), new Date(now - 10_000))
      await utimes(latestPath, new Date(now), new Date(now))
      const env = { ...process.env, HOME: home, USERPROFILE: home }
      delete env.RECALL_CWD

      const recalled = await execFileOk(process.execPath, ['src/cli.js', 'recall_conversation', '--cwd', cwd], { cwd: process.cwd(), env })
      assert.equal(recalled.stderr, '')
      assert.equal(recalled.stdout, recallConversationFromJsonl(latestRaw, { jsonlPath: latestPath }))

      const listed = await execFileOk(process.execPath, ['src/cli.js', 'recall_conversation', '--list', '--cwd', cwd], { cwd: process.cwd(), env })
      assert.equal(listed.stderr, '')
      const rows = listed.stdout.trimEnd().split('\n')
      assert.equal(rows.length, 2)
      assert.ok(rows[0].endsWith(latestPath), `expected newest transcript first: ${listed.stdout}`)
      assert.ok(rows[1].endsWith(olderPath), `expected older transcript second: ${listed.stdout}`)
      const columns = rows[0].trim().split(/\s+/)
      assert.equal(columns[0], '-rw-r--r--')
      assert.match(columns[1], /^\d+$/)
      assert.match(columns[4], /^\d+$/)
      assert.match(columns[5], /^[A-Z][a-z]{2}$/)
      assert.match(columns[6], /^\d{1,2}$/)
      assert.ok(/^\d{2}:\d{2}$/.test(columns[7]) || /^\d{4}$/.test(columns[7]))
    })
  })
})

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

test('read_file selectors return OMP-style snapshots while edit_file rejects stale sha without mutating', async () => {
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
    assert.match(selected.output.sha256, /^[a-f0-9]{64}$/)
    assert.equal(selected.output.snapshot, expectedSnapshot('notes.txt', selected.output.sha256))
    assert.equal(selected.output.visual, `${expectedSnapshotHeader('notes.txt', selected.output.sha256)}\n2:bravo\n3:charlie`)

    const multi = await registry.execute('read_file', { path: 'notes.txt:1-1,4' })
    assert.equal(multi.output.content, 'alpha\ndelta')
    assert.deepEqual(multi.output.ranges.map((range) => [range.startLine, range.endLine]), [[1, 1], [4, 4]])
    assert.equal(multi.output.snapshot, expectedSnapshot('notes.txt', multi.output.sha256))
    assert.equal(multi.output.visual, `${expectedSnapshotHeader('notes.txt', multi.output.sha256)}\n1:alpha\n4:delta`)

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
    assert.equal(applied.output.path, 'notes.txt')
    assert.match(applied.output.sha256, /^[a-f0-9]{64}$/)
    assert.equal(applied.output.snapshot, expectedSnapshot('notes.txt', applied.output.sha256))
    assert.equal(applied.output.ops, 2)
    assert.equal(applied.output.bytes, Buffer.byteLength('alpha\nBRAVO\nCHARLIE\ndelta\necho\n', 'utf8'))
    assert.match(applied.output.diff, /^--- notes\.txt\n\+\+\+ notes\.txt\n@@ -1,4 \+1,5 @@/m)
    assert.match(applied.output.diff, /^-bravo$/m)
    assert.match(applied.output.diff, /^\+BRAVO$/m)
    assert.match(applied.output.diff, /^-CHANGED$/m)
    assert.match(applied.output.diff, /^\+CHARLIE$/m)
    assert.match(applied.output.diff, /^\+echo$/m)

    const distantPath = join(dir, 'distant.txt')
    await writeFile(distantPath, `${Array.from({ length: 11 }, (_, index) => `line ${index + 1}`).join('\n')}\n`, 'utf8')
    const distantCurrent = await registry.execute('read_file', { path: 'distant.txt' })
    const distantEdit = await registry.execute('edit_file', {
      path: 'distant.txt',
      expectedSha256: distantCurrent.output.sha256,
      ops: [
        { op: 'replace', startLine: 1, endLine: 1, content: 'LINE 1' },
        { op: 'replace', startLine: 11, endLine: 11, content: 'LINE 11' },
      ],
    })
    assert.equal(distantEdit.ok, true)
    assert.match(distantEdit.output.diff, /^@@ -1,4 \+1,4 @@$/m)
    assert.match(distantEdit.output.diff, /^@@ -8,4 \+8,4 @@$/m)
    assert.doesNotMatch(distantEdit.output.diff, /^[-+]line 6$/m)

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

test('edit_file returns a coarse visual diff preview for large files', async () => {
  await withTempDir(async (dir) => {
    const registry = createToolRegistry({ cwd: dir, allowWrite: true })
    const prefix = Array.from({ length: 260 }, (_, index) => `prefix context ${index + 1}`)
    const suffix = Array.from({ length: 260 }, (_, index) => `suffix context ${index + 1}`)
    const content = [...prefix, 'old head line', 'old tail line', ...suffix].join('\n') + '\n'
    await writeFile(join(dir, 'large-diff.txt'), content, 'utf8')

    const current = await registry.execute('read_file', { path: 'large-diff.txt' })
    const edited = await registry.execute('edit_file', {
      path: 'large-diff.txt',
      expectedSha256: current.output.sha256,
      ops: [
        { op: 'replace', startLine: 261, endLine: 261, content: 'new head line' },
        { op: 'replace', startLine: 262, endLine: 262, content: 'new tail line' },
      ],
    })

    assert.equal(edited.ok, true)
    assert.equal(await readFile(join(dir, 'large-diff.txt'), 'utf8'), [...prefix, 'new head line', 'new tail line', ...suffix].join('\n') + '\n')
    assert.match(edited.output.diff, /^@@ coarse preview: exact diff skipped/m)
    assert.match(edited.output.diff, /^-old head line$/m)
    assert.match(edited.output.diff, /^\+new head line$/m)
    assert.match(edited.output.diff, /^-old tail line$/m)
    assert.match(edited.output.diff, /^\+new tail line$/m)
    assert.match(edited.output.diff, /^ prefix context 260$/m)
    assert.match(edited.output.diff, /^ suffix context 1$/m)
  })
})


test('filesystem mutation tools require hashes and expose visual diffs', async () => {
  await withTempDir(async (dir) => {
    const locked = createToolRegistry({ cwd: dir })
    const denied = await locked.execute('write_file', { path: 'notes.txt', content: 'draft\n' })
    assert.equal(denied.ok, false)
    assert.match(denied.error, /requires --allow-write/)

    const registry = createToolRegistry({ cwd: dir, allowWrite: true })
    const created = await registry.execute('write_file', { path: 'notes.txt', content: 'alpha\nbravo\n' })
    assert.equal(created.ok, true)
    assert.match(created.output.sha256, /^[a-f0-9]{64}$/)
    assert.deepEqual(created.output, {
      path: 'notes.txt',
      sha256: created.output.sha256,
      snapshot: expectedSnapshot('notes.txt', created.output.sha256),
      diff: '--- notes.txt\n+++ notes.txt\n@@ -1,0 +1,2 @@\n+alpha\n+bravo',
      bytes: Buffer.byteLength('alpha\nbravo\n', 'utf8'),
    })

    const missingHash = await registry.execute('write_file', { path: 'notes.txt', content: 'bad\n' })
    assert.equal(missingHash.ok, false)
    assert.match(missingHash.error, /expectedSha256 is required/)
    assert.equal(await readFile(join(dir, 'notes.txt'), 'utf8'), 'alpha\nbravo\n')

    const current = await registry.execute('read_file', { path: 'notes.txt' })
    const patched = await registry.execute('apply_patch', {
      path: 'notes.txt',
      expectedSha256: current.output.sha256,
      replacements: [{ old: 'bravo', new: 'charlie' }],
    })
    assert.equal(patched.ok, true)
    assert.equal(await readFile(join(dir, 'notes.txt'), 'utf8'), 'alpha\ncharlie\n')
    assert.equal(patched.output.path, 'notes.txt')
    assert.match(patched.output.sha256, /^[a-f0-9]{64}$/)
    assert.equal(patched.output.snapshot, expectedSnapshot('notes.txt', patched.output.sha256))
    assert.equal(patched.output.replacements, 1)
    assert.equal(patched.output.bytes, Buffer.byteLength('alpha\ncharlie\n', 'utf8'))
    assert.match(patched.output.diff, /^--- notes\.txt\n\+\+\+ notes\.txt\n@@ -1,2 \+1,2 @@/m)
    assert.match(patched.output.diff, /^-bravo$/m)
    assert.match(patched.output.diff, /^\+charlie$/m)

    const patchedCurrent = await registry.execute('read_file', { path: 'notes.txt' })
    const moved = await registry.execute('move_file', { from: 'notes.txt', to: 'archive/notes.txt', expectedSha256: patchedCurrent.output.sha256 })
    assert.deepEqual(moved.output, {
      from: 'notes.txt',
      to: 'archive/notes.txt',
      moved: true,
      snapshot: expectedSnapshot('archive/notes.txt', patchedCurrent.output.sha256),
      diff: 'rename notes.txt -> archive/notes.txt',
    })
    assert.equal(await readFile(join(dir, 'archive', 'notes.txt'), 'utf8'), 'alpha\ncharlie\n')
    await assert.rejects(() => readFile(join(dir, 'notes.txt'), 'utf8'), /ENOENT/)

    const movedCurrent = await registry.execute('read_file', { path: 'archive/notes.txt' })
    const deleted = await registry.execute('delete_file', { path: 'archive/notes.txt', expectedSha256: movedCurrent.output.sha256 })
    assert.deepEqual(deleted.output, {
      path: 'archive/notes.txt',
      deleted: true,
      diff: '--- archive/notes.txt\n+++ archive/notes.txt\n@@ -1,2 +1,0 @@\n-alpha\n-charlie',
    })
    await assert.rejects(() => readFile(join(dir, 'archive', 'notes.txt'), 'utf8'), /ENOENT/)
  })
})

test('list_dir and read_binary_file expose bounded metadata', async () => {
  await withTempDir(async (dir) => {
    await mkdir(join(dir, 'nested'), { recursive: true })
    await writeFile(join(dir, 'nested', 'child.txt'), 'child\n', 'utf8')
    const binary = Buffer.from([0, 1, 2, 3, 4])
    await writeFile(join(dir, 'data.bin'), binary)
    const registry = createToolRegistry({ cwd: dir })

    const shallow = await registry.execute('list_dir', {})
    assert.deepEqual(shallow.output, [
      { name: 'data.bin', type: 'file' },
      { name: 'nested', type: 'dir' },
    ])

    const recursive = await registry.execute('list_dir', { depth: 2, limit: 10 })
    assert.deepEqual(recursive.output.entries.map((entry) => [entry.path, entry.type]), [
      ['data.bin', 'file'],
      ['nested', 'dir'],
      ['nested/child.txt', 'file'],
    ])
    assert.equal(recursive.output.truncated, false)

    const readBinary = await registry.execute('read_binary_file', { path: 'data.bin', maxBytes: 3 })
    assert.equal(readBinary.output.path, 'data.bin')
    assert.equal(readBinary.output.bytes, 5)
    assert.equal(readBinary.output.truncated, true)
    assert.equal(readBinary.output.base64, binary.subarray(0, 3).toString('base64'))
  })
})

test('read_image returns metadata for PNG, GIF, JPEG, and WebP', async () => {
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

    const gif = Buffer.from([
      0x47, 0x49, 0x46, 0x38, 0x39, 0x61,
      0x04, 0x00, 0x05, 0x00,
    ])
    await writeFile(join(dir, 'tiny.gif'), gif)
    const gifImage = await registry.execute('read_image', { path: 'tiny.gif' })
    assert.equal(gifImage.output.mimeType, 'image/gif')
    assert.equal(gifImage.output.width, 4)
    assert.equal(gifImage.output.height, 5)

    const jpeg = Buffer.from([
      0xff, 0xd8,
      0xff, 0xc0, 0x00, 0x11, 0x08,
      0x00, 0x07,
      0x00, 0x06,
      0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x00, 0x03, 0x11, 0x00,
    ])
    await writeFile(join(dir, 'tiny.jpg'), jpeg)
    const jpegImage = await registry.execute('read_image', { path: 'tiny.jpg' })
    assert.equal(jpegImage.output.mimeType, 'image/jpeg')
    assert.equal(jpegImage.output.width, 6)
    assert.equal(jpegImage.output.height, 7)

    const webp = Buffer.concat([
      Buffer.from('RIFF', 'ascii'),
      Buffer.from([0x16, 0x00, 0x00, 0x00]),
      Buffer.from('WEBPVP8X', 'ascii'),
      Buffer.from([0x0a, 0x00, 0x00, 0x00]),
      Buffer.from([0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x09, 0x00, 0x00]),
    ])
    await writeFile(join(dir, 'tiny.webp'), webp)
    const webpImage = await registry.execute('read_image', { path: 'tiny.webp' })
    assert.equal(webpImage.output.mimeType, 'image/webp')
    assert.equal(webpImage.output.width, 9)
    assert.equal(webpImage.output.height, 10)
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
    const csvRange = await registry.execute('read_document', { path: 'data.csv', range: '3-4' })
    assert.equal(csvRange.output.text, '| alpha | 1 |\n| beta, quoted | 2 |')
    assert.equal(csvRange.output.startLine, 3)
    assert.equal(csvRange.output.endLine, 4)
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
      'CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, active INTEGER NOT NULL); INSERT INTO users (name, active) VALUES (\'Ada\', 1), (\'Grace\', 0); CREATE TABLE nums (id INTEGER PRIMARY KEY); INSERT INTO nums (id) VALUES (1), (2), (3), (4), (5); CREATE TABLE "user data" ("account id" TEXT PRIMARY KEY, "display name" TEXT NOT NULL); INSERT INTO "user data" VALUES (\'acct-1\', \'Quoted Name\'); CREATE TABLE composite (tenant TEXT, slug TEXT, value TEXT, PRIMARY KEY (tenant, slug)); INSERT INTO composite VALUES (\'t1\', \'s1\', \'v1\');',
    ])
    const registry = createToolRegistry({ cwd: dir })

    const tables = await registry.execute('read_sqlite', { path: 'app.db' })
    assert.equal(tables.ok, true)
    assert.deepEqual(tables.output.tables, [{ name: 'composite', type: 'table', rows: 1 }, { name: 'nums', type: 'table', rows: 5 }, { name: 'user data', type: 'table', rows: 1 }, { name: 'users', type: 'table', rows: 2 }])

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

    const pagedQuery = await registry.execute('read_sqlite', { path: 'app.db', query: 'SELECT id FROM nums ORDER BY id', limit: 2, offset: 2 })
    assert.equal(pagedQuery.ok, true)
    assert.deepEqual(pagedQuery.output.rows, [{ id: 3 }, { id: 4 }])

    const writeCte = await registry.execute('read_sqlite', { path: 'app.db', query: 'WITH picked AS (SELECT id FROM users) DELETE FROM users' })
    assert.equal(writeCte.ok, false)
    const escapedQuery = await registry.execute('read_sqlite', { path: 'app.db', query: 'SELECT 1); DELETE FROM users; --' })
    assert.equal(escapedQuery.ok, false)
    assert.match(escapedQuery.error, /single SELECT or WITH/)


    const denied = await registry.execute('read_sqlite', { path: 'app.db', query: 'DELETE FROM users' })
    assert.equal(denied.ok, false)
    assert.match(denied.error, /SELECT or WITH/)
  })
})

test('read_archive lists and reads tar and zip entries', async () => {
  await withTempDir(async (dir) => {
    await writeFile(join(dir, 'bundle.tar'), makeTar('docs/readme.txt', 'tar text'), 'utf8')
    await writeFile(join(dir, 'bundle.zip'), makeZip('src/index.js', 'zip one\nzip two\nzip three'))
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
    assert.deepEqual(zipList.output.entries, [{ name: 'src/index.js', type: 'file', bytes: 25 }])
    const zipEntry = await registry.execute('read_archive', { path: 'bundle.zip', entry: 'src/index.js' })
    assert.equal(zipEntry.output.content, 'zip one\nzip two\nzip three')
    const zipRange = await registry.execute('read_archive', { path: 'bundle.zip', entry: 'src/index.js', range: '2-3' })
    assert.equal(zipRange.output.content, 'zip two\nzip three')
    assert.equal(zipRange.output.startLine, 2)
    assert.equal(zipRange.output.endLine, 3)
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

test('search_files hides git dot paths unless requested', async () => {
  await withTempDir(async (dir) => {
    await execFileOk('git', ['init'], { cwd: dir })
    await writeFile(join(dir, 'visible.txt'), 'git needle\n', 'utf8')
    await writeFile(join(dir, '.hidden.txt'), 'git needle\n', 'utf8')
    await mkdir(join(dir, '.secret'), { recursive: true })
    await writeFile(join(dir, '.secret', 'note.txt'), 'git needle\n', 'utf8')
    const registry = createToolRegistry({ cwd: dir })

    const hiddenDefault = await registry.execute('search_files', { query: 'git needle', limit: 10 })
    assert.deepEqual(hiddenDefault.output.matches.map((match) => match.path), ['visible.txt'])

    const hiddenIncluded = await registry.execute('search_files', { query: 'git needle', hidden: true, limit: 10 })
    assert.deepEqual(hiddenIncluded.output.matches.map((match) => match.path), ['.hidden.txt', '.secret/note.txt', 'visible.txt'])

    const explicitHiddenFile = await registry.execute('search_files', { path: '.hidden.txt', query: 'git needle', limit: 10 })
    assert.deepEqual(explicitHiddenFile.output.matches.map((match) => match.path), ['.hidden.txt'])

    const explicitHiddenDir = await registry.execute('search_files', { path: '.secret', query: 'git needle', limit: 10 })
    assert.deepEqual(explicitHiddenDir.output.matches.map((match) => match.path), ['.secret/note.txt'])

    const explicitHiddenGlob = await registry.execute('glob_paths', { path: '.secret', patterns: '**', limit: 10 })
    assert.deepEqual(explicitHiddenGlob.output.matches, [{ path: '.secret/note.txt', type: 'file' }])
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

test('artifact and todo tools use the session artifact directory', async () => {
  await withTempDir(async (dir) => {
    const artifactDir = join(dir, 'artifacts')
    const missingSession = createToolRegistry({ cwd: dir })
    const denied = await missingSession.execute('save_artifact', { name: 'x.txt', content: 'x' })
    assert.equal(denied.ok, false)
    assert.match(denied.error, /requires an active session/)

    const registry = createToolRegistry({ cwd: dir, artifactDir })
    const saved = await registry.execute('save_artifact', { name: 'analysis report.txt', content: 'alpha\nbeta\n' })
    assert.equal(saved.output.bytes, Buffer.byteLength('alpha\nbeta\n'))
    assert.equal(saved.output.path, join(artifactDir, 'analysis_report.txt'))
    const invalidDot = await registry.execute('save_artifact', { name: '.', content: 'bad' })
    assert.equal(invalidDot.ok, false)
    assert.match(invalidDot.error, /invalid artifact name/)
    const invalidParent = await registry.execute('save_artifact', { name: '..', content: 'bad' })
    assert.equal(invalidParent.ok, false)
    assert.match(invalidParent.error, /invalid artifact name/)


    const listed = await registry.execute('list_artifacts', {})
    assert.deepEqual(listed.output.artifacts.map((artifact) => artifact.name), ['analysis_report.txt'])

    const readArtifact = await registry.execute('read_artifact', { name: 'analysis_report.txt', maxBytes: 6 })
    assert.equal(readArtifact.output.name, 'analysis_report.txt')
    assert.equal(readArtifact.output.bytes, Buffer.byteLength('alpha\nbeta\n'))
    assert.equal(readArtifact.output.truncated, true)
    assert.equal(readArtifact.output.content, 'alpha\n')

    const initialized = await registry.execute('todo', { op: 'init', list: [{ phase: 'Build', items: ['Patch code', 'Run tests'] }] })
    assert.equal(initialized.output.total, 2)
    assert.equal(initialized.output.active, 'Patch code')

    const done = await registry.execute('todo', { op: 'done', task: 'Patch code' })
    assert.equal(done.output.completed, 1)
    assert.equal(done.output.active, 'Run tests')

    const appended = await registry.execute('todo', { op: 'append', phase: 'Review', items: ['Push branch'] })
    assert.equal(appended.output.total, 3)
    assert.deepEqual(appended.output.phases.map((phase) => phase.phase), ['Build', 'Review'])

    const view = await registry.execute('todo', { op: 'view' })
    assert.deepEqual(view.output.items.map((item) => [item.text, item.status, item.phase]), [
      ['Patch code', 'done', 'Build'],
      ['Run tests', 'in_progress', 'Build'],
      ['Push branch', 'pending', 'Review'],
    ])
  })
})

test('memory tool stores MemoryRecord metadata and recalls only visible provenance-backed records', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedMemory(dir, async (memoryFile) => {
      const otherRepo = join(dir, 'other-repo')
      await mkdir(otherRepo, { recursive: true })

      const registry = createToolRegistry({ cwd: dir })
      const saved = await registry.execute('memory', {
        op: 'remember',
        text: 'Prefer pnpm for release packaging in this repository',
        kind: 'user_preference',
        scope: { kind: 'repo', id: dir },
        tags: ['release', 'packaging', 'release'],
        confidence: 0.91,
      })
      assert.equal(saved.ok, true)
      assert.match(saved.output.entry.id, /^[a-z0-9]+-[a-z0-9]+$/)
      assert.equal(saved.output.entry.kind, 'user_preference')
      assert.deepEqual(saved.output.entry.scope, { kind: 'repo', id: dir })
      assert.deepEqual(saved.output.entry.tags, ['release', 'packaging'])
      assert.deepEqual(saved.output.entry.source, { origin: 'memory_tool' })
      assert.equal(saved.output.entry.confidence, 0.91)
      assert.match(saved.output.entry.createdAt, /^\d{4}-\d{2}-\d{2}T/)

      const currentMemory = await readFile(memoryFile, 'utf8')
      await writeFile(memoryFile, `${currentMemory}${JSON.stringify({
        id: 'legacy-note',
        text: 'Legacy deploy checklist requires a changelog review',
        tags: ['legacy', 'deploy'],
        createdAt: '2024-01-01T00:00:00.000Z',
      })}\n`, 'utf8')

      const recallRepo = await registry.execute('memory', { op: 'recall', query: 'release packaging' })
      assert.equal(recallRepo.ok, true)
      assert.deepEqual(recallRepo.output.entries.map((entry) => entry.id), [saved.output.entry.id])
      assert.deepEqual(recallRepo.output.entries[0].source, { origin: 'memory_tool' })
      assert.deepEqual(recallRepo.output.entries[0].scope, { kind: 'repo', id: dir })

      const otherRegistry = createToolRegistry({ cwd: otherRepo })
      const recallOtherRepo = await otherRegistry.execute('memory', { op: 'recall', query: 'release packaging' })
      assert.deepEqual(recallOtherRepo.output.entries, [])

      const recallLegacy = await otherRegistry.execute('memory', { op: 'recall', query: 'legacy deploy' })
      assert.deepEqual(recallLegacy.output.entries.map((entry) => ({
        id: entry.id,
        kind: entry.kind,
        scope: entry.scope,
        text: entry.text,
        source: entry.source,
        confidence: entry.confidence,
      })), [{
        id: 'legacy-note',
        kind: 'note',
        scope: { kind: 'global', id: 'global' },
        text: 'Legacy deploy checklist requires a changelog review',
        source: { origin: 'legacy_memory_tool' },
        confidence: 0.4,
      }])
    })
  })
})

test('local memory backend forgets repo scope without deleting global memories', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedMemory(dir, async (memoryFile) => {
      const otherRepo = join(dir, 'other-repo')
      const backend = createLocalMemoryBackend({ file: memoryFile, cwd: dir })
      await backend.remember([
        { text: 'Global instruction survives repository cleanup', scope: { kind: 'global', id: 'global' }, tags: ['cleanup'] },
        { text: 'Repo-specific instruction is removable', scope: { kind: 'repo', id: dir }, tags: ['cleanup'] },
        { text: 'Other repository instruction survives', scope: { kind: 'repo', id: otherRepo }, tags: ['cleanup'] },
      ])

      const forgotten = await backend.forget({ kind: 'repo', id: dir })
      assert.deepEqual(forgotten, { removed: 1 })

      const remaining = await loadMemoryRecords(memoryFile, { cwd: dir })
      assert.deepEqual(remaining.map((entry) => [entry.text, entry.scope]), [
        ['Global instruction survives repository cleanup', { kind: 'global', id: 'global' }],
        ['Other repository instruction survives', { kind: 'repo', id: otherRepo }],
      ])
    })
  })
})

test('system prompt enforces dedicated tool policy', () => {
  const prompt = systemPrompt(createToolRegistry().list())
  assert.match(prompt, /Use glob_paths\/list_dir for file discovery/)
  assert.match(prompt, /Use grep_regex\/search_files for content search/)
  assert.match(prompt, /do not use run_command\/run_process for grep, find, ls, or globbing/)
  assert.match(prompt, /Use read_file ranges\/selectors for targeted reads/)
})

test('capability manifest lists built-in tools and runtime defaults', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedHome(dir, async () => {
      const previous = {
        JEDEN_MODEL: process.env.JEDEN_MODEL,
        MODEL_ROUTER_URL: process.env.MODEL_ROUTER_URL,
        WISENT_APP_AGENT_ID: process.env.WISENT_APP_AGENT_ID,
      }
      delete process.env.JEDEN_MODEL
      delete process.env.MODEL_ROUTER_URL
      delete process.env.WISENT_APP_AGENT_ID
      try {
        const expectedRoute = modelRouterConfig({})
        const manifest = await buildCapabilityManifest({ cwd: dir })
        assert.equal(manifest.cwd, dir)
        assert.equal(manifest.model, expectedRoute.model)
        assert.equal(manifest.agentId, expectedRoute.agentId)
        assert.equal(manifest.modelRouterUrl, expectedRoute.url)
        assert.ok(manifest.tools.total >= manifest.tools.builtIn.length)
        assert.ok(manifest.tools.builtIn.some((tool) => tool.name === 'read_file'))
      } finally {
        for (const [key, value] of Object.entries(previous)) {
          if (value === undefined) delete process.env[key]
          else process.env[key] = value
        }
      }
    })
  })
})

test('doctor reports missing model router auth as fatal', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedHome(dir, async () => {
      const previous = {
        WISENT_APP_AGENT_AUTH_SECRET: process.env.WISENT_APP_AGENT_AUTH_SECRET,
        JEDEN_MEMORY_FILE: process.env.JEDEN_MEMORY_FILE,
        JEDEN_MODEL: process.env.JEDEN_MODEL,
        MODEL_ROUTER_URL: process.env.MODEL_ROUTER_URL,
        WISENT_APP_AGENT_ID: process.env.WISENT_APP_AGENT_ID,
      }
      delete process.env.WISENT_APP_AGENT_AUTH_SECRET
      delete process.env.JEDEN_MODEL
      delete process.env.MODEL_ROUTER_URL
      delete process.env.WISENT_APP_AGENT_ID
      process.env.JEDEN_MEMORY_FILE = join(dir, 'memory.jsonl')
      try {
        const expectedRoute = modelRouterConfig({})
        const report = await buildDoctorReport({ cwd: dir })
        const auth = report.checks.find((item) => item.id === 'secret.modelRouterAuth.present')
        assert.equal(report.ok, false)
        assert.equal(auth.ok, false)
        assert.equal(auth.fatal, true)
        assert.equal(report.modelRouterUrl, expectedRoute.url)
      } finally {
        for (const [key, value] of Object.entries(previous)) {
          if (value === undefined) delete process.env[key]
          else process.env[key] = value
        }
      }
    })
  })
})

test('CLI emits capability and doctor JSON', async () => {
  await withTempDir(async (dir) => {
    const env = { ...process.env, HOME: join(dir, 'home'), USERPROFILE: join(dir, 'home'), JEDEN_MEMORY_FILE: join(dir, 'memory.jsonl'), WISENT_APP_AGENT_AUTH_SECRET: 'test-secret' }
    const capabilities = await execFileOk(process.execPath, ['src/cli.js', 'capabilities', '--cwd', dir], { cwd: join(process.cwd()), env })
    const manifest = JSON.parse(capabilities.stdout)
    assert.ok(manifest.tools.all.some((tool) => tool.name === 'read_file'))

    const doctor = await execFileOk(process.execPath, ['src/cli.js', 'doctor', '--cwd', dir], { cwd: join(process.cwd()), env })
    const report = JSON.parse(doctor.stdout)
    assert.equal(report.checks.find((item) => item.id === 'secret.modelRouterAuth.present').ok, true)
    assert.equal(report.tools.total, manifest.tools.total)
  })
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
    assert.equal(result.output.stdoutTruncated, false)
    assert.equal(result.output.stderrTruncated, false)
  })
})

test('run_process reports capped stdout and stderr', async () => {
  await withTempDir(async (dir) => {
    const registry = createToolRegistry({ cwd: dir, allowCommand: true })
    const result = await registry.execute('run_process', {
      command: process.execPath,
      args: ['-e', 'process.stdout.write("x".repeat(100050)); process.stderr.write("e".repeat(100050))'],
    })
    assert.equal(result.ok, true)
    assert.equal(result.output.code, 0)
    assert.equal(result.output.stdout.length, 100000)
    assert.equal(result.output.stderr.length, 100000)
    assert.equal(result.output.stdoutTruncated, true)
    assert.equal(result.output.stderrTruncated, true)
  })
})

test('run_process escalates timed-out children to SIGKILL', async () => {
  await withTempDir(async (dir) => {
    const registry = createToolRegistry({ cwd: dir, allowCommand: true })
    const result = await registry.execute('run_process', {
      command: process.execPath,
      args: ['-e', 'process.on("SIGTERM", () => {}); setInterval(() => {}, 1000)'],
      timeoutMs: 50,
    })
    assert.equal(result.ok, true)
    assert.equal(result.output.timedOut, true)
    assert.equal(result.output.signal, 'SIGKILL')
  })
})

test('run_command timeout kills shell grandchildren', async () => {
  await withTempDir(async (dir) => {
    const registry = createToolRegistry({ cwd: dir, allowCommand: true })
    const command = `${JSON.stringify(process.execPath)} -e 'setTimeout(() => require("fs").writeFileSync("marker", "late"), 3000)' & wait`
    const result = await registry.execute('run_command', { command, timeoutMs: 1000 })
    assert.equal(result.ok, true)
    assert.equal(result.output.timedOut, true)
    await new Promise((resolve) => setTimeout(resolve, 2300))
    await assert.rejects(() => readFile(join(dir, 'marker'), 'utf8'), /ENOENT/)
  })
})

test('eval and package script tools execute with command permission', async () => {
  await withTempDir(async (dir) => {
    await writeFile(join(dir, 'package.json'), JSON.stringify({ scripts: { echoenv: 'node -e "process.stdout.write(process.env.JEDEN_SCRIPT_VALUE)"' } }), 'utf8')
    const locked = createToolRegistry({ cwd: dir })
    const denied = await locked.execute('node_eval', { code: 'console.log("nope")' })
    assert.equal(denied.ok, false)
    assert.match(denied.error, /requires --allow-command/)

    const registry = createToolRegistry({ cwd: dir, allowCommand: true })
    const node = await registry.execute('node_eval', { code: 'console.log(JSON.stringify({ ok: 2 + 3 }))', timeoutMs: 1000 })
    assert.equal(node.output.code, 0)
    assert.equal(node.output.stdout.trim(), '{"ok":5}')

    let hasPython3 = true
    try {
      await execFileOk('python3', ['--version'])
    } catch {
      hasPython3 = false
    }
    if (hasPython3) {
      const python = await registry.execute('python_eval', { code: 'print("py-ok")', timeoutMs: 1000 })
      assert.equal(python.output.code, 0)
      assert.equal(python.output.stdout.trim(), 'py-ok')
    }

    const scripts = await registry.execute('list_package_scripts', {})
    assert.deepEqual(scripts.output, { echoenv: 'node -e "process.stdout.write(process.env.JEDEN_SCRIPT_VALUE)"' })
    const script = await registry.execute('run_package_script', { script: 'echoenv', env: { JEDEN_SCRIPT_VALUE: 'script-ok' }, timeoutMs: 5000 })
    assert.equal(script.output.code, 0)
    assert.equal(script.output.stdout.trim().split('\n').at(-1), 'script-ok')
  })
})

test('git read tools expose status, diff, log, and show', async () => {
  await withTempDir(async (dir) => {
    await execFileOk('git', ['init'], { cwd: dir })
    await writeFile(join(dir, 'tracked.txt'), 'alpha\n', 'utf8')
    await execFileOk('git', ['add', 'tracked.txt'], { cwd: dir })
    await execFileOk('git', ['-c', 'user.name=Jeden Test', '-c', 'user.email=jeden@example.com', 'commit', '-m', 'initial'], { cwd: dir })
    await writeFile(join(dir, 'tracked.txt'), 'alpha\nbeta\n', 'utf8')
    await writeFile(join(dir, 'new.txt'), 'new\n', 'utf8')
    const registry = createToolRegistry({ cwd: dir })

    const status = await registry.execute('git_status', {})
    assert.match(status.output.stdout, / M tracked\.txt/)
    assert.match(status.output.stdout, /\?\? new\.txt/)

    const diff = await registry.execute('git_diff', { path: 'tracked.txt' })
    assert.match(diff.output.stdout, /\+beta/)

    const log = await registry.execute('git_log', { limit: 1 })
    assert.match(log.output.stdout, /initial/)

    const show = await registry.execute('git_show', { ref: 'HEAD' })
    assert.match(show.output.stdout, /initial/)
    assert.match(show.output.stdout, /tracked\.txt/)
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
      const rawRange = await registry.execute('fetch_url', { url: `${origin}/table.tsv`, range: '2-3', maxBytes: 1000, timeoutMs: 1000 })
      assert.equal(rawRange.output.text, 'alpha\t1\nbeta\t2')
      assert.equal(rawRange.output.startLine, 2)
      assert.equal(rawRange.output.endLine, 3)


      const json = await registry.execute('fetch_readable_url', { url: `${origin}/data.json` })
      assert.equal(json.output.text, '{\n  "name": "Jeden",\n  "nested": {\n    "ok": true\n  }\n}')

      const feed = await registry.execute('fetch_readable_url', { url: `${origin}/feed.xml` })
      assert.equal(feed.output.text, '# News\n- First — https://example.com/first\n- Second')

      const table = await registry.execute('fetch_readable_url', { url: `${origin}/table.tsv` })
      assert.equal(table.output.text, '| name | count |\n| --- | --- |\n| alpha | 1 |\n| beta | 2 |')
      const tableRange = await registry.execute('fetch_readable_url', { url: `${origin}/table.tsv`, range: '3-4' })
      assert.equal(tableRange.output.text, '| alpha | 1 |\n| beta | 2 |')
      assert.equal(tableRange.output.startLine, 3)
      assert.equal(tableRange.output.endLine, 4)

      const pdf = await registry.execute('fetch_readable_url', { url: `${origin}/paper.pdf` })
      assert.equal(pdf.output.text, 'Remote PDF text')
    })
  })
})

test('runJeden injects scoped memory into the first model system prompt', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedMemory(dir, async (memoryFile) => {
      const otherRepo = join(dir, 'other-repo')
      const backend = createLocalMemoryBackend({ file: memoryFile, cwd: dir })
      await backend.remember([
        {
          text: 'Repo release summaries must mention the risk register',
          kind: 'project_fact',
          scope: { kind: 'repo', id: dir },
          tags: ['release'],
          source: { origin: 'fixture' },
          confidence: 0.8,
        },
        {
          text: 'Global release notes should include customer impact',
          kind: 'user_preference',
          scope: { kind: 'global', id: 'global' },
          tags: ['release'],
          source: { origin: 'fixture' },
          confidence: 0.7,
        },
        {
          text: 'Other repository release summaries use the deprecated staging checklist',
          kind: 'project_fact',
          scope: { kind: 'repo', id: otherRepo },
          tags: ['release'],
          source: { origin: 'fixture' },
          confidence: 0.9,
        },
      ])

      const recorder = makeInMemoryRecorder(dir, 'recall-session')
      let calls = 0
      const chat = async ({ messages }) => {
        calls += 1
        assert.equal(calls, 1)
        assert.match(messages[0].content, /Durable memory \(scoped, provenance-backed/)
        assert.match(messages[0].content, /Repo release summaries must mention the risk register/)
        assert.match(messages[0].content, /Global release notes should include customer impact/)
        assert.doesNotMatch(messages[0].content, /deprecated staging checklist/)
        assert.equal(messages.at(-1).content, 'Write a release summary for this repository.')
        return JSON.stringify({ action: 'final', text: 'release summary complete' })
      }

      const result = await runJeden({
        task: 'Write a release summary for this repository.',
        cwd: dir,
        chat,
        recorder,
        maxSteps: 1,
      })

      assert.equal(result.text, 'release summary complete')
      assert.equal(calls, 1)
      const recallEvent = recorder.events.find((event) => event.type === 'memory_recall')
      assert.equal(recallEvent?.data.count, 2)
    })
  })
})

test('runJeden learns a run_episode memory after a successful final answer', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedMemory(dir, async (memoryFile) => {
      const recorder = makeInMemoryRecorder(dir, 'learn-session')
      const chat = async () => JSON.stringify({ action: 'final', text: 'Documented the release procedure.' })

      const result = await runJeden({
        task: 'Summarize the release procedure.',
        cwd: dir,
        chat,
        recorder,
        maxSteps: 1,
      })

      assert.equal(result.text, 'Documented the release procedure.')
      const records = await loadMemoryRecords(memoryFile, { cwd: dir })
      assert.equal(records.length, 1)
      assert.equal(records[0].kind, 'run_episode')
      assert.deepEqual(records[0].scope, { kind: 'repo', id: dir })
      assert.equal(records[0].source.origin, 'runJeden')
      assert.equal(records[0].source.runId, 'learn-session')
      assert.equal(records[0].source.sessionPath, join(dir, 'sessions', 'learn-session'))
      assert.equal(records[0].source.verified, false)
      assert.deepEqual(records[0].tags, ['auto', 'run', 'unverified'])
      assert.equal(records[0].confidence, 0.45)
      assert.match(records[0].text, /Completed run for task: Summarize the release procedure\./)
      assert.match(records[0].text, /Final result: Documented the release procedure\./)

      const learnedEvent = recorder.events.find((event) => event.type === 'memory_learned')
      assert.deepEqual(learnedEvent?.data, {
        id: records[0].id,
        kind: 'run_episode',
        confidence: 0.45,
      })
    })
  })
})

test('runJeden records memory errors without failing successful runs', async () => {
  await withTempDir(async (dir) => {
    const unreadableMemoryPath = join(dir, 'memory-path-is-directory')
    await mkdir(unreadableMemoryPath, { recursive: true })
    await withIsolatedMemory(dir, async () => {
      const recorder = makeInMemoryRecorder(dir, 'read-error-session')
      const chat = async () => JSON.stringify({ action: 'final', text: 'completed despite recall failure' })

      const result = await runJeden({
        task: 'Finish even if memory cannot be read.',
        cwd: dir,
        chat,
        recorder,
        maxSteps: 1,
      })

      assert.equal(result.text, 'completed despite recall failure')
      assert.ok(recorder.events.some((event) => event.type === 'memory_error' && event.data.stage === 'recall'))
    }, unreadableMemoryPath)
  })

  await withTempDir(async (dir) => {
    const readOnlyMemoryFile = join(dir, 'memory.jsonl')
    await writeFile(readOnlyMemoryFile, '', 'utf8')
    await chmod(readOnlyMemoryFile, 0o444)
    await withIsolatedMemory(dir, async () => {
      const recorder = makeInMemoryRecorder(dir, 'write-error-session')
      const chat = async () => JSON.stringify({ action: 'final', text: 'completed despite learn failure' })

      const result = await runJeden({
        task: 'Finish even if memory cannot be written.',
        cwd: dir,
        chat,
        recorder,
        maxSteps: 1,
      })

      assert.equal(result.text, 'completed despite learn failure')
      assert.ok(recorder.events.some((event) => event.type === 'memory_error' && event.data.stage === 'learn'))
    }, readOnlyMemoryFile)
  })
})

test('self-repair: runJeden records run_error with original max-steps failure', async () => {
  await withTempDir(async (dir) => {
    const recorder = makeInMemoryRecorder(dir, 'max-steps-session')
    const chat = async () => JSON.stringify({ action: 'tool', tool: 'list_dir', input: {} })

    await assert.rejects(
      runJeden({
        task: 'List files forever.',
        cwd: dir,
        chat,
        recorder,
        maxSteps: 1,
        memory: false,
      }),
      /max steps exceeded: 1/,
    )

    const runError = recorder.events.find((event) => event.type === 'run_error')
    assert.deepEqual(runError?.data, { message: 'max steps exceeded: 1' })
  })
})

test('self-repair: permission gating protects own package unless explicitly allowed', async () => {
  await withTempDir(async (dir) => {
    const packageRoot = join(dir, 'jeden')
    const packageChild = join(packageRoot, 'src')
    const projectCwd = join(dir, 'consumer-project')
    await mkdir(packageChild, { recursive: true })
    await mkdir(projectCwd, { recursive: true })

    assert.deepEqual(
      selfRepairPermissions({ cwd: packageChild, packageRoot, allowWrite: true, allowCommand: true }),
      { allowWrite: false, allowCommand: false, ownCodeProtected: true, packageRoot },
    )
    assert.deepEqual(
      selfRepairPermissions({ cwd: packageChild, packageRoot, allowWrite: true, allowCommand: true, allowOwnCode: true }),
      { allowWrite: true, allowCommand: true, ownCodeProtected: false, packageRoot },
    )
    assert.deepEqual(
      selfRepairPermissions({ cwd: projectCwd, packageRoot, allowWrite: true, allowCommand: true }),
      { allowWrite: true, allowCommand: true, ownCodeProtected: false, packageRoot },
    )
  })
})

test('self-repair: CLI repairs a max-steps run and records requested transcript', async () => {
  await withTempDir(async (dir) => {
    const cwd = join(dir, 'project')
    const home = join(dir, 'home')
    await mkdir(cwd, { recursive: true })
    await mkdir(home, { recursive: true })

    const requests = []
    await withHttpServer((request, response) => {
      let body = ''
      request.setEncoding('utf8')
      request.on('data', (chunk) => {
        body += chunk
      })
      request.on('end', () => {
        requests.push(JSON.parse(body))
        response.writeHead(200, { 'content-type': 'application/json' })
        const content = requests.length === 1
          ? JSON.stringify({ action: 'tool', tool: 'list_dir', input: {} })
          : JSON.stringify({ action: 'final', text: 'self-repair completed' })
        response.end(JSON.stringify({ choices: [{ message: { content } }] }))
      })
    }, async (origin) => {
      const task = 'Repair the generated release notes.'
      const env = {
        ...process.env,
        HOME: home,
        USERPROFILE: home,
        JEDEN_MEMORY_FILE: join(dir, 'memory.jsonl'),
        MODEL_ROUTER_URL: origin,
        WISENT_APP_AGENT_AUTH_SECRET: 'test-secret',
        JEDEN_HOOKS: '0',
      }
      const { stdout, stderr } = await execFileOk(
        process.execPath,
        ['src/cli.js', 'run', task, '--cwd', cwd, '--max-steps', '1', '--self-repair', '--json'],
        { cwd: process.cwd(), env },
      )

      assert.equal(stderr, '')
      assert.equal(requests.length, 2)
      const output = JSON.parse(stdout)
      assert.equal(output.ok, true)
      assert.equal(output.repaired, true)
      assert.equal(output.originalError, 'max steps exceeded: 1')
      assert.equal(output.text, 'self-repair completed')

      const repairMessages = requests[1].messages
      const replayedMessages = repairMessages.slice(1, -1)
      assert.deepEqual(replayedMessages.map((message) => message.role), ['user', 'assistant', 'user'])
      assert.equal(replayedMessages[0].content, task)
      assert.deepEqual(JSON.parse(replayedMessages[1].content), { action: 'tool', tool: 'list_dir', input: {} })
      const replayedToolResult = JSON.parse(replayedMessages[2].content)
      assert.equal(replayedToolResult.type, 'tool_result')
      assert.deepEqual(replayedToolResult.result, { ok: true, output: [] })

      const repairPrompt = repairMessages.at(-1).content
      assert.match(repairPrompt, /A previous Jeden run failed\. Self-repair mode is enabled\./)
      assert.match(repairPrompt, new RegExp(task.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')))
      assert.match(repairPrompt, /Failure:\nmax steps exceeded: 1/)
      assert.match(repairPrompt, new RegExp(`${output.sessionPath.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}/transcript\\.jsonl`))

      const transcript = (await readFile(join(output.sessionPath, 'transcript.jsonl'), 'utf8'))
        .trim()
        .split('\n')
        .map((line) => JSON.parse(line))
      const repairRequested = transcript.find((event) => event.type === 'self_repair_requested')
      assert.deepEqual(repairRequested?.data, {
        originalError: 'max steps exceeded: 1',
        allowWrite: false,
        allowCommand: false,
        ownCodeProtected: false,
      })
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
        memory: false,
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

    const result = await runJeden({ task: 'Read a file', cwd: dir, chat, hookRunner, maxSteps: 2, memory: false })
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
      const result = await runJeden({ task: 'Run slow reads', cwd: dir, chat, maxSteps: 2, memory: false })
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
    const result = await runJeden({ task: 'new task', cwd: dir, chat, priorMessages: replay, maxSteps: 1, memory: false })
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
      const registry = createToolRegistry({ cwd: dir })
      try {
        const directTools = await registry.execute('mcp_list_tools', { server: 'local', timeoutMs: 1000 })
        assert.deepEqual(directTools.output.tools.map((tool) => tool.name), ['echo'])
        const directCall = await registry.execute('mcp_call_tool', { server: 'local', tool: 'echo', args: { text: 'direct' }, timeoutMs: 1000 })
        assert.equal(directCall.output.content[0].text, 'direct:1')
      } finally {
        await closeMcpClients({ cwd: dir })
      }


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

      const result = await runJeden({ task: 'Call native MCP', cwd: dir, chat, maxSteps: 3, memory: false })
      assert.equal(result.text, 'native mcp ok')
    })
  })
})

test('configured MCP tools read resources and prompts directly', async () => {
  await withTempDir(async (dir) => {
    await withIsolatedHome(dir, async () => {
      const serverFile = join(dir, 'mcp-resource-server.mjs')
      await writeFile(serverFile, `
let buffer = Buffer.alloc(0)
function send(message) {
  const body = Buffer.from(JSON.stringify(message), 'utf8')
  process.stdout.write('Content-Length: ' + body.length + '\\r\\n\\r\\n')
  process.stdout.write(body)
}
function handle(message) {
  if (message.method === 'initialize') {
    send({ jsonrpc: '2.0', id: message.id, result: { protocolVersion: '2024-11-05', capabilities: {}, serverInfo: { name: 'fake-resources', version: '1' } } })
    return
  }
  if (message.method === 'resources/list') {
    send({ jsonrpc: '2.0', id: message.id, result: { resources: [{ uri: 'wisent://alpha', name: 'Alpha', mimeType: 'text/plain' }] } })
    return
  }
  if (message.method === 'resources/read') {
    send({ jsonrpc: '2.0', id: message.id, result: { contents: [{ uri: message.params.uri, mimeType: 'text/plain', text: 'resource:' + message.params.uri }] } })
    return
  }
  if (message.method === 'prompts/list') {
    send({ jsonrpc: '2.0', id: message.id, result: { prompts: [{ name: 'brief', description: 'Make a brief', arguments: [{ name: 'topic', required: true }] }] } })
    return
  }
  if (message.method === 'prompts/get') {
    send({ jsonrpc: '2.0', id: message.id, result: { description: 'Brief prompt', messages: [{ role: 'user', content: { type: 'text', text: 'Brief ' + message.params.arguments.topic } }] } })
  }
}
process.stdin.on('data', (chunk) => {
  buffer = Buffer.concat([buffer, chunk])
  for (;;) {
    const headerEnd = buffer.indexOf('\\r\\n\\r\\n')
    if (headerEnd === -1) break
    const header = buffer.subarray(0, headerEnd).toString('utf8')
    const length = Number(header.split('\\r\\n')[0].slice('Content-Length:'.length).trim())
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
      const registry = createToolRegistry({ cwd: dir })
      try {
        const resources = await registry.execute('mcp_list_resources', { server: 'local', timeoutMs: 1000 })
        assert.deepEqual(resources.output.resources, [{ uri: 'wisent://alpha', name: 'Alpha', mimeType: 'text/plain' }])
        const resource = await registry.execute('mcp_read_resource', { server: 'local', uri: 'wisent://alpha', timeoutMs: 1000 })
        assert.deepEqual(resource.output.contents, [{ uri: 'wisent://alpha', mimeType: 'text/plain', text: 'resource:wisent://alpha' }])

        const prompts = await registry.execute('mcp_list_prompts', { server: 'local', timeoutMs: 1000 })
        assert.deepEqual(prompts.output.prompts, [{ name: 'brief', description: 'Make a brief', arguments: [{ name: 'topic', required: true }] }])
        const prompt = await registry.execute('mcp_get_prompt', { server: 'local', name: 'brief', args: { topic: 'parity' }, timeoutMs: 1000 })
        assert.deepEqual(prompt.output.messages, [{ role: 'user', content: { type: 'text', text: 'Brief parity' } }])
      } finally {
        await closeMcpClients({ cwd: dir })
      }
    })
  })
})
