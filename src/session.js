import { mkdir, appendFile, writeFile, readdir, readFile, stat } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join, resolve } from 'node:path'

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
  if (!idOrPath) throw new Error('session id or path is required')
  const dir = String(idOrPath).indexOf('/') === -1 ? join(root, String(idOrPath)) : resolve(String(idOrPath))
  const text = await readFile(join(dir, 'transcript.jsonl'), 'utf8')
  const events = text.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line))
  return { id: dir.split('/').pop(), path: dir, events }
}
