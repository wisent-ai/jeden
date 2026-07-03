import { mkdir, appendFile, writeFile, readdir, readFile, stat, rm } from 'node:fs/promises'
import { homedir, tmpdir } from 'node:os'
import { basename, dirname, join, resolve } from 'node:path'
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
    const statePath = join(this.dir, 'state.json')
    const startedAt = new Date().toISOString()
    const existing = await readJsonFile(statePath, {})
    await writeFile(statePath, JSON.stringify({ ...existing, id: this.id, cwd: this.cwd, startedAt: existing.startedAt || startedAt }, null, 2))
    await appendFile(join(this.dir, 'transcript.jsonl'), '', 'utf8')
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

async function readJsonFile(path, fallback = null) {
  try {
    return JSON.parse(await readFile(path, 'utf8'))
  } catch (error) {
    if (error?.code === 'ENOENT') return fallback
    throw error
  }
}

async function readTranscriptEvents(dir) {
  try {
    const text = await readFile(join(dir, 'transcript.jsonl'), 'utf8')
    return text.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line))
  } catch (error) {
    if (error?.code === 'ENOENT') return []
    throw error
  }
}

async function recordSessionEvent(dir, type, data) {
  await appendFile(join(dir, 'transcript.jsonl'), `${JSON.stringify({ ts: new Date().toISOString(), type, data })}\n`, 'utf8')
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
      title: state.title || null,
      startedAt: state.startedAt || null,
      updatedAt: info ? info.mtime.toISOString() : state.startedAt || null,
    })
  }
  return rows.sort((a, b) => String(b.updatedAt || '').localeCompare(String(a.updatedAt || ''))).slice(0, limit)
}

