import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile, mkdir } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { parseAction } from '../src/protocol.js'
import { createToolRegistry } from '../src/tools.js'
import { loadProjectContext } from '../src/context.js'
import { SessionRecorder, listSessionArtifacts, readSessionArtifact, readSession, listSessions } from '../src/session.js'

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)))

async function withTempDir(fn) {
  const dir = await mkdtemp(join(repoRoot, '.tmp-test-'))
  try {
    return await fn(dir)
  } finally {
    await rm(dir, { recursive: true, force: true })
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
