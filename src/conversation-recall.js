import { readdir, readFile, stat } from 'node:fs/promises'
import { homedir, userInfo } from 'node:os'
import { join } from 'node:path'

const SYSTEM_NOISE = /^(<system-reminder>|<\/system-reminder>|<command-message>|<\/command-message>|<command-name>|<\/command-name>|<command-args>|<\/command-args>|<local-command-stdout>|<\/local-command-stdout>|<local-command-caveat>|<\/local-command-caveat>)/
const HOOK_NOISE = /^(Stop hook (feedback|blocking error)|\[python3 \$HOME\/\.shared-hooks\/|BLOCKED|Bypass:)/
const TASK_NOISE = /^(<task-notification>|<\/task-notification>|<task-id>|<\/task-id>|<tool-use-id>|<\/tool-use-id>|<output-file>|<\/output-file>|<status>|<\/status>|<summary>|<\/summary>|<event>|<\/event>)/
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']

function modeString(mode) {
  const type = (mode & 0o170000) === 0o040000 ? 'd' : '-'
  const bits = [
    0o400, 0o200, 0o100,
    0o040, 0o020, 0o010,
    0o004, 0o002, 0o001,
  ]
  const chars = ['r', 'w', 'x', 'r', 'w', 'x', 'r', 'w', 'x']
  return type + bits.map((bit, index) => (mode & bit) ? chars[index] : '-').join('')
}

function listDate(date) {
  const now = Date.now()
  const mtime = date.getTime()
  const month = MONTHS[date.getMonth()]
  const day = String(date.getDate()).padStart(2, ' ')
  const old = Math.abs(now - mtime) > 180 * 24 * 60 * 60 * 1000
  if (old) return `${month} ${day}  ${date.getFullYear()}`
  return `${month} ${day} ${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`
}

function ownerName(uid) {
  try {
    const current = userInfo()
    if (current.uid === uid) return current.username
  } catch {}
  return String(uid)
}

function formatLongListEntry(entry) {
  const group = String(entry.gid)
  return `${modeString(entry.mode)} ${String(entry.nlink).padStart(2, ' ')} ${ownerName(entry.uid)} ${group} ${String(entry.size).padStart(8, ' ')} ${listDate(entry.mtime)} ${entry.path}`
}


export function claudeProjectPath({ cwd = process.cwd(), home = homedir() } = {}) {
  const encoded = String(cwd).replaceAll('/', '-')
  return join(home, '.claude', 'projects', encoded)
}

export async function listConversationJsonls({ cwd = process.cwd(), home = homedir(), limit = 10 } = {}) {
  const projectDir = claudeProjectPath({ cwd, home })
  let names
  try {
    names = await readdir(projectDir)
  } catch (error) {
    if (error?.code === 'ENOENT') throw new Error(`no project dir at ${projectDir}`)
    throw error
  }
  const entries = []
  for (const name of names) {
    if (!name.endsWith('.jsonl')) continue
    const path = join(projectDir, name)
    const info = await stat(path)
    entries.push({
      path,
      name,
      mtimeMs: info.mtimeMs,
      size: info.size,
      mode: info.mode,
      nlink: info.nlink,
      uid: info.uid,
      gid: info.gid,
      mtime: info.mtime,
    })
  }
  entries.sort((a, b) => b.mtimeMs - a.mtimeMs || a.name.localeCompare(b.name))
  return entries.slice(0, Math.min(Math.max(Number(limit) || 10, 1), 100))
}

export function formatConversationList(entries) {
  return entries.map((entry) => formatLongListEntry(entry)).join('\n') + (entries.length > 0 ? '\n' : '')
}

export async function resolveConversationJsonl({ cwd = process.cwd(), home = homedir(), session = null } = {}) {
  const projectDir = claudeProjectPath({ cwd, home })
  if (session) {
    const raw = String(session)
    const first = join(projectDir, `${raw}.jsonl`)
    try {
      await stat(first)
      return first
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
    }
    const second = join(projectDir, raw)
    try {
      await stat(second)
      return second
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
      throw new Error(`no JSONL found at ${first} or ${second}`)
    }
  }
  const entries = await listConversationJsonls({ cwd, home, limit: 1 })
  if (entries.length === 0) throw new Error('no JSONL found')
  return entries[0].path
}

function messageText(content) {
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return ''
  return content.map((part) => {
    if (part?.type === 'text') return String(part.text || '')
    if (part?.type === 'tool_use') return ''
    if (part?.type === 'tool_result') return ''
    if (part?.type === 'image') return ''
    return ''
  }).filter(Boolean).join('\n')
}

function visibleLine(line) {
  return !SYSTEM_NOISE.test(line) && !HOOK_NOISE.test(line) && !TASK_NOISE.test(line)
}

function cleanBlock(role, text) {
  const body = String(text || '').split(/\r?\n/).filter((line, index) => {
    if (index === 0 && line.trim() === '') return false
    return visibleLine(line)
  }).join('\n').trim()
  if (!body) return ''
  return `[${role}]\n${body}`
}

export function recallConversationFromJsonl(raw, { jsonlPath = null } = {}) {
  const blocks = []
  for (const line of String(raw || '').split(/\r?\n/)) {
    if (!line.trim()) continue
    let event
    try {
      event = JSON.parse(line)
    } catch {
      continue
    }
    if (event.type !== 'user' && event.type !== 'assistant') continue
    const role = String(event.type).toUpperCase()
    const block = cleanBlock(role, messageText(event.message?.content))
    if (block) blocks.push(block)
  }
  const header = jsonlPath ? `# Conversation transcript: ${jsonlPath}\n# (text-only, no tool_use / tool_result / images / hooks)\n\n` : ''
  return `${header}${blocks.join('\n\n')}${blocks.length > 0 ? '\n' : ''}`
}

export async function recallConversation({ cwd = process.cwd(), home = homedir(), session = null } = {}) {
  const jsonlPath = await resolveConversationJsonl({ cwd, home, session })
  const raw = await readFile(jsonlPath, 'utf8')
  return recallConversationFromJsonl(raw, { jsonlPath })
}
