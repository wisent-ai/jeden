import { appendFile, mkdir, readFile, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { spawn } from 'node:child_process'
import { createCipheriv, randomBytes } from 'node:crypto'
import { fileURLToPath, pathToFileURL } from 'node:url'

function joinLines(values) {
  return values.filter((value) => value !== null && value !== undefined && value !== '').join('\n')
}

function timestampSlug() {
  return new Date().toISOString().replace(/[:.]/g, '-')
}

function safeArtifactName(value, fallback) {
  const safe = String(value || '').replace(/[^a-zA-Z0-9._-]/g, '_')
  if (!safe || safe === '.' || safe === '..') return fallback
  return safe
}

function channelId() {
  return `jeden-${randomBytes(9).toString('base64url')}`
}

function clipboardCandidates() {
  if (process.platform === 'darwin') return [['pbcopy', []]]
  if (process.platform === 'win32') {
    return [
      ['powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', 'Set-Clipboard -Value ([Console]::In.ReadToEnd())']],
      ['clip.exe', []],
    ]
  }
  return [
    ['wl-copy', []],
    ['xclip', ['-selection', 'clipboard']],
    ['xsel', ['--clipboard', '--input']],
  ]
}

function runClipboardCommand(command, args, payload) {
  return new Promise((resolveResult) => {
    const child = spawn(command, args, { stdio: ['pipe', 'ignore', 'pipe'] })
    let settled = false
    let stderr = ''
    const settle = (result) => {
      if (settled) return
      settled = true
      resolveResult(result)
    }
    child.stderr.on('data', (chunk) => { stderr += chunk.toString('utf8') })
    child.on('error', (error) => settle({ ok: false, error: error.message }))
    child.on('close', (code) => {
      if (code === 0) settle({ ok: true })
      else settle({ ok: false, error: stderr.trim() || `${command} exited with ${code}` })
    })
    child.stdin.on('error', (error) => settle({ ok: false, error: error.message }))
    child.stdin.end(payload)
  })
}

async function writeClipboard(payload) {
  let lastError = 'no clipboard command was attempted'
  for (const [command, args] of clipboardCandidates()) {
    const result = await runClipboardCommand(command, args, payload)
    if (result.ok) return { ok: true, command }
    lastError = result.error || `${command} failed`
  }
  return { ok: false, error: lastError }
}

export async function copyTextToClipboardOrArtifact({ payload, source, recorder }) {
  const result = await writeClipboard(payload)
  if (result.ok) return `Copied ${source} to the OS clipboard with ${result.command}.`
  const file = await recorder.writeArtifact('copy.txt', payload)
  return `OS clipboard is unavailable (${result.error}). Wrote ${source} to fallback artifact: ${file}`
}

export async function createEncryptedShareBundle({ session, artifactDir, copyLink = false }) {
  const createdAt = new Date().toISOString()
  const key = randomBytes(32)
  const iv = randomBytes(12)
  const plain = Buffer.from(JSON.stringify({ version: 1, kind: 'jeden-session', createdAt, session }, null, 2), 'utf8')
  const cipher = createCipheriv('aes-256-gcm', key, iv)
  const ciphertext = Buffer.concat([cipher.update(plain), cipher.final()])
  const bundle = {
    version: 1,
    kind: 'jeden-encrypted-share',
    backend: 'file',
    durable: true,
    algorithm: 'AES-256-GCM',
    createdAt,
    sessionId: session.id,
    iv: iv.toString('base64url'),
    tag: cipher.getAuthTag().toString('base64url'),
    ciphertext: ciphertext.toString('base64url'),
    note: 'Durable encrypted session bundle. The decryption key is carried only in the returned URL fragment; keep the fragment private.',
  }
  const file = join(artifactDir, `share-${safeArtifactName(session.id, 'session')}-${timestampSlug()}.jeden-share`)
  await mkdir(dirname(file), { recursive: true })
  await writeFile(file, `${JSON.stringify(bundle, null, 2)}\n`, 'utf8')
  const url = `${pathToFileURL(file).href}#key=${key.toString('base64url')}`
  const clipboard = copyLink ? await writeClipboard(url) : null
  return joinLines([
    `Encrypted durable share bundle written to ${file}`,
    `Share URL with decryption key: ${url}`,
    copyLink ? (clipboard.ok ? `Copied share URL to clipboard with ${clipboard.command}.` : `Could not copy share URL to clipboard: ${clipboard.error}`) : 'Add `copy`, `--copy`, or `--clipboard` to copy the share URL.',
    'Backend: durable local file bundle. Move or sync the file anywhere you trust; the URL fragment/key is never written into the bundle.',
  ])
}

function collabStateFile(cwd) {
  return resolve(cwd, '.jeden', 'collab.json')
}

