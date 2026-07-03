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

export async function createEncryptedShareBundle({ session, artifactDir }) {
  const createdAt = new Date().toISOString()
  const key = randomBytes(32)
  const iv = randomBytes(12)
  const plain = Buffer.from(JSON.stringify({ version: 1, kind: 'jeden-session', createdAt, session }, null, 2), 'utf8')
  const cipher = createCipheriv('aes-256-gcm', key, iv)
  const ciphertext = Buffer.concat([cipher.update(plain), cipher.final()])
  const bundle = {
    version: 1,
    kind: 'jeden-local-encrypted-share',
    algorithm: 'AES-256-GCM',
    createdAt,
    sessionId: session.id,
    iv: iv.toString('base64url'),
    tag: cipher.getAuthTag().toString('base64url'),
    ciphertext: ciphertext.toString('base64url'),
    note: 'Local portable encrypted session bundle. The decryption key is carried only in the returned URL fragment; this file is not uploaded anywhere by Jeden.',
  }
  const file = join(artifactDir, `share-${safeArtifactName(session.id, 'session')}-${timestampSlug()}.jeden-share`)
  await mkdir(dirname(file), { recursive: true })
  await writeFile(file, `${JSON.stringify(bundle, null, 2)}\n`, 'utf8')
  const url = `${pathToFileURL(file).href}#key=${key.toString('base64url')}`
  return joinLines([
    `Encrypted local share bundle written to ${file}`,
    `File URL with decryption key: ${url}`,
    'Local-only: this is a portable encrypted file, not a cloud share or network relay. Keep the URL fragment/key private.',
  ])
}

function collabRelayArtifactPath(artifactDir, label) {
  const name = label ? safeArtifactName(label, 'collab-relay.jsonl') : `collab-relay-${timestampSlug()}.jsonl`
  return join(artifactDir, name)
}

function collabRelayInputPath(cwd, input) {
  try {
    const parsed = new URL(input)
    if (parsed.protocol === 'file:') return fileURLToPath(parsed)
  } catch {}
  return resolve(cwd, input)
}

async function appendCollabEvent(file, type, data = {}) {
  await mkdir(dirname(file), { recursive: true })
  await appendFile(file, `${JSON.stringify({ ts: new Date().toISOString(), type, ...data })}\n`, 'utf8')
}

async function readCollabEvents(file) {
  try {
    const text = await readFile(file, 'utf8')
    return text.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line))
  } catch (error) {
    if (error?.code === 'ENOENT') return []
    throw error
  }
}

function activeCollab(entry) {
  if (!entry) return null
  if (typeof entry === 'string') return { relayFile: entry }
  return entry
}

async function collabStatusText(entry, role) {
  const active = activeCollab(entry)
  if (!active?.relayFile) return 'Collab off.'
  const events = await readCollabEvents(active.relayFile)
  const latest = events.length ? `${events[events.length - 1].type} at ${events[events.length - 1].ts}` : 'none'
  return joinLines([
    `Local collab ${role}: ${active.relayFile}`,
    `Relay URL: ${pathToFileURL(active.relayFile).href}`,
    `Events: ${events.length}`,
    `Latest event: ${latest}`,
    'Local-only append-only relay file; no cloud or network relay is running.',
  ])
}

export async function handleLocalCollab({ canonical, verb = 'status', relay, target, collabState, sessionPath, cwd, artifactDir }) {
  if (canonical === 'collab') {
    if (verb === 'stop') {
      const host = activeCollab(collabState.host)
      if (!host?.relayFile) return { text: 'Collab hosting is already stopped.' }
      await appendCollabEvent(host.relayFile, 'host-stop', { sessionPath })
      collabState.host = null
      return { text: `Collab hosting stopped. Local relay file remains at ${host.relayFile}` }
    }
    if (verb === 'status' || verb === 'view') {
      if (collabState.host) return { text: await collabStatusText(collabState.host, 'host') }
      if (collabState.guest) return { text: await collabStatusText(collabState.guest, 'guest') }
      return { text: 'Collab off. Use /collab start to create a local append-only relay file in this session artifacts directory.' }
    }
    if (verb === 'start') {
      const relayFile = collabRelayArtifactPath(artifactDir, relay)
      await appendCollabEvent(relayFile, 'host-start', { sessionPath, cwd })
      collabState.host = { relayFile, startedAt: new Date().toISOString() }
      const url = pathToFileURL(relayFile).href
      return { text: joinLines([
        `Local collab started with append-only relay file: ${relayFile}`,
        `Join locally with: /join ${url}`,
        'Local-only: no cloud service, websocket, or OMP relay is running.',
      ]) }
    }
    return { error: 'Usage: /collab [start|status|view|stop] [relay-file-name]' }
  }
  if (canonical === 'join') {
    if (!target) return { error: 'Usage: /join <file-url-or-path>' }
    const relayFile = collabRelayInputPath(cwd, target)
    await appendCollabEvent(relayFile, 'guest-join', { sessionPath, cwd })
    collabState.guest = { relayFile, joinedAt: new Date().toISOString() }
    return { text: joinLines([
      `Joined local collab relay file: ${relayFile}`,
      'Local-only: this attaches to an append-only file on this machine or mounted filesystem; no network relay is contacted.',
    ]) }
  }
  if (canonical === 'leave') {
    const guest = activeCollab(collabState.guest)
    if (!guest?.relayFile) return { text: 'No guest collab attachment is active.' }
    await appendCollabEvent(guest.relayFile, 'guest-leave', { sessionPath })
    collabState.guest = null
    return { text: `Left local collab relay file: ${guest.relayFile}` }
  }
  return null
}
