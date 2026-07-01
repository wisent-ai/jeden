import { mkdir, appendFile, writeFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join, resolve } from 'node:path'

function stamp() {
  const suffix = Math.random().toString(36).slice(2, 8)
  return `${new Date().toISOString().replace(/[:.]/g, '-')}-${suffix}`
}

export class SessionRecorder {
  constructor({ root = join(homedir(), '.jeden', 'sessions'), cwd = process.cwd(), id = stamp() } = {}) {
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