async function loadPersistedCollabState(cwd) {
  try {
    const parsed = JSON.parse(await readFile(collabStateFile(cwd), 'utf8'))
    return {
      host: parsed?.host || null,
      guest: parsed?.guest || null,
      file: collabStateFile(cwd),
    }
  } catch (error) {
    if (error?.code === 'ENOENT' || error instanceof SyntaxError) return { host: null, guest: null, file: collabStateFile(cwd) }
    throw error
  }
}

async function savePersistedCollabState(cwd, collabState) {
  const file = collabStateFile(cwd)
  await mkdir(dirname(file), { recursive: true })
  await writeFile(file, `${JSON.stringify({ version: 1, updatedAt: new Date().toISOString(), host: collabState.host || null, guest: collabState.guest || null }, null, 2)}\n`, 'utf8')
  return file
}

async function hydrateCollabState(cwd, collabState = {}) {
  const persisted = await loadPersistedCollabState(cwd)
  collabState.host ??= persisted.host
  collabState.guest ??= persisted.guest
  return { collabState, stateFile: persisted.file }
}

function configRelayUrl(config = {}) {
  const candidates = [
    config.relayUrl,
    config.collabRelayUrl,
    config.relay?.url,
    config.relay?.baseUrl,
    config.collab?.relayUrl,
  ]
  const found = candidates.find((value) => typeof value === 'string' && value.trim())
  return found ? found.trim() : ''
}

function parsedUrl(value) {
  try { return new URL(value) } catch { return null }
}

function isHttpUrl(value) {
  const parsed = parsedUrl(value)
  return parsed?.protocol === 'http:' || parsed?.protocol === 'https:'
}

function isFileUrl(value) {
  return parsedUrl(value)?.protocol === 'file:'
}

function localRelayArtifactPath(artifactDir, label) {
  const name = label ? safeArtifactName(label, 'collab-relay.jsonl') : `collab-relay-${timestampSlug()}.jsonl`
  return join(artifactDir, name)
}

function localRelayInputPath(cwd, input) {
  if (isFileUrl(input)) return fileURLToPath(input)
  return resolve(cwd, input)
}

function localRelayFromConfiguredUrl(cwd, configured, artifactDir, label) {
  if (!configured) return { relayFile: localRelayArtifactPath(artifactDir, label) }
  const root = isFileUrl(configured) ? fileURLToPath(configured) : resolve(cwd, configured)
  if (root.endsWith('.jsonl')) return { relayFile: root }
  return { relayFile: join(root, `${safeArtifactName(label || channelId(), 'collab-relay')}.jsonl`) }
}

function eventsUrlFor(channelUrl) {
  const url = parsedUrl(channelUrl)
  if (!url) return ''
  if (url.pathname.endsWith('/events')) return url.href
  url.pathname = `${url.pathname.replace(/\/$/, '')}/events`
  return url.href
}

function configuredHttpChannel(baseUrl, label) {
  const base = new URL(baseUrl)
  if (!base.pathname.endsWith('/')) base.pathname = `${base.pathname}/`
  const channelPath = `${base.pathname}channels/${safeArtifactName(label || channelId(), 'collab-relay')}`.replace(/\/+/g, '/')
  base.pathname = channelPath
  return base.href
}

function entryFromRelayUrl(url, role, label) {
  if (isHttpUrl(url)) {
    const relayUrl = role === 'start-config' ? configuredHttpChannel(url, label) : url
    return { backend: 'http', relayUrl, eventsUrl: eventsUrlFor(relayUrl) }
  }
  return null
}

function activeCollab(entry) {
  if (!entry) return null
  if (typeof entry === 'string') return { backend: 'file', relayFile: entry, relayUrl: pathToFileURL(entry).href }
  if (entry.backend === 'http' || entry.relayUrl && isHttpUrl(entry.relayUrl)) return { backend: 'http', relayUrl: entry.relayUrl, eventsUrl: entry.eventsUrl || eventsUrlFor(entry.relayUrl), ...entry }
  if (entry.relayFile) return { backend: 'file', relayUrl: entry.relayUrl || pathToFileURL(entry.relayFile).href, ...entry }
  return entry
}

