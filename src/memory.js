import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { dirname, resolve } from 'node:path'

const DEFAULT_MEMORY_LIMIT = 10
const MAX_MEMORY_TEXT = 2_000
const SECRET_PATTERNS = [
  /\b(?:sk|pk|rk)_[A-Za-z0-9_\-]{16,}\b/g,
  /\bgh[pousr]_[A-Za-z0-9_]{20,}\b/g,
  /\b[A-Za-z0-9+/]{32,}={0,2}\b/g,
  /((?:api[_-]?key|token|secret|password|authorization)\s*[:=]\s*)[^\s,;]+/gi,
]

export function memoryPath() {
  return process.env.JEDEN_MEMORY_FILE ? resolve(process.env.JEDEN_MEMORY_FILE) : resolve(homedir(), '.jeden', 'memory.jsonl')
}

export function memoryId() {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`
}

export function repoMemoryScope(cwd = process.cwd()) {
  return { kind: 'repo', id: resolve(cwd) }
}

export function normalizeMemoryScope(scope = null, cwd = process.cwd()) {
  if (!scope) return repoMemoryScope(cwd)
  if (typeof scope === 'string') return { kind: scope, id: scope === 'repo' ? resolve(cwd) : scope }
  const kind = String(scope.kind || 'repo')
  const id = scope.id == null ? (kind === 'repo' ? resolve(cwd) : kind) : String(scope.id)
  return { kind, id }
}

export function redactMemoryText(value) {
  let text = String(value ?? '')
  for (const pattern of SECRET_PATTERNS) text = text.replace(pattern, (match, prefix) => prefix ? `${prefix}[REDACTED]` : '[REDACTED]')
  return text
}

function clipText(text, max = MAX_MEMORY_TEXT) {
  const value = redactMemoryText(text).replace(/\s+/g, ' ').trim()
  return value.length > max ? `${value.slice(0, max - 1)}…` : value
}

function memoryTags(tags) {
  return Array.isArray(tags) ? [...new Set(tags.map((tag) => String(tag).trim()).filter(Boolean))] : []
}

export function createMemoryRecord({
  id = memoryId(),
  kind = 'note',
  scope = null,
  cwd = process.cwd(),
  text,
  tags = [],
  source = {},
  confidence = 0.5,
  status = 'active',
  createdAt = new Date().toISOString(),
  lastVerifiedAt = null,
  expiresAt = null,
  structured = null,
} = {}) {
  if (!text || typeof text !== 'string') throw new Error('text is required')
  const record = {
    id,
    kind: String(kind || 'note'),
    scope: normalizeMemoryScope(scope, cwd),
    text: clipText(text),
    tags: memoryTags(tags),
    source: source && typeof source === 'object' && !Array.isArray(source) ? source : {},
    confidence: Math.min(Math.max(Number(confidence) || 0, 0), 1),
    status: String(status || 'active'),
    createdAt,
  }
  if (lastVerifiedAt) record.lastVerifiedAt = String(lastVerifiedAt)
  if (expiresAt) record.expiresAt = String(expiresAt)
  if (structured && typeof structured === 'object' && !Array.isArray(structured)) record.structured = structured
  return record
}

function normalizeLegacyEntry(entry, cwd = process.cwd()) {
  if (entry?.scope && entry?.kind && entry?.source) return entry
  return createMemoryRecord({
    id: entry?.id || memoryId(),
    kind: 'note',
    scope: { kind: 'global', id: 'global' },
    cwd,
    text: String(entry?.text || ''),
    tags: entry?.tags || [],
    source: { origin: 'legacy_memory_tool' },
    confidence: 0.4,
    createdAt: entry?.createdAt || new Date().toISOString(),
  })
}

export async function loadMemoryRecords(file = memoryPath(), { cwd = process.cwd() } = {}) {
  try {
    const raw = await readFile(file, 'utf8')
    return raw.split(/\r?\n/).filter((line) => line.trim()).map((line) => normalizeLegacyEntry(JSON.parse(line), cwd))
  } catch (error) {
    if (error?.code === 'ENOENT') return []
    throw error
  }
}

export async function saveMemoryRecords(records, file = memoryPath()) {
  await mkdir(dirname(file), { recursive: true })
  const body = records.map((entry) => JSON.stringify(entry)).join('\n')
  await writeFile(file, body ? `${body}\n` : '', 'utf8')
}

export async function rememberMemory(input, { file = memoryPath(), cwd = process.cwd() } = {}) {
  const records = await loadMemoryRecords(file, { cwd })
  const record = createMemoryRecord({ ...input, cwd })
  records.push(record)
  await saveMemoryRecords(records, file)
  return record
}

function tokenize(value) {
  return String(value || '').toLowerCase().split(/[^a-z0-9ąćęłńóśźż_-]+/i).filter((part) => part.length > 1)
}

function isExpired(record, now = Date.now()) {
  return Boolean(record.expiresAt && Date.parse(record.expiresAt) <= now)
}

function scopeVisibleForRecall(recordScope, requestedScope) {
  if (!recordScope) return false
  if (recordScope.kind === 'global') return true
  if (!requestedScope) return true
  if (recordScope.kind !== requestedScope.kind) return false
  return String(recordScope.id) === String(requestedScope.id)
}

function scopeEquals(recordScope, requestedScope) {
  if (!recordScope || !requestedScope) return false
  return String(recordScope.kind) === String(requestedScope.kind) && String(recordScope.id) === String(requestedScope.id)
}

function kindAllowed(record, kinds) {
  return !Array.isArray(kinds) || kinds.length === 0 || kinds.includes(record.kind)
}

function memoryScore(record, query) {
  const terms = tokenize(query)
  if (terms.length === 0) return Number(record.confidence || 0)
  const haystack = tokenize(`${record.text} ${(record.tags || []).join(' ')} ${record.kind}`).join(' ')
  let matches = 0
  for (const term of terms) if (haystack.includes(term)) matches += 1
  if (matches === 0) return -1
  return matches / terms.length + Number(record.confidence || 0)
}

export function memoryMatchesQuery(record, query) {
  return memoryScore(record, query) >= 0
}

export async function recallMemories({
  query = '',
  cwd = process.cwd(),
  scope = repoMemoryScope(cwd),
  limit = DEFAULT_MEMORY_LIMIT,
  kinds = null,
  file = memoryPath(),
} = {}) {
  const requestedScope = normalizeMemoryScope(scope, cwd)
  const records = await loadMemoryRecords(file, { cwd })
  const now = Date.now()
  const matched = []
  for (const record of records) {
    if (record.status && record.status !== 'active') continue
    if (isExpired(record, now)) continue
    if (!scopeVisibleForRecall(record.scope, requestedScope)) continue
    if (!kindAllowed(record, kinds)) continue
    const score = memoryScore(record, query)
    if (score < 0) continue
    matched.push({ record, score })
  }
  matched.sort((a, b) => b.score - a.score || String(b.record.createdAt).localeCompare(String(a.record.createdAt)))
  return matched.slice(0, Math.min(Math.max(Number(limit) || DEFAULT_MEMORY_LIMIT, 1), 100)).map((item) => item.record)
}

export function formatMemoryContext(records) {
  if (!Array.isArray(records) || records.length === 0) return ''
  const lines = ['Durable memory (scoped, provenance-backed; current repository state and tool results override memory):']
  for (const record of records) {
    const confidence = Number(record.confidence || 0).toFixed(2)
    const source = record.source?.runId ? ` source:run:${record.source.runId}` : record.source?.origin ? ` source:${record.source.origin}` : ''
    lines.push(`- [${record.kind} confidence=${confidence}] ${record.text}${source}`)
  }
  return lines.join('\n')
}

export async function buildMemoryContext({ cwd = process.cwd(), task = '', limit = 6, file = memoryPath() } = {}) {
  const records = await recallMemories({
    cwd,
    query: task,
    limit,
    file,
    kinds: ['user_preference', 'project_fact', 'procedure', 'tool_failure', 'verification_pattern', 'anti_pattern', 'run_episode', 'note'],
  })
  return { records, text: formatMemoryContext(records) }
}

export async function learnFromCompletedRun({ cwd = process.cwd(), task, finalText, runId = null, sessionPath = null, verification = null, file = memoryPath() } = {}) {
  if (!task || !finalText) return null
  const verified = verification?.ok === true
  const text = `Completed run for task: ${task}. Final result: ${finalText}`
  return rememberMemory({
    kind: 'run_episode',
    scope: repoMemoryScope(cwd),
    text,
    tags: ['auto', 'run', verified ? 'verified' : 'unverified'],
    source: { origin: 'runJeden', runId, sessionPath, verified, verification: verification || null },
    confidence: verified ? 0.8 : 0.45,
    lastVerifiedAt: verified ? new Date().toISOString() : null,
  }, { file, cwd })
}

export function createLocalMemoryBackend({ file = memoryPath(), cwd = process.cwd() } = {}) {
  return {
    async remember(records) {
      const inputs = Array.isArray(records) ? records : [records]
      const saved = []
      for (const input of inputs) saved.push(await rememberMemory(input, { file, cwd }))
      return saved
    },
    async recall(query) {
      return recallMemories({ ...query, file, cwd: query?.cwd || cwd })
    },
    async improve(input) {
      if (input?.completedRun) return learnFromCompletedRun({ ...input.completedRun, file, cwd: input.completedRun.cwd || cwd })
      return null
    },
    async forget(scope = null) {
      if (!scope) {
        await saveMemoryRecords([], file)
        return { removed: 'all' }
      }
      const requestedScope = normalizeMemoryScope(scope, cwd)
      const records = await loadMemoryRecords(file, { cwd })
      const kept = records.filter((record) => !scopeEquals(record.scope, requestedScope))
      await saveMemoryRecords(kept, file)
      return { removed: records.length - kept.length }
    },
  }
}

export function createCogneeMemoryBackend({ client } = {}) {
  if (!client) throw new Error('Cognee backend requires an explicit client')
  return {
    async remember(records) {
      const inputs = Array.isArray(records) ? records : [records]
      return client.remember(inputs)
    },
    async recall(query) {
      return client.recall(query)
    },
    async improve(input) {
      if (typeof client.improve !== 'function') return null
      return client.improve(input)
    },
    async forget(scope) {
      if (typeof client.forget !== 'function') throw new Error('Cognee client does not expose forget')
      return client.forget(scope)
    },
  }
}