export async function readSession({ idOrPath, root = defaultSessionRoot() }) {
  const dir = sessionDir(idOrPath, root)
  await stat(dir)
  const events = await readTranscriptEvents(dir)
  const state = await readJsonFile(join(dir, 'state.json'), {})
  return { id: basename(dir), path: dir, state, events }
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

async function writeSessionState(dir, patch) {
  const statePath = join(dir, 'state.json')
  const current = await readJsonFile(statePath, {})
  const next = { ...current, ...patch, id: current.id || basename(dir) }
  await writeFile(statePath, JSON.stringify(next, null, 2), 'utf8')
  return next
}

function htmlEscape(value) {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

export function renderSessionHtml(session) {
  const events = session.events.map((event) => {
    const label = htmlEscape(`${event.ts || ''} ${event.type || ''}`.trim())
    const body = htmlEscape(JSON.stringify(event.data || {}, null, 2))
    return `<section class="event"><h2>${label}</h2><pre>${body}</pre></section>`
  }).join('\n')
  const title = session.state?.title || session.id
  return `<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <title>Jeden session ${htmlEscape(title)}</title>
  <style>
    body { font-family: ui-sans-serif, system-ui, sans-serif; margin: 2rem; background: #fafafa; color: #111; }
    .event { border: 1px solid #ddd; border-radius: 8px; background: white; margin: 1rem 0; padding: 1rem; }
    h1 { margin-bottom: 0; }
    h2 { font-size: 0.9rem; color: #555; margin-top: 0; }
    pre { white-space: pre-wrap; overflow-wrap: anywhere; }
  </style>
</head>
<body>
  <h1>Jeden session ${htmlEscape(title)}</h1>
  <p>${htmlEscape(session.path)}</p>
  ${events}
</body>
</html>
`
}

export function renderSessionMarkdown(session) {
  const title = session.state?.title || session.id
  const parts = [`# Jeden session ${title}`, '', session.path, '']
  for (const event of session.events) {
    parts.push(`## ${`${event.ts || ''} ${event.type || ''}`.trim()}`, '', '```json', JSON.stringify(event.data || {}, null, 2), '```', '')
  }
  return `${parts.join('\n')}\n`
}

export function renderSessionExport(session, { format = 'json' } = {}) {
  if (format === 'html') return renderSessionHtml(session)
  if (format === 'markdown' || format === 'md') return renderSessionMarkdown(session)
  if (format !== 'json') throw new Error(`unsupported session export format: ${format}`)
  return `${JSON.stringify(session, null, 2)}\n`
}

export async function exportSession({ idOrPath, root = defaultSessionRoot(), outputPath = null, format = 'json' } = {}) {
  const session = await readSession({ idOrPath, root })
  const content = renderSessionExport(session, { format })
  if (outputPath) {
    await mkdir(dirname(resolve(outputPath)), { recursive: true })
    await writeFile(outputPath, content, 'utf8')
  }
  return { id: session.id, path: session.path, outputPath: outputPath ? resolve(outputPath) : null, format, content }
}

export async function getSessionInfo({ idOrPath, root = defaultSessionRoot() } = {}) {
  const session = await readSession({ idOrPath, root })
  const transcriptPath = join(session.path, 'transcript.jsonl')
  let transcript = null
  try { transcript = await stat(transcriptPath) } catch {}
  const artifacts = await listSessionArtifacts({ idOrPath: session.path, root })
  return {
    id: session.id,
    path: session.path,
    cwd: session.state?.cwd || null,
    title: session.state?.title || null,
    startedAt: session.state?.startedAt || null,
    updatedAt: transcript ? transcript.mtime.toISOString() : session.state?.startedAt || null,
    eventCount: session.events.length,
    artifactCount: artifacts.artifacts.length,
    transcriptBytes: transcript?.size || 0,
  }
}

export function formatSessionInfo(info) {
  return [
    `Session: ${info.id}`,
    `Path: ${info.path}`,
    `Workspace: ${info.cwd || '(unknown)'}`,
    `Title: ${info.title || '(untitled)'}`,
    `Started: ${info.startedAt || '(unknown)'}`,
    `Updated: ${info.updatedAt || '(unknown)'}`,
    `Events: ${info.eventCount}`,
    `Artifacts: ${info.artifactCount}`,
  ].join('\n')
}

export async function renameSession({ idOrPath, title, root = defaultSessionRoot() } = {}) {
  const cleanTitle = String(title || '').trim()
  if (!cleanTitle) throw new Error('session title is required')
  const dir = sessionDir(idOrPath, root)
  const state = await writeSessionState(dir, { title: cleanTitle, renamedAt: new Date().toISOString() })
  await recordSessionEvent(dir, 'session_renamed', { title: cleanTitle })
  return { id: basename(dir), path: dir, title: cleanTitle, state }
}

export async function moveSessionWorkspace({ idOrPath, cwd, root = defaultSessionRoot() } = {}) {
  if (!cwd) throw new Error('target workspace path is required')
  const dir = sessionDir(idOrPath, root)
  const target = resolve(String(cwd))
  const targetInfo = await stat(target)
  if (!targetInfo.isDirectory()) throw new Error(`target workspace is not a directory: ${target}`)
  const state = await writeSessionState(dir, { cwd: target, movedAt: new Date().toISOString() })
  await recordSessionEvent(dir, 'session_moved', { cwd: target })
  return { id: basename(dir), path: dir, cwd: target, state }
}

export async function deleteSession({ idOrPath, root = defaultSessionRoot() } = {}) {
  const dir = sessionDir(idOrPath, root)
  const state = await readJsonFile(join(dir, 'state.json'), null)
  const events = await readTranscriptEvents(dir)
  if (!state && events.length === 0) throw new Error(`refusing to delete ${dir}: not a Jeden session`)
  await rm(dir, { recursive: true, force: false })
  return { id: basename(dir), path: dir, deleted: true }
}

function summarizeEvent(event) {
  if (event.type === 'user') return `User: ${String(event.data?.task || '').slice(0, 500)}`
  if (event.type === 'final') return `Assistant final: ${String(event.data?.text || '').slice(0, 500)}`
  if (event.type === 'tool_result') return `Tool result: ${event.data?.tool || 'tool'}`
  if (event.type === 'artifact') return `Artifact: ${event.data?.name || event.data?.path || 'artifact'}`
  return `${event.type || 'event'}: ${JSON.stringify(event.data || {}).slice(0, 300)}`
}

export function buildSessionHandoffText(session, { focus = '', maxEvents = 80 } = {}) {
  const selected = (session.events || []).slice(-maxEvents)
  const lines = [
    `Handoff from Jeden session ${session.id}`,
    `Path: ${session.path}`,
    session.state?.cwd ? `Workspace: ${session.state.cwd}` : null,
    session.state?.title ? `Title: ${session.state.title}` : null,
    focus ? `Focus: ${String(focus).trim()}` : null,
    '',
    'Recent transcript summary:',
    ...selected.map((event) => `- ${summarizeEvent(event)}`),
  ].filter((line) => line !== null)
  return lines.join('\n')
}

export async function createHandoffSession({ idOrPath, root = defaultSessionRoot(), cwd = null, focus = '' } = {}) {
  const previous = await readSession({ idOrPath, root })
  const recorder = new SessionRecorder({ root, cwd: cwd || previous.state?.cwd || process.cwd() })
  await recorder.ensure()
  const text = buildSessionHandoffText(previous, { focus })
  await recorder.record('handoff_from', { id: previous.id, path: previous.path, focus: String(focus || '') })
  const artifactPath = await recorder.writeArtifact('handoff.md', text)
  return { id: recorder.id, path: recorder.path(), artifactPath, text, priorMessages: [{ role: 'user', content: text }] }
}

export function compactSessionMessages(session, { limit = 40, focus = '' } = {}) {
  const replay = sessionReplayMessages(session, { limit })
  const summary = buildSessionHandoffText(session, { focus, maxEvents: Math.max(limit, 20) })
  return [{ role: 'user', content: `Compacted previous session context:\n\n${summary}` }, ...replay.slice(-Math.max(Math.floor(limit / 2), 1))]
}

export async function compactSession({ idOrPath, root = defaultSessionRoot(), focus = '', limit = 40 } = {}) {
  const session = await readSession({ idOrPath, root })
  const messages = compactSessionMessages(session, { limit, focus })
  const artifactDir = join(session.path, 'artifacts')
  await mkdir(artifactDir, { recursive: true })
  const artifactPath = join(artifactDir, 'compact-context.md')
  await writeFile(artifactPath, messages[0].content, 'utf8')
  await recordSessionEvent(session.path, 'session_compacted', { artifactPath, focus: String(focus || ''), messageCount: messages.length })
  return { id: session.id, path: session.path, artifactPath, messages, text: `Compacted context written to ${artifactPath}.` }
}

export function shakeSessionMessages(session, { mode = 'elide', limit = 40 } = {}) {
  const messages = sessionReplayMessages(session, { limit })
  return messages.map((message) => {
    if (message.role !== 'user') return message
    let content = message.content
    if (mode === 'images') content = content.replace(/data:image\/[a-zA-Z0-9.+-]+;base64,[A-Za-z0-9+/=]+/g, '[image elided]')
    else content = content.length > 4_000 ? `${content.slice(0, 4_000)}\n[content elided by /shake]` : content
    return { ...message, content }
  })
}

export async function shakeSession({ idOrPath, root = defaultSessionRoot(), mode = 'elide', limit = 40 } = {}) {
  if (!new Set(['elide', 'images']).has(mode)) throw new Error('shake mode must be elide or images')
  const session = await readSession({ idOrPath, root })
  const messages = shakeSessionMessages(session, { mode, limit })
  await recordSessionEvent(session.path, 'session_shaken', { mode, messageCount: messages.length })
  return { id: session.id, path: session.path, mode, messages, text: `Prepared ${messages.length} shaken replay messages. Dispatcher must install them for future turns.` }
}

export async function dumpSession({ idOrPath, root = defaultSessionRoot(), outputDir = null } = {}) {
  const session = await readSession({ idOrPath, root })
  const dir = resolve(outputDir || join(tmpdir(), `jeden-dump-${session.id}`))
  await mkdir(dir, { recursive: true })
  const transcriptPath = join(dir, 'transcript.jsonl')
  const replayPath = join(dir, 'replay-messages.json')
  const transcript = (session.events || []).map((event) => JSON.stringify(event)).join('\n')
  await writeFile(transcriptPath, transcript ? `${transcript}\n` : '', 'utf8')
  await writeFile(replayPath, `${JSON.stringify(sessionReplayMessages(session), null, 2)}\n`, 'utf8')
  return { id: session.id, path: session.path, outputDir: dir, transcriptPath, replayPath, note: 'Jeden does not persist exact provider request JSON; replay-messages.json is the closest stored context.' }
}

export async function createNewSession({ root = defaultSessionRoot(), cwd = process.cwd() } = {}) {
  const recorder = new SessionRecorder({ root, cwd })
  await recorder.ensure()
  return { id: recorder.id, path: recorder.path(), recorder }
}

export async function dropSessionAndCreateNew({ recorder = null, idOrPath = null, root = defaultSessionRoot(), cwd = process.cwd() } = {}) {
  const current = idOrPath || recorder?.path?.()
  if (!current) throw new Error('current session is required')
  const next = await createNewSession({ root, cwd })
  const deleted = await deleteSession({ idOrPath: current, root })
  return { deleted, ...next }
}

export async function resumeSessionIntoNew({ idOrPath, root = defaultSessionRoot(), cwd = process.cwd() } = {}) {
  const previous = await readSession({ idOrPath, root })
  const recorder = new SessionRecorder({ root, cwd })
  await recorder.ensure()
  await recorder.record('resumed_from', { id: previous.id, path: previous.path })
  return { id: recorder.id, path: recorder.path(), recorder, previous, priorMessages: sessionReplayMessages(previous) }
}

export function freshSessionUnsupported() {
  throw new Error('Jeden has no persistent provider stream state to reset; use /new for a new local session or CLI resume to continue from saved transcript context.')
}