function startRelayEntry({ cwd, artifactDir, label, relay, relayConfig }) {
  if (relay) {
    if (isHttpUrl(relay)) return { backend: 'http', relayUrl: relay, eventsUrl: eventsUrlFor(relay) }
    const parsed = parsedUrl(relay)
    if (parsed && parsed.protocol !== 'file:') throw new Error(`relay URL protocol requires file:, http:, or https:: ${parsed.protocol}`)
    const relayFile = isFileUrl(relay) || relay.includes('/') || relay.startsWith('.') || relay.endsWith('.jsonl')
      ? localRelayInputPath(cwd, relay)
      : localRelayArtifactPath(artifactDir, relay)
    return { backend: 'file', relayFile, relayUrl: pathToFileURL(relayFile).href }
  }
  const configured = configRelayUrl(relayConfig)
  const http = configured ? entryFromRelayUrl(configured, 'start-config', label) : null
  if (http) return http
  const local = localRelayFromConfiguredUrl(cwd, configured, artifactDir, label)
  return { backend: 'file', relayFile: local.relayFile, relayUrl: pathToFileURL(local.relayFile).href }
}

function joinRelayEntry(cwd, target) {
  if (isHttpUrl(target)) return { backend: 'http', relayUrl: target, eventsUrl: eventsUrlFor(target) }
  const parsed = parsedUrl(target)
  if (parsed && parsed.protocol !== 'file:') throw new Error(`relay URL protocol requires file:, http:, or https:: ${parsed.protocol}`)
  const relayFile = localRelayInputPath(cwd, target)
  return { backend: 'file', relayFile, relayUrl: pathToFileURL(relayFile).href }
}

function eventRecord(type, data = {}) {
  return { ts: new Date().toISOString(), type, ...data }
}

async function appendFileRelayEvent(file, type, data = {}) {
  await mkdir(dirname(file), { recursive: true })
  await appendFile(file, `${JSON.stringify(eventRecord(type, data))}\n`, 'utf8')
}

function parseEventLines(text) {
  const events = []
  const lines = text.split(/\r?\n/).filter(Boolean)
  for (let index = 0; index < lines.length; index += 1) {
    try {
      events.push(JSON.parse(lines[index]))
    } catch (error) {
      events.push({ ts: null, type: 'invalid-event', line: index + 1, error: error.message })
    }
  }
  return events
}

async function readFileRelayEvents(file) {
  try {
    return parseEventLines(await readFile(file, 'utf8'))
  } catch (error) {
    if (error?.code === 'ENOENT') return []
    throw error
  }
}

async function appendHttpRelayEvent(active, type, data = {}) {
  const body = JSON.stringify(eventRecord(type, data))
  const response = await fetch(active.eventsUrl, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body,
  })
  if (!response.ok) throw new Error(`HTTP relay append failed (${response.status}): ${(await response.text()).slice(0, 300)}`)
}

function parseHttpEvents(text, contentType) {
  if (!text.trim()) return []
  const mediaType = String(contentType || '').split(';', 1)[0].trim().toLowerCase()
  if (mediaType === 'application/x-ndjson' || mediaType === 'application/jsonl') return parseEventLines(text)
  const parsed = JSON.parse(text)
  if (Array.isArray(parsed)) return parsed
  if (Array.isArray(parsed.events)) return parsed.events
  if (typeof parsed.events === 'string') return parseEventLines(parsed.events)
  return [parsed]
}

async function readHttpRelayEvents(active) {
  const response = await fetch(active.eventsUrl, { method: 'GET', headers: { accept: 'application/json, application/x-ndjson, text/plain' } })
  const text = await response.text()
  if (!response.ok) throw new Error(`HTTP relay read failed (${response.status}): ${text.slice(0, 300)}`)
  return parseHttpEvents(text, response.headers.get('content-type') || '')
}

async function appendRelayEvent(entry, type, data = {}) {
  const active = activeCollab(entry)
  if (!active) throw new Error('no active relay')
  if (active.backend === 'http') return appendHttpRelayEvent(active, type, data)
  return appendFileRelayEvent(active.relayFile, type, data)
}

async function readRelayEvents(entry) {
  const active = activeCollab(entry)
  if (!active) return []
  if (active.backend === 'http') return readHttpRelayEvents(active)
  return readFileRelayEvents(active.relayFile)
}

function relayDescriptor(active) {
  if (!active) return 'off'
  if (active.backend === 'http') return `configured HTTP durable relay: ${active.relayUrl}`
  return `durable file relay: ${active.relayFile}`
}

function eventSummary(event, index) {
  const extra = { ...event }
  delete extra.ts
  delete extra.type
  const suffix = Object.keys(extra).length ? ` ${JSON.stringify(extra)}` : ''
  return `${index + 1}. ${event.ts || '-'} ${event.type || 'event'}${suffix}`
}

