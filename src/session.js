import { mkdir, appendFile, writeFile, readdir, readFile, stat } from 'node:fs/promises'
import { homedir } from 'node:os'
import { basename, join, resolve } from 'node:path'
import { formatToolResult } from './protocol.js'

export function defaultSessionRoot() {
  return join(homedir(), '.jeden', 'sessions')
}

function stamp() {
  const suffix = Math.random().toString(36).slice(2, 8)
  return `${new Date().toISOString().replace(/[:.]/g, '-')}-${suffix}`
}

export class SessionRecorder {
  constructor({ root = defaultSessionRoot(), cwd = process.cwd(), id = stamp() } = {}) {
    this.id = id
    this.dir = resolve(root, id)
    this.cwd = resolve(cwd)
    this.ready = false
  }

  async ensure() {
    if (this.ready) return
    await mkdir(this.dir, { recursive: true })
    await mkdir(join(this.dir, 'artifacts'), { recursive: true })
    await writeFile(join(this.dir, 'state.json'), JSON.stringify({ id: this.id, cwd: this.cwd, startedAt: new Date().toISOString() }, null, 2))
    this.ready = true
  }

  async record(type, data) {
    await this.ensure()
    const event = { ts: new Date().toISOString(), type, data }
    await appendFile(join(this.dir, 'transcript.jsonl'), `${JSON.stringify(event)}\n`, 'utf8')
  }

  artifactDir() {
    return join(this.dir, 'artifacts')
  }

  async writeArtifact(name, content) {
    await this.ensure()
    const safeName = String(name || 'artifact.txt').replace(/[^a-zA-Z0-9._-]/g, '_')
    const file = join(this.artifactDir(), safeName)
    await writeFile(file, String(content ?? ''), 'utf8')
    await this.record('artifact', { name: safeName, path: file })
    return file
  }

  path() {
    return this.dir
  }
}

function sessionDir(idOrPath, root = defaultSessionRoot()) {
  if (!idOrPath) throw new Error('session id or path is required')
  return String(idOrPath).indexOf('/') === -1 ? join(root, String(idOrPath)) : resolve(String(idOrPath))
}

function artifactPath(dir, name) {
  if (!name) throw new Error('artifact name is required')
  const root = join(dir, 'artifacts')
  const file = resolve(root, String(name))
  if (file === root || file.slice(0, root.length + 1) === `${root}/`) return file
  throw new Error(`artifact path escapes session: ${name}`)
}

export async function listSessions({ root = defaultSessionRoot(), limit = 20 } = {}) {
  let entries = []
  try {
    entries = await readdir(root, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOENT') return []
    throw error
  }
  const rows = []
  for (const entry of entries) {
    if (!entry.isDirectory()) continue
    const dir = join(root, entry.name)
    let state = {}
    let info = null
    try { state = JSON.parse(await readFile(join(dir, 'state.json'), 'utf8')) } catch {}
    try { info = await stat(join(dir, 'transcript.jsonl')) } catch {}
    rows.push({
      id: entry.name,
      path: dir,
      cwd: state.cwd || null,
      startedAt: state.startedAt || null,
      updatedAt: info ? info.mtime.toISOString() : state.startedAt || null,
    })
  }
  return rows.sort((a, b) => String(b.updatedAt || '').localeCompare(String(a.updatedAt || ''))).slice(0, limit)
}

export async function readSession({ idOrPath, root = defaultSessionRoot() }) {
  const dir = sessionDir(idOrPath, root)
  const text = await readFile(join(dir, 'transcript.jsonl'), 'utf8')
  const events = text.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line))
  return { id: basename(dir), path: dir, events }
}

function clippedToolResult(result) {
  const text = formatToolResult(result)
  if (Buffer.byteLength(text, 'utf8') <= 20_000) return text
  return JSON.stringify({
    type: 'tool_result',
    result: {
      truncated: true,
      bytes: Buffer.byteLength(text, 'utf8'),
      preview: text.slice(0, 4_000),
    },
  })
}

export function sessionReplayMessages(session, { limit = 80 } = {}) {
  const messages = []
  for (const event of session.events || []) {
    if (event.type === 'user') messages.push({ role: 'user', content: String(event.data?.task || '') })
    else if (event.type === 'assistant_raw') messages.push({ role: 'assistant', content: String(event.data?.content || '') })
    else if (event.type === 'tool_result') messages.push({ role: 'user', content: clippedToolResult(event.data?.result || {}) })
    else if (event.type === 'final' && messages[messages.length - 1]?.role !== 'assistant') messages.push({ role: 'assistant', content: JSON.stringify({ action: 'final', text: event.data?.text || '' }) })
  }
  return messages.filter((message) => message.content).slice(-limit)
}

export async function listSessionArtifacts({ idOrPath, root = defaultSessionRoot() } = {}) {
  const dir = sessionDir(idOrPath, root)
  const artifactDir = join(dir, 'artifacts')
  let entries = []
  try {
    entries = await readdir(artifactDir, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOENT') return { id: basename(dir), path: dir, artifacts: [] }
    throw error
  }
  const artifacts = []
  for (const entry of entries) {
    if (!entry.isFile()) continue
    const file = join(artifactDir, entry.name)
    const info = await stat(file)
    artifacts.push({ name: entry.name, path: file, bytes: info.size, updatedAt: info.mtime.toISOString() })
  }
  return { id: basename(dir), path: dir, artifacts: artifacts.sort((a, b) => a.name.localeCompare(b.name)) }
}

export async function readSessionArtifact({ idOrPath, name, root = defaultSessionRoot() } = {}) {
  const dir = sessionDir(idOrPath, root)
  const file = artifactPath(dir, name)
  return { id: basename(dir), name: basename(file), path: file, content: await readFile(file, 'utf8') }
}