async function collabStatusText(entry, role, mode = 'status') {
  const active = activeCollab(entry)
  if (!active) return `Collab ${role}: off.`
  try {
    const events = await readRelayEvents(active)
    const latest = events.length ? `${events[events.length - 1].type || 'event'} at ${events[events.length - 1].ts || '-'}` : 'none'
    const header = [
      `Collab ${role}: ${relayDescriptor(active)}`,
      `Relay URL: ${active.relayUrl || pathToFileURL(active.relayFile).href}`,
      active.eventsUrl ? `Events endpoint: ${active.eventsUrl}` : null,
      `Events: ${events.length}`,
      `Latest event: ${latest}`,
    ]
    if (mode !== 'view') return joinLines(header)
    return joinLines([...header, events.length ? 'Event log:' : 'Event log is empty.', ...events.map(eventSummary)])
  } catch (error) {
    return joinLines([
      `Collab ${role}: ${relayDescriptor(active)}`,
      `Relay URL: ${active.relayUrl || pathToFileURL(active.relayFile).href}`,
      `Relay read failed: ${error.message}`,
    ])
  }
}

async function statusForActiveRoles(collabState, mode) {
  const rows = []
  if (collabState.host) rows.push(await collabStatusText(collabState.host, 'host', mode))
  if (collabState.guest) rows.push(await collabStatusText(collabState.guest, 'guest', mode))
  return rows.join('\n\n')
}

export async function handleLocalCollab({ canonical, verb = 'status', relay, target, collabState, sessionPath, cwd, artifactDir, relayConfig = {} }) {
  try {
    const hydrated = await hydrateCollabState(cwd, collabState || {})
    collabState = hydrated.collabState
    const stateFile = hydrated.stateFile

    if (canonical === 'collab') {
      if (verb === 'stop') {
        const host = activeCollab(collabState.host)
        if (!host) {
          const guestNote = collabState.guest ? ' Guest attachment is still active; use /leave to detach it.' : ''
          return { text: `Collab hosting is already stopped.${guestNote}` }
        }
        await appendRelayEvent(host, 'host-stop', { sessionPath })
        collabState.host = null
        await savePersistedCollabState(cwd, collabState)
        return { text: joinLines([
          'Collab hosting stopped.',
          `Relay URL: ${host.relayUrl}`,
          host.relayFile ? `Durable relay file remains at ${host.relayFile}` : null,
          `State: ${stateFile}`,
        ]) }
      }
      if (verb === 'status' || verb === 'view') {
        if (collabState.host || collabState.guest) return { text: joinLines([await statusForActiveRoles(collabState, verb), `State: ${stateFile}`]) }
        return { text: joinLines([
          'Collab off.',
          'Use /collab start to create an active durable relay backend.',
          configRelayUrl(relayConfig) ? `Configured relay URL: ${configRelayUrl(relayConfig)}` : 'Backend fallback: durable local file relay in this session artifacts directory.',
          `State: ${stateFile}`,
        ]) }
      }
      if (verb === 'start') {
        const entry = startRelayEntry({ cwd, artifactDir, label: null, relay, relayConfig })
        await appendRelayEvent(entry, 'host-start', { sessionPath, cwd })
        collabState.host = { ...entry, startedAt: new Date().toISOString(), sessionPath, cwd }
        await savePersistedCollabState(cwd, collabState)
        return { text: joinLines([
          `Collab started with ${relayDescriptor(entry)}.`,
          `Join with: /join ${entry.relayUrl}`,
          entry.backend === 'http' ? 'Backend: configured HTTP durable relay.' : 'Backend: durable local file relay.',
          `State: ${stateFile}`,
        ]) }
      }
      return { error: 'Usage: /collab [start|status|view|stop] [relay-url-or-file-name]' }
    }

    if (canonical === 'join') {
      if (!target) return { error: 'Usage: /join <relay-url-or-path>' }
      const entry = joinRelayEntry(cwd, target)
      await appendRelayEvent(entry, 'guest-join', { sessionPath, cwd })
      collabState.guest = { ...entry, joinedAt: new Date().toISOString(), sessionPath, cwd }
      await savePersistedCollabState(cwd, collabState)
      return { text: joinLines([
        `Joined collab via ${relayDescriptor(entry)}.`,
        `Relay URL: ${entry.relayUrl}`,
        `State: ${stateFile}`,
      ]) }
    }

    if (canonical === 'leave') {
      const guest = activeCollab(collabState.guest)
      if (!guest) {
        const hostNote = collabState.host ? ' Hosting is still active; use /collab stop to stop the host relay.' : ''
        return { text: `No guest collab attachment is active.${hostNote}` }
      }
      await appendRelayEvent(guest, 'guest-leave', { sessionPath })
      collabState.guest = null
      await savePersistedCollabState(cwd, collabState)
      return { text: joinLines([
        'Left collab relay.',
        `Relay URL: ${guest.relayUrl}`,
        guest.relayFile ? `Durable relay file remains at ${guest.relayFile}` : null,
        `State: ${stateFile}`,
      ]) }
    }
    return null
  } catch (error) {
    return { error: error.message }
  }
}
