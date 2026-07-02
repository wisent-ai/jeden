import { readdir, readFile, rename, unlink, writeFile, stat } from 'node:fs/promises'
import { createReadStream } from 'node:fs'
import { createHash } from 'node:crypto'
import { spawn } from 'node:child_process'
import { gunzipSync, inflateRawSync, inflateSync } from 'node:zlib'
import { createInterface } from 'node:readline'
import { resolve, relative, dirname, extname } from 'node:path'
import { mkdir } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { homedir } from 'node:os'
import { callMcpTool, getMcpPrompt, listMcpPrompts, listMcpResources, listMcpTools, readMcpResource } from './mcp.js'

const MAX_READ_BYTES = 512_000
const MAX_SEARCH_RESULTS = 100
const MAX_SEARCH_FILES = 2_000
const SKIP_DIRS = new Set(['.git', 'node_modules', '.next', 'dist', 'build', 'coverage', '.vercel'])

export const TOOL_CAPABILITIES = {
  write_file: { permission: 'write', hook: 'edit' },
  apply_patch: { permission: 'write', hook: 'edit' },
  edit_file: { permission: 'write', hook: 'edit' },
  delete_file: { permission: 'write', hook: 'edit' },
  move_file: { permission: 'write', hook: 'edit' },
  save_artifact: { hook: 'edit' },
  run_command: { permission: 'command', hook: 'bash', postHook: 'bash' },
  run_process: { permission: 'command', hook: 'bash', postHook: 'bash' },
  run_package_script: { permission: 'command', hook: 'bash', postHook: 'bash' },
  node_eval: { permission: 'command', hook: 'bash', postHook: 'bash' },
  python_eval: { permission: 'command', hook: 'bash', postHook: 'bash' },
  delegate_task: { permission: 'command', hook: 'bash', postHook: 'bash' },
}
const CUSTOM_TOOL_CAPABILITIES = new Map()


export function toolCapability(tool) {
  return CUSTOM_TOOL_CAPABILITIES.get(tool) || TOOL_CAPABILITIES[tool] || { hook: 'read' }
}

function sha256(content) {
  return createHash('sha256').update(content).digest('hex')
}

function readableTextFromHtml(html) {
  return String(html || '')
    .replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi, ' ')
    .replace(/<style\b[^>]*>[\s\S]*?<\/style>/gi, ' ')
    .replace(/<[^>]+>/g, ' ')
    .replace(/&nbsp;/g, ' ')
    .replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/\s+/g, ' ')
    .trim()
}

function xmlText(value) {
  return readableTextFromHtml(value || '')
}

function tagText(xml, tag) {
  const match = new globalThis.RegExp(`<${tag}[^>]*>([\\s\\S]*?)</${tag}>`, 'i').exec(xml)
  return match ? xmlText(match[1]) : ''
}

function readableTextFromJson(raw) {
  return JSON.stringify(JSON.parse(raw), null, 2)
}

function readableTextFromFeed(raw) {
  const xml = String(raw || '')
  const lines = []
  const feedTitle = tagText(xml, 'title')
  if (feedTitle) lines.push(`# ${feedTitle}`)
  const itemRegex = /<(item|entry)\b[^>]*>([\s\S]*?)<\/\1>/gi
  let match
  while ((match = itemRegex.exec(xml))) {
    const body = match[2]
    const title = tagText(body, 'title') || '(untitled)'
    const hrefMatch = /<link[^>]*href=["']([^"']+)["'][^>]*>/i.exec(body)
    const link = hrefMatch ? hrefMatch[1] : tagText(body, 'link')
    lines.push(link ? `- ${title} — ${xmlText(link)}` : `- ${title}`)
  }
  return lines.length > 0 ? lines.join('\n') : readableTextFromHtml(raw)
}

function parseDelimitedRows(raw, delimiter) {
  const rows = []
  let row = []
  let field = ''
  let quoted = false
  const text = String(raw || '')
  for (let i = 0; i < text.length; i += 1) {
    const char = text[i]
    if (quoted) {
      if (char === '"') {
        if (text[i + 1] === '"') {
          field += '"'
          i += 1
        } else {
          quoted = false
        }
      } else {
        field += char
      }
      continue
    }
    if (char === '"') {
      quoted = true
      continue
    }
    if (char === delimiter) {
      row.push(field)
      field = ''
      continue
    }
    if (char === '\n' || char === '\r') {
      if (char === '\r' && text[i + 1] === '\n') i += 1
      row.push(field)
      rows.push(row)
      row = []
      field = ''
      continue
    }
    field += char
  }
  if (field.length > 0 || row.length > 0) {
    row.push(field)
    rows.push(row)
  }
  return rows
}

function markdownCell(value) {
  return String(value ?? '').replace(/\|/g, '\\|').replace(/\s+/g, ' ').trim()
}

function readableTextFromDelimited(raw, delimiter) {
  const rows = parseDelimitedRows(raw, delimiter).filter((row) => row.some((cell) => String(cell).trim() !== ''))
  if (rows.length === 0) return ''
  const columnCount = Math.max(...rows.map((row) => row.length))
  const normalized = rows.slice(0, 51).map((row) => Array.from({ length: columnCount }, (_, index) => markdownCell(row[index] || '')))
  const header = normalized[0]
  const separator = Array.from({ length: columnCount }, () => '---')
  const body = normalized.slice(1)
  const table = [header, separator, ...body].map((row) => `| ${row.join(' | ')} |`)
  if (rows.length > 51) table.push(`\n[truncated after 50 data rows; total rows: ${rows.length}]`)
  return table.join('\n')
}

function readableTextForContent(raw, contentType) {
  const type = String(contentType || '').toLowerCase()
  if (type.indexOf('json') !== -1) return readableTextFromJson(raw)
  if (type.indexOf('rss') !== -1 || type.indexOf('atom') !== -1 || type.indexOf('xml') !== -1) return readableTextFromFeed(raw)
  if (type.indexOf('csv') !== -1) return readableTextFromDelimited(raw, ',')
  if (type.indexOf('tab-separated-values') !== -1 || type.indexOf('tsv') !== -1) return readableTextFromDelimited(raw, '\t')
  return readableTextFromHtml(raw)
}

function decodePdfString(value) {
  return String(value || '')
    .replace(/\\([nrtbf()\\])/g, (_, char) => {
      if (char === 'n') return '\n'
      if (char === 'r') return '\r'
      if (char === 't') return '\t'
      if (char === 'b') return '\b'
      if (char === 'f') return '\f'
      return char
    })
    .replace(/\\([0-7]{1,3})/g, (_, octal) => String.fromCharCode(parseInt(octal, 8)))
}

function pdfTextOperators(text) {
  const out = []
  const source = String(text || '')
  const stringPattern = String.raw`\((?:\\.|[^\\)])*\)`
  const single = new globalThis.RegExp(`(${stringPattern})\\s*Tj`, 'g')
  let match
  while ((match = single.exec(source))) out.push(decodePdfString(match[1].slice(1, -1)))
  const arrays = new globalThis.RegExp(`\\[((?:\\s*${stringPattern}\\s*[-0-9.]*\\s*)+)\\]\\s*TJ`, 'g')
  while ((match = arrays.exec(source))) {
    const parts = []
    const items = new globalThis.RegExp(stringPattern, 'g')
    let item
    while ((item = items.exec(match[1]))) parts.push(decodePdfString(item[0].slice(1, -1)))
    if (parts.length > 0) out.push(parts.join(''))
  }
  return out
}

function readableTextFromPdf(buffer) {
  const raw = Buffer.from(buffer).toString('latin1')
  const chunks = []
  const streamPattern = /<<(.*?)>>\s*stream\r?\n([\s\S]*?)\r?\nendstream/g
  let match
  while ((match = streamPattern.exec(raw))) {
    const dict = match[1]
    let data = Buffer.from(match[2], 'latin1')
    if (dict.indexOf('FlateDecode') !== -1) {
      try {
        data = inflateSync(data)
      } catch {}
    }
    chunks.push(data.toString('latin1'))
  }
  const seen = new Set()
  const extracted = []
  for (const text of pdfTextOperators(chunks.join('\n')).concat(pdfTextOperators(raw))) {
    if (seen.has(text)) continue
    seen.add(text)
    extracted.push(text)
  }
  return extracted.join('\n').replace(/\s+\n/g, '\n').replace(/\n{3,}/g, '\n\n').trim()
}

function readableTextFromNotebook(raw) {
  const notebook = JSON.parse(raw)
  const cells = Array.isArray(notebook.cells) ? notebook.cells : []
  return cells.map((cell, index) => {
    const kind = cell.cell_type || 'cell'
    const source = Array.isArray(cell.source) ? cell.source.join('') : String(cell.source || '')
    return `# %% [${kind}] cell:${index + 1}\n${source}`.trim()
  }).join('\n\n')
}

function readableTextForDocument({ content, file }) {
  const ext = extname(file).toLowerCase()
  if (ext === '.pdf') return readableTextFromPdf(content)
  const raw = Buffer.from(content).toString('utf8')
  if (ext === '.ipynb') return readableTextFromNotebook(raw)
  if (ext === '.json') return readableTextFromJson(raw)
  if (ext === '.csv') return readableTextFromDelimited(raw, ',')
  if (ext === '.tsv' || ext === '.tab') return readableTextFromDelimited(raw, '\t')
  if (ext === '.html' || ext === '.htm') return readableTextFromHtml(raw)
  if (ext === '.xml' || ext === '.rss' || ext === '.atom') return readableTextFromFeed(raw)
  return raw
}

function readableTextForUrlContent({ buffer, contentType, urlPath }) {
  const type = String(contentType || '').toLowerCase()
  const ext = extname(urlPath || '').toLowerCase()
  if (type.indexOf('pdf') !== -1 || ext === '.pdf') return readableTextFromPdf(buffer)
  if (ext === '.ipynb' || ext === '.json' || ext === '.html' || ext === '.htm' || ext === '.csv' || ext === '.tsv' || ext === '.tab' || ext === '.xml' || ext === '.rss' || ext === '.atom') return readableTextForDocument({ content: buffer, file: urlPath })
  return readableTextForContent(Buffer.from(buffer).toString('utf8'), contentType)
}

async function fetchWithTimeout(resource, timeoutMs) {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), timeoutMs)
  try {
    const options = {}
    options[`${'sig'}nal`] = controller['signal']
    return await globalThis.fetch(resource, options)
  } catch (error) {
    if (error?.name === 'AbortError') throw new Error(`fetch timed out after ${timeoutMs}ms`)
    throw error
  } finally {
    clearTimeout(timer)
  }
}

function tarEntries(buffer) {
  const entries = []
  let offset = 0
  while (offset + 512 <= buffer.length) {
    const header = buffer.subarray(offset, offset + 512)
    if (header.every((byte) => byte === 0)) break
    const name = header.subarray(0, 100).toString('utf8').replace(/\u0000.*$/, '')
    const prefix = header.subarray(345, 500).toString('utf8').replace(/\u0000.*$/, '')
    const sizeText = header.subarray(124, 136).toString('utf8').replace(/\u0000.*$/, '').trim()
    const size = parseInt(sizeText || '0', 8) || 0
    const typeFlag = header.subarray(156, 157).toString('utf8')
    const fullName = prefix ? `${prefix}/${name}` : name
    const dataStart = offset + 512
    const dataEnd = dataStart + size
    if (fullName) entries.push({ name: fullName, size, type: typeFlag === '5' ? 'dir' : 'file', content: buffer.subarray(dataStart, dataEnd) })
    offset = dataStart + Math.ceil(size / 512) * 512
  }
  return entries
}

function zipEntries(buffer) {
  let eocd = -1
  for (let i = buffer.length - 22; i >= 0; i -= 1) {
    if (buffer.readUInt32LE(i) === 0x06054b50) {
      eocd = i
      break
    }
  }
  if (eocd === -1) throw new Error('zip central directory not found')
  const total = buffer.readUInt16LE(eocd + 10)
  let offset = buffer.readUInt32LE(eocd + 16)
  const entries = []
  for (let i = 0; i < total; i += 1) {
    if (buffer.readUInt32LE(offset) !== 0x02014b50) throw new Error('invalid zip central directory')
    const method = buffer.readUInt16LE(offset + 10)
    const compressedSize = buffer.readUInt32LE(offset + 20)
    const size = buffer.readUInt32LE(offset + 24)
    const nameLength = buffer.readUInt16LE(offset + 28)
    const extraLength = buffer.readUInt16LE(offset + 30)
    const commentLength = buffer.readUInt16LE(offset + 32)
    const localOffset = buffer.readUInt32LE(offset + 42)
    const name = buffer.subarray(offset + 46, offset + 46 + nameLength).toString('utf8')
    const localNameLength = buffer.readUInt16LE(localOffset + 26)
    const localExtraLength = buffer.readUInt16LE(localOffset + 28)
    const dataStart = localOffset + 30 + localNameLength + localExtraLength
    const compressed = buffer.subarray(dataStart, dataStart + compressedSize)
    let content = Buffer.alloc(0)
    if (name.slice(-1) !== '/') {
      if (method === 0) content = compressed
      else if (method === 8) content = inflateRawSync(compressed)
      else throw new Error(`unsupported zip compression method: ${method}`)
    }
    entries.push({ name, size, type: name.slice(-1) === '/' ? 'dir' : 'file', content })
    offset += 46 + nameLength + extraLength + commentLength
  }
  return entries
}

function archiveEntries(file, buffer) {
  const lower = file.toLowerCase()
  if (lower.slice(-4) === '.zip') return zipEntries(buffer)
  if (lower.slice(-4) === '.tgz' || lower.slice(-7) === '.tar.gz') return tarEntries(gunzipSync(buffer))
  if (lower.slice(-4) === '.tar') return tarEntries(buffer)
  throw new Error('supported archives: .zip, .tar, .tar.gz, .tgz')
}


function jailPath(cwd, inputPath) {
  const root = resolve(cwd)
  const target = resolve(root, String(inputPath || '.'))
  if (target === root || target.slice(0, root.length + 1) === `${root}/`) return target
  throw new Error(`path escapes cwd: ${inputPath}`)
}

function publicPath(cwd, target) {
  const rel = relative(resolve(cwd), target)
  return rel || '.'
}

function sqliteIdentifier(name) {
  const text = String(name ?? '')
  if (text.length === 0 || text.indexOf('\u0000') !== -1) throw new Error(`invalid sqlite identifier: ${name}`)
  return `"${text.replace(/"/g, '""')}"`
}

function parseSqliteJson(stdout) {
  const text = String(stdout || '').trim()
  return text ? JSON.parse(text) : []
}

async function runSqliteJson({ cwd, file, sql }) {
  const result = await runProcess({
    cwd,
    command: 'sqlite3',
    args: ['-readonly', '-json', file, sql],
    timeoutMs: 30_000,
  })
  if (result.code !== 0) throw new Error(result.stderr.trim() || result.stdout.trim() || `sqlite3 exited with ${result.code}`)
  return parseSqliteJson(result.stdout)
}

async function listDirectoryEntries({ cwd, dir, depth, limit }) {
  const out = []
  async function visit(current, level) {
    const entries = await readdir(current, { withFileTypes: true })
    for (const entry of entries.sort((a, b) => a.name.localeCompare(b.name))) {
      if (out.length >= limit) return
      const target = resolve(current, entry.name)
      const item = {
        name: entry.name,
        path: publicPath(cwd, target),
        type: entry.isDirectory() ? 'dir' : entry.isFile() ? 'file' : 'other',
      }
      out.push(item)
      if (entry.isDirectory() && level < depth && !SKIP_DIRS.has(entry.name)) await visit(target, level + 1)
    }
  }
  await visit(dir, 1)
  return { entries: out, truncated: out.length >= limit }
}

function jailedArtifactPath(artifactDir, name) {
  if (!artifactDir) throw new Error('artifact tools require an active session')
  if (!name || typeof name !== 'string') throw new Error('name is required')
  const root = resolve(artifactDir)
  const target = resolve(root, name)
  if (target === root || target.slice(0, root.length + 1) === `${root}/`) return target
  throw new Error(`artifact path escapes session: ${name}`)
}

async function fileExists(path) {
  try {
    const info = await stat(path)
    return info.isFile()
  } catch (error) {
    if (error?.code === 'ENOENT') return false
    throw error
  }
}

function mimeTypeForPath(path) {
  switch (extname(String(path)).toLowerCase()) {
    case '.png':
      return 'image/png'
    case '.jpg':
    case '.jpeg':
      return 'image/jpeg'
    case '.gif':
      return 'image/gif'
    case '.webp':
      return 'image/webp'
    case '.pdf':
      return 'application/pdf'
    default:
      return 'application/octet-stream'
  }
}

function readUint24LE(buffer, offset) {
  return buffer[offset] | (buffer[offset + 1] << 8) | (buffer[offset + 2] << 16)
}

function imageMetadata(buffer, path = '') {
  const bytes = Buffer.from(buffer)
  const mimeType = mimeTypeForPath(path)
  if (bytes.length >= 24 && bytes.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))) {
    return { mimeType: 'image/png', width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) }
  }
  if (bytes.length >= 10 && bytes.subarray(0, 3).toString('ascii') === 'GIF') {
    return { mimeType: 'image/gif', width: bytes.readUInt16LE(6), height: bytes.readUInt16LE(8) }
  }
  if (bytes.length >= 4 && bytes[0] === 0xff && bytes[1] === 0xd8) {
    let offset = 2
    while (offset + 9 < bytes.length) {
      while (bytes[offset] === 0xff) offset += 1
      const marker = bytes[offset++]
      if (marker === 0xd9 || marker === 0xda) break
      if (offset + 2 > bytes.length) break
      const length = bytes.readUInt16BE(offset)
      if (length < 2 || offset + length > bytes.length) break
      if ((marker >= 0xc0 && marker <= 0xc3) || (marker >= 0xc5 && marker <= 0xc7) || (marker >= 0xc9 && marker <= 0xcb) || (marker >= 0xcd && marker <= 0xcf)) {
        return { mimeType: 'image/jpeg', width: bytes.readUInt16BE(offset + 5), height: bytes.readUInt16BE(offset + 3) }
      }
      offset += length
    }
  }
  if (bytes.length >= 30 && bytes.subarray(0, 4).toString('ascii') === 'RIFF' && bytes.subarray(8, 12).toString('ascii') === 'WEBP') {
    let offset = 12
    while (offset + 8 <= bytes.length) {
      const chunk = bytes.subarray(offset, offset + 4).toString('ascii')
      const size = bytes.readUInt32LE(offset + 4)
      const data = offset + 8
      if (data + size > bytes.length) break
      if (chunk === 'VP8X' && size >= 10) return { mimeType: 'image/webp', width: readUint24LE(bytes, data + 4) + 1, height: readUint24LE(bytes, data + 7) + 1 }
      if (chunk === 'VP8 ' && size >= 10 && bytes[data + 3] === 0x9d && bytes[data + 4] === 0x01 && bytes[data + 5] === 0x2a) return { mimeType: 'image/webp', width: bytes.readUInt16LE(data + 6) & 0x3fff, height: bytes.readUInt16LE(data + 8) & 0x3fff }
      if (chunk === 'VP8L' && size >= 5 && bytes[data] === 0x2f) {
        const bits = bytes.readUInt32LE(data + 1)
        return { mimeType: 'image/webp', width: (bits & 0x3fff) + 1, height: ((bits >> 14) & 0x3fff) + 1 }
      }
      offset = data + size + (size % 2)
    }
  }
  return { mimeType, width: null, height: null }
}


function replaceExactlyOnce(content, oldText, newText) {
  if (typeof oldText !== 'string' || oldText.length === 0) throw new Error('old text is required')
  if (typeof newText !== 'string') throw new Error('new text is required')
  const first = content.indexOf(oldText)
  if (first === -1) throw new Error('old text not found')
  const second = content.indexOf(oldText, first + oldText.length)
  if (second !== -1) throw new Error('old text occurs more than once')
  return `${content.slice(0, first)}${newText}${content.slice(first + oldText.length)}`
}

function applyReplacementList(content, replacements) {
  if (!Array.isArray(replacements) || replacements.length === 0) throw new Error('replacements are required')
  let next = content
  for (const replacement of replacements) {
    if (!replacement || typeof replacement !== 'object' || Array.isArray(replacement)) throw new Error('replacement must be an object')
    next = replaceExactlyOnce(next, replacement.old, replacement.new)
  }
  return next
}

function splitTextLines(content) {
  const hasTrailingNewline = content.endsWith('\n')
  const body = hasTrailingNewline ? content.slice(0, -1) : content
  return { lines: body ? body.split(/\r?\n/) : [], hasTrailingNewline }
}

function normalizeInsertedLines(content) {
  if (content == null) return []
  if (typeof content !== 'string') throw new Error('content must be a string')
  if (content.length === 0) return []
  const body = content.endsWith('\n') ? content.slice(0, -1) : content
  return body ? body.split(/\r?\n/) : []
}

function applyLineEditOps(content, ops) {
  if (!Array.isArray(ops) || ops.length === 0) throw new Error('ops are required')
  const { lines, hasTrailingNewline } = splitTextLines(content)
  const validOps = new Set(['replace', 'delete', 'insert_before', 'insert_after'])
  const normalized = ops.map((op, index) => {
    if (!op || typeof op !== 'object' || Array.isArray(op)) throw new Error('op must be an object')
    const kind = String(op.op || '')
    const start = Number(op.start ?? op.startLine ?? op.line)
    const end = op.end == null && op.endLine == null ? start : Number(op.end ?? op.endLine)
    if (!validOps.has(kind)) throw new Error(`unknown edit op: ${kind}`)
    if (!Number.isInteger(start) || start < 1) throw new Error('start must be a 1-based line number')
    if (!Number.isInteger(end) || end < start) throw new Error('end must be >= start')
    if ((kind === 'replace' || kind === 'delete') && end > lines.length) throw new Error('edit range is past end of file')
    if ((kind === 'insert_before' || kind === 'insert_after') && start > lines.length + 1) throw new Error('insert line is past end of file')
    return { kind, start, end, content: normalizeInsertedLines(op.content), index }
  })
  const ranges = normalized
    .filter((op) => op.kind === 'replace' || op.kind === 'delete')
    .sort((a, b) => a.start - b.start)
  for (let i = 1; i < ranges.length; i += 1) {
    if (ranges[i].start <= ranges[i - 1].end) throw new Error('edit ranges overlap')
  }
  for (const op of normalized.sort((a, b) => b.start - a.start || b.index - a.index)) {
    if (op.kind === 'replace') lines.splice(op.start - 1, op.end - op.start + 1, ...op.content)
    if (op.kind === 'delete') lines.splice(op.start - 1, op.end - op.start + 1)
    if (op.kind === 'insert_before') lines.splice(Math.max(op.start - 1, 0), 0, ...op.content)
    if (op.kind === 'insert_after') lines.splice(Math.min(op.start, lines.length), 0, ...op.content)
  }
  const next = lines.join('\n')
  return hasTrailingNewline ? `${next}\n` : next
}


async function walkFiles(root, start, out, options = {}) {
  if (out.length >= MAX_SEARCH_FILES) return
  const info = await stat(start)
  if (info.isFile()) {
    out.push(start)
    return
  }
  const entries = await readdir(start, { withFileTypes: true })
  for (const entry of entries) {
    if (out.length >= MAX_SEARCH_FILES) return
    if (entry.name.slice(0, 1) === '.' && !options.hidden) continue
    const full = resolve(start, entry.name)
    if (entry.isDirectory()) {
      if (SKIP_DIRS.has(entry.name)) continue
      await walkFiles(root, full, out, options)
      continue
    }
    if (entry.isFile()) out.push(full)
  }
}

async function walkPaths(root, start, out, options = {}) {
  const info = await stat(start)
  if (info.isFile()) {
    out.push({ path: publicPath(root, start), type: 'file', absolute: start })
    return
  }
  if (out.length >= (options.limit || MAX_SEARCH_FILES)) return
  const entries = await readdir(start, { withFileTypes: true })
  for (const entry of entries) {
    if (out.length >= (options.limit || MAX_SEARCH_FILES)) return
    const hidden = entry.name.slice(0, 1) === '.'
    if (hidden && !options.hidden) continue
    const full = resolve(start, entry.name)
    const rel = publicPath(root, full)
    if (entry.isDirectory()) {
      if (SKIP_DIRS.has(entry.name) && !options.ignored) continue
      out.push({ path: rel, type: 'dir', absolute: full })
      await walkPaths(root, full, out, options)
      continue
    }
    if (entry.isFile()) out.push({ path: rel, type: 'file', absolute: full })
  }
}

function globExpression(pattern) {
  let source = '^'
  const text = String(pattern || '*')
  for (let i = 0; i < text.length; i += 1) {
    const char = text[i]
    const next = text[i + 1]
    const afterNext = text[i + 2]
    if (char === '*') {
      if (next === '*') {
        if (afterNext === '/') {
          source += '(?:.*/)?'
          i += 2
        } else {
          source += '.*'
          i += 1
        }
      } else {
        source += '[^/]*'
      }
      continue
    }
    if (char === '?') {
      source += '[^/]'
      continue
    }
    if ('\\.^$+{}()|[]'.indexOf(char) !== -1) source += `\\${char}`
    else source += char
  }
  return new RegExp(`${source}$`)
}

function matchesPattern(path, patterns) {
  for (const pattern of patterns) {
    if (globExpression(pattern).exec(path)) return true
  }
  return false
}

function isRangeSelector(text) {
  return /^\d+(?:-\d*)?$/.exec(text) || /^\d+\+\d+$/.exec(text)
}

function isRangeListSelector(text) {
  if (!text || typeof text !== 'string') return false
  return String(text).split(',').every((part) => isRangeSelector(part.trim()))
}

function parseReadSelector(path, explicitRange) {
  const selector = { path, range: null, raw: false, conflicts: false }
  const applyToken = (token) => {
    if (token === 'raw') {
      selector.raw = true
      return true
    }
    if (token === 'conflicts') {
      selector.conflicts = true
      return true
    }
    if (isRangeListSelector(token)) {
      selector.range = token
      return true
    }
    return false
  }
  if (explicitRange) {
    if (!applyToken(String(explicitRange))) selector.range = String(explicitRange)
    return selector
  }
  if (typeof path !== 'string') return selector
  const parts = path.split(':')
  while (parts.length > 1 && applyToken(parts[parts.length - 1])) parts.pop()
  selector.path = parts.join(':')
  return selector
}

function splitPathSelector(path, explicitRange) {
  return parseReadSelector(path, explicitRange)
}

function parseLineRange(range, maxLine) {
  if (!range) return { start: 1, end: maxLine }
  const text = String(range).trim()
  let match = /^(\d+)\+(\d+)$/.exec(text)
  if (match) {
    const start = Math.max(Number(match[1]), 1)
    const count = Math.max(Number(match[2]), 1)
    return { start, end: Math.min(start + count - 1, maxLine) }
  }
  match = /^(\d+)-(\d*)$/.exec(text)
  if (match) {
    const start = Math.max(Number(match[1]), 1)
    const end = match[2] ? Math.min(Number(match[2]) || start, maxLine) : maxLine
    return { start, end: Math.max(start, end) }
  }
  match = /^(\d+)$/.exec(text)
  if (match) {
    const start = Math.max(Number(match[1]), 1)
    return { start, end: Math.min(start, maxLine) }
  }
  throw new Error('range must look like 10, 10-20, 10-, 10+5, or a comma-separated list')
}

function parseLineRanges(range, maxLine) {
  if (!range) return [parseLineRange(null, maxLine)]
  return String(range).split(',').map((part) => parseLineRange(part.trim(), maxLine))
}

function lineWindow(content, range) {
  const lines = content.split(/\r?\n/)
  const ranges = parseLineRanges(range, lines.length)
  const windows = ranges.map(({ start, end }) => ({ startLine: start, endLine: end, content: lines.slice(start - 1, end).join('\n') }))
  return { content: windows.map((window) => window.content).join('\n'), startLine: windows[0]?.startLine || 1, endLine: windows[windows.length - 1]?.endLine || 1, ranges: windows }
}

function conflictBlocks(content) {
  const lines = content.split(/\r?\n/)
  const blocks = []
  let start = null
  let middle = null
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i]
    if (line.slice(0, 7) === '<<<<<<<') {
      start = i + 1
      middle = null
      continue
    }
    if (start && line.slice(0, 7) === '=======') {
      middle = i + 1
      continue
    }
    if (start && line.slice(0, 7) === '>>>>>>>') {
      blocks.push({ startLine: start, separatorLine: middle, endLine: i + 1, content: lines.slice(start - 1, i + 1).join('\n') })
      start = null
      middle = null
    }
  }
  return blocks
}

function processEnv(inputEnv) {
  if (!inputEnv || typeof inputEnv !== 'object' || Array.isArray(inputEnv)) return process.env
  const next = { ...process.env }
  for (const [key, value] of Object.entries(inputEnv)) {
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.exec(key)) throw new Error(`invalid env name: ${key}`)
    if (value === null) delete next[key]
    else next[key] = String(value)
  }
  return next
}

function killChildTree(child, signal) {
  try {
    if (child.pid) {
      process.kill(-child.pid, signal)
      return
    }
  } catch {}
  try {
    child.kill(signal)
  } catch {}
}

function runShellCommand({ cwd, command, timeoutMs, env = null }) {
  return new Promise((resolvePromise) => {
    const child = spawn('/bin/sh', ['-lc', command], { cwd, env: processEnv(env), detached: true })
    let stdout = ''
    let stderr = ''
    let stdoutTruncated = false
    let stderrTruncated = false
    let timedOut = false
    let killTimer = null
    const timer = setTimeout(() => {
      timedOut = true
      killChildTree(child, 'SIGTERM')
      killTimer = setTimeout(() => killChildTree(child, 'SIGKILL'), 1_000)
    }, timeoutMs)
    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString('utf8')
      if (stdout.length > 100_000) {
        stdoutTruncated = true
        stdout = stdout.slice(0, 100_000)
      }
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString('utf8')
      if (stderr.length > 100_000) {
        stderrTruncated = true
        stderr = stderr.slice(0, 100_000)
      }
    })
    child.on('close', (code, signal) => {
      clearTimeout(timer)
      if (killTimer) clearTimeout(killTimer)
      resolvePromise({ code, signal, timedOut, stdout, stderr, stdoutTruncated, stderrTruncated })
    })
  })
}

function runProcess({ cwd, command, args, timeoutMs, stdin = null, env = null }) {
  return new Promise((resolvePromise) => {
    const child = spawn(command, args, { cwd, env: processEnv(env), detached: true })
    let stdout = ''
    let stderr = ''
    let stdoutTruncated = false
    let stderrTruncated = false
    let timedOut = false
    let killTimer = null
    const timer = setTimeout(() => {
      timedOut = true
      killChildTree(child, 'SIGTERM')
      killTimer = setTimeout(() => killChildTree(child, 'SIGKILL'), 1_000)
    }, timeoutMs)
    if (stdin !== null) child.stdin.end(String(stdin))
    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString('utf8')
      if (stdout.length > 100_000) {
        stdoutTruncated = true
        stdout = stdout.slice(0, 100_000)
      }
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString('utf8')
      if (stderr.length > 100_000) {
        stderrTruncated = true
        stderr = stderr.slice(0, 100_000)
      }
    })
    child.on('close', (code, signal) => {
      clearTimeout(timer)
      if (killTimer) clearTimeout(killTimer)
      resolvePromise({ code, signal, timedOut, stdout, stderr, stdoutTruncated, stderrTruncated })
    })
  })
}



async function discoverTextFiles(cwd, root, options = {}) {
  const repoRoot = resolve(cwd)
  if (options.gitignore !== false) {
    const relRoot = publicPath(repoRoot, root)
    const listed = await runProcess({
      cwd: repoRoot,
      command: 'git',
      args: ['ls-files', '-co', '--exclude-standard', '-z', '--', relRoot],
      timeoutMs: 30_000,
    })
    if (listed.code === 0) {
      const files = []
      for (const rel of listed.stdout.split('\u0000')) {
        if (!rel) continue
        const file = jailPath(repoRoot, rel)
        if (file === root || file.slice(0, root.length + 1) === `${root}/`) {
          try {
            const info = await stat(file)
            if (info.isFile()) files.push(file)
          } catch {}
        }
        if (files.length >= MAX_SEARCH_FILES) break
      }
      return files
    }
  }
  const files = []
  await walkFiles(repoRoot, root, files, options)
  return files
}

async function textFilesForInputs(cwd, input) {
  const raw = Array.isArray(input.paths) ? input.paths : [input.path || '.']
  const seen = new Set()
  const files = []
  for (const item of raw) {
    const root = jailPath(cwd, item || '.')
    for (const file of await discoverTextFiles(cwd, root, { gitignore: input.gitignore, hidden: Boolean(input.hidden) })) {
      const key = publicPath(cwd, file)
      if (seen.has(key)) continue
      seen.add(key)
      files.push(file)
      if (files.length >= MAX_SEARCH_FILES) return files
    }
  }
  return files
}

function hasHiddenSegment(path) {
  return path.split('/').some((segment) => segment.slice(0, 1) === '.')
}

async function discoverPathEntries(cwd, root, options = {}) {
  const repoRoot = resolve(cwd)
  if (options.gitignore === false) {
    const entries = []
    await walkPaths(repoRoot, root, entries, { hidden: Boolean(options.hidden), ignored: true, limit: options.limit || MAX_SEARCH_FILES })
    return entries.sort((a, b) => a.path.localeCompare(b.path)).slice(0, options.limit || MAX_SEARCH_FILES)
  }
  const files = await discoverTextFiles(repoRoot, root, { gitignore: options.gitignore, hidden: Boolean(options.hidden) })
  const byPath = new Map()
  for (const file of files) {
    const relFile = publicPath(repoRoot, file)
    if (!options.hidden && hasHiddenSegment(relFile)) continue
    byPath.set(relFile, { path: relFile, absolute: file, type: 'file' })
    let dir = dirname(file)
    while (dir !== root && dir.slice(0, root.length + 1) === `${root}/`) {
      const relDir = publicPath(repoRoot, dir)
      if (options.hidden || !hasHiddenSegment(relDir)) byPath.set(relDir, { path: relDir, absolute: dir, type: 'dir' })
      dir = dirname(dir)
    }
    if (byPath.size >= (options.limit || MAX_SEARCH_FILES)) break
  }
  return Array.from(byPath.values()).sort((a, b) => a.path.localeCompare(b.path)).slice(0, options.limit || MAX_SEARCH_FILES)
}


function cliPath() {
  return fileURLToPath(new URL('./cli.js', import.meta.url))
}
async function packageScripts(cwd) {
  const file = jailPath(cwd, 'package.json')
  const pkg = JSON.parse(await readFile(file, 'utf8'))
  const scripts = pkg?.scripts && typeof pkg.scripts === 'object' && !Array.isArray(pkg.scripts) ? pkg.scripts : {}
  return Object.fromEntries(Object.entries(scripts).filter((entry) => typeof entry[1] === 'string'))
}

async function loadTodoState(file) {
  try {
    const raw = await readFile(file, 'utf8')
    const parsed = JSON.parse(raw)
    return normalizeTodoState(parsed)
  } catch (error) {
    if (error?.code === 'ENOENT') return { phases: [] }
    throw error
  }
}

async function saveTodoState(file, state) {
  await mkdir(dirname(file), { recursive: true })
  await writeFile(file, JSON.stringify(normalizeTodoState(state), null, 2), 'utf8')
}

function todoItem(item, status = 'pending') {
  if (item && typeof item === 'object' && !Array.isArray(item)) {
    return { text: String(item.text ?? item.task ?? item.name ?? ''), status: String(item.status || status) }
  }
  return { text: String(item), status }
}

function normalizeTodoState(state) {
  if (Array.isArray(state?.phases)) {
    return {
      phases: state.phases.map((phase) => ({
        phase: String(phase.phase || phase.name || 'Tasks'),
        items: Array.isArray(phase.items) ? phase.items.map((item) => todoItem(item)) : [],
      })),
    }
  }
  if (Array.isArray(state?.items)) return { phases: [{ phase: 'Tasks', items: state.items.map((item) => todoItem(item)) }] }
  return { phases: [] }
}

function flattenTodoItems(state) {
  const out = []
  for (const phase of state.phases) {
    for (const item of phase.items) out.push(Object.assign(item, { phase: phase.phase }))
  }
  return out
}

function promoteTodo(state) {
  if (flattenTodoItems(state).some((item) => item.status === 'in_progress')) return
  const next = flattenTodoItems(state).find((item) => item.status === 'pending')
  if (next) next.status = 'in_progress'
}

function findTodo(state, task) {
  for (const phase of state.phases) {
    const item = phase.items.find((candidate) => candidate.text === task)
    if (item) return { phase, item }
  }
  return null
}

function summarizeTodos(state) {
  promoteTodo(state)
  const items = flattenTodoItems(state)
  const completed = items.filter((item) => item.status === 'done').length
  const active = items.find((item) => item.status === 'in_progress') || items.find((item) => item.status !== 'done' && item.status !== 'dropped') || null
  return { total: items.length, completed, active: active?.text || null, phases: state.phases, items }
}

function memoryPath() {
  return process.env.JEDEN_MEMORY_FILE ? resolve(process.env.JEDEN_MEMORY_FILE) : resolve(homedir(), '.jeden', 'memory.jsonl')
}

async function loadMemoryEntries(file = memoryPath()) {
  try {
    const raw = await readFile(file, 'utf8')
    return raw.split(/\r?\n/).filter((line) => line.trim()).map((line) => JSON.parse(line))
  } catch (error) {
    if (error?.code === 'ENOENT') return []
    throw error
  }
}

async function saveMemoryEntries(entries, file = memoryPath()) {
  await mkdir(dirname(file), { recursive: true })
  const body = entries.map((entry) => JSON.stringify(entry)).join('\n')
  await writeFile(file, body ? `${body}\n` : '', 'utf8')
}

function memoryId() {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`
}

function memoryMatchesQuery(entry, query) {
  const needle = String(query || '').trim().toLowerCase()
  if (!needle) return true
  const text = `${entry.text || ''} ${(entry.tags || []).join(' ')}`.toLowerCase()
  return text.indexOf(needle) !== -1
}





export function createToolRegistry({ cwd = process.cwd(), allowWrite = false, allowCommand = false, artifactDir = null, customTools = [], askUser = null } = {}) {
  const tools = new Map()

  function add(definition) {
    tools.set(definition.name, definition)
    const capability = {}
    if (definition.permission) capability.permission = definition.permission
    if (definition.hook) capability.hook = definition.hook
    if (!definition.hook && definition.permission === 'write') capability.hook = 'edit'
    if (!definition.hook && definition.permission === 'command') capability.hook = 'bash'
    if (definition.postHook) capability.postHook = definition.postHook
    if (!definition.postHook && definition.permission === 'command') capability.postHook = 'bash'
    if (Object.keys(capability).length > 0) CUSTOM_TOOL_CAPABILITIES.set(definition.name, capability)
  }

  add({
    name: 'list_dir',
    description: 'List a directory under cwd; optional depth recursively includes children',
    input: { path: 'string optional', depth: 'number optional', limit: 'number optional' },
    async execute(input) {
      const dir = jailPath(cwd, input.path || '.')
      const depth = Math.min(Math.max(Number(input.depth) || 1, 1), 5)
      const limit = Math.min(Math.max(Number(input.limit) || 200, 1), 1000)
      if (input.depth === undefined && input.limit === undefined) {
        const listing = await listDirectoryEntries({ cwd, dir, depth: 1, limit: 200 })
        return listing.entries.map((entry) => ({ name: entry.name, type: entry.type }))
      }
      return listDirectoryEntries({ cwd, dir, depth, limit })
    },
  })

  add({
    name: 'read_file',
    description: 'Read a UTF-8 text file under cwd, capped at 512KB unless a selector narrows output; selectors support ranges, comma ranges, raw, and conflicts',
    input: { path: 'string required; may end with selectors like :10-30, :5-8,20-22, :raw, or :conflicts', range: 'string optional' },
    async execute(input) {
      if (!input.path) throw new Error('path is required')
      const selectedPath = splitPathSelector(input.path, input.range)
      const file = jailPath(cwd, selectedPath.path)
      const bytes = await readFile(file)
      const content = bytes.toString('utf8')
      const hasSelector = selectedPath.range || selectedPath.conflicts
      if (!hasSelector && bytes.length > MAX_READ_BYTES) throw new Error('file exceeds 512KB read cap; use a line selector for large files')
      if (selectedPath.conflicts) {
        return { path: publicPath(cwd, file), sha256: sha256(bytes), conflicts: conflictBlocks(content) }
      }
      const selected = lineWindow(content, selectedPath.range)
      if (Buffer.byteLength(selected.content, 'utf8') > MAX_READ_BYTES) throw new Error('selected range exceeds 512KB read cap')
      return { path: publicPath(cwd, file), sha256: sha256(bytes), range: selectedPath.range || null, raw: selectedPath.raw, startLine: selected.startLine, endLine: selected.endLine, ranges: selected.ranges, content: selected.content }
    },
  })

  add({
    name: 'read_binary_file',
    description: 'Read one binary file under cwd as base64, capped at 512KB',
    input: { path: 'string required', maxBytes: 'number optional' },
    async execute(input) {
      if (!input.path) throw new Error('path is required')
      const file = jailPath(cwd, input.path)
      const maxBytes = Math.min(Math.max(Number(input.maxBytes) || MAX_READ_BYTES, 1), MAX_READ_BYTES)
      const content = await readFile(file)
      const sliced = content.subarray(0, maxBytes)
      return {
        path: publicPath(cwd, file),
        bytes: content.length,
        truncated: content.length > sliced.length,
        mimeType: mimeTypeForPath(file),
        base64: sliced.toString('base64'),
        sha256: sha256(content),
      }
    },
  })

  add({
    name: 'read_image',
    description: 'Read one PNG, JPEG, GIF, or WebP image under cwd as base64 with mime type and dimensions, capped at 512KB',
    input: { path: 'string required', maxBytes: 'number optional' },
    async execute(input) {
      if (!input.path) throw new Error('path is required')
      const file = jailPath(cwd, input.path)
      const maxBytes = Math.min(Math.max(Number(input.maxBytes) || MAX_READ_BYTES, 1), MAX_READ_BYTES)
      const content = await readFile(file)
      const sliced = content.subarray(0, maxBytes)
      const metadata = imageMetadata(content, file)
      if (metadata.mimeType.indexOf('image/') !== 0) throw new Error(`unsupported image type: ${metadata.mimeType}`)
      return {
        path: publicPath(cwd, file),
        bytes: content.length,
        truncated: content.length > sliced.length,
        mimeType: metadata.mimeType,
        width: metadata.width,
        height: metadata.height,
        base64: sliced.toString('base64'),
        sha256: sha256(content),
      }
    },
  })

  add({
    name: 'read_archive',
    description: 'List archive entries or read one entry from .zip, .tar, .tar.gz, or .tgz under cwd as text, binary base64, document text, or image metadata',
    input: { path: 'string required', entry: 'string optional', mode: 'text|binary|document|image optional', maxBytes: 'number optional' },
    async execute(input) {
      if (!input.path) throw new Error('path is required')
      const file = jailPath(cwd, input.path)
      const content = await readFile(file)
      const entries = archiveEntries(file, content)
      if (!input.entry) {
        return { path: publicPath(cwd, file), entries: entries.map((entry) => ({ name: entry.name, type: entry.type, bytes: entry.size })) }
      }
      const match = entries.find((entry) => entry.name === input.entry)
      if (!match) throw new Error(`archive entry not found: ${input.entry}`)
      if (match.type !== 'file') throw new Error(`archive entry is not a file: ${input.entry}`)
      const maxBytes = Math.min(Math.max(Number(input.maxBytes) || MAX_READ_BYTES, 1), MAX_READ_BYTES)
      const mode = String(input.mode || 'text')
      const sliced = match.content.subarray(0, maxBytes)
      if (mode === 'binary') {
        return {
          path: publicPath(cwd, file),
          entry: match.name,
          mode,
          bytes: match.content.length,
          truncated: match.content.length > sliced.length,
          mimeType: mimeTypeForPath(match.name),
          base64: sliced.toString('base64'),
          sha256: sha256(match.content),
        }
      }
      if (mode === 'document') {
        const text = readableTextForDocument({ content: match.content, file: match.name })
        const buffer = Buffer.from(text, 'utf8')
        const docSlice = buffer.subarray(0, maxBytes)
        return {
          path: publicPath(cwd, file),
          entry: match.name,
          mode,
          bytes: buffer.length,
          truncated: buffer.length > docSlice.length,
          mimeType: mimeTypeForPath(match.name),
          text: docSlice.toString('utf8'),
          sha256: sha256(match.content),
        }
      }
      if (mode === 'image') {
        const metadata = imageMetadata(match.content, match.name)
        if (metadata.mimeType.indexOf('image/') !== 0) throw new Error(`unsupported image type: ${metadata.mimeType}`)
        return {
          path: publicPath(cwd, file),
          entry: match.name,
          mode,
          bytes: match.content.length,
          truncated: match.content.length > sliced.length,
          mimeType: metadata.mimeType,
          width: metadata.width,
          height: metadata.height,
          base64: sliced.toString('base64'),
          sha256: sha256(match.content),
        }
      }
      if (mode !== 'text') throw new Error(`unsupported archive read mode: ${mode}`)
      return {
        path: publicPath(cwd, file),
        entry: match.name,
        mode,
        bytes: match.content.length,
        truncated: match.content.length > sliced.length,
        content: sliced.toString('utf8'),
        sha256: sha256(match.content),
      }
    },
  })

  add({
    name: 'read_document',
    description: 'Extract readable text from one document under cwd; supports text, HTML, JSON, CSV/TSV, XML/RSS/Atom, notebooks, and basic PDF text streams; capped at 512KB output',
    input: { path: 'string required', maxBytes: 'number optional' },
    async execute(input) {
      if (!input.path) throw new Error('path is required')
      const file = jailPath(cwd, input.path)
      const maxBytes = Math.min(Math.max(Number(input.maxBytes) || MAX_READ_BYTES, 1_000), MAX_READ_BYTES)
      const content = await readFile(file)
      const text = readableTextForDocument({ content, file })
      const buffer = Buffer.from(text, 'utf8')
      const sliced = buffer.subarray(0, maxBytes)
      return {
        path: publicPath(cwd, file),
        bytes: buffer.length,
        truncated: buffer.length > sliced.length,
        mimeType: mimeTypeForPath(file),
        text: sliced.toString('utf8'),
        sha256: sha256(content),
      }
    },
  })

  add({
    name: 'read_sqlite',
    description: 'Read a SQLite database under cwd: list tables, inspect a table, fetch one row by primary key, or run a read-only SELECT/WITH query',
    input: { path: 'string required', table: 'string optional', key: 'string optional', query: 'string optional', limit: 'number optional', offset: 'number optional', where: 'string optional', order: 'string optional' },
    async execute(input) {
      if (!input.path) throw new Error('path is required')
      const file = jailPath(cwd, input.path)
      const limit = Math.min(Math.max(Number(input.limit) || 20, 1), 100)
      const offset = Math.max(Number(input.offset) || 0, 0)
      if (input.query) {
        const query = String(input.query).trim().replace(/;+\s*$/, '')
        if (query.indexOf(';') !== -1 || query.indexOf('\u0000') !== -1) throw new Error('read_sqlite query must be a single SELECT or WITH statement')
        if (query.search(/^(select|with)\b/i) !== 0) throw new Error('read_sqlite query must be SELECT or WITH')
        const rows = await runSqliteJson({ cwd, file, sql: `SELECT * FROM (${query}) LIMIT ${limit} OFFSET ${offset}` })
        return { path: publicPath(cwd, file), query, rows, limit, offset }
      }
      if (!input.table) {
        const tables = await runSqliteJson({
          cwd,
          file,
          sql: "SELECT name, type FROM sqlite_schema WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name",
        })
        for (const table of tables) {
          if (table.type !== 'table') continue
          const countRows = await runSqliteJson({ cwd, file, sql: `SELECT count(*) AS count FROM ${sqliteIdentifier(table.name)}` })
          table.rows = countRows[0]?.count ?? 0
        }
        return { path: publicPath(cwd, file), tables }
      }
      const table = sqliteIdentifier(input.table)
      const schema = await runSqliteJson({ cwd, file, sql: `PRAGMA table_info(${table})` })
      if (input.key != null) {
        const primaryKeys = schema.filter((column) => Number(column.pk) > 0)
        if (primaryKeys.length !== 1) throw new Error(`table has no single-column primary key: ${input.table}`)
        const primaryKey = primaryKeys[0]
        const keySql = JSON.stringify(String(input.key))
        const rows = await runSqliteJson({ cwd, file, sql: `SELECT * FROM ${table} WHERE ${sqliteIdentifier(primaryKey.name)} = ${keySql} LIMIT 1` })
        return { path: publicPath(cwd, file), table: input.table, schema, row: rows[0] || null }
      }
      const clauses = []
      if (input.where) clauses.push(`WHERE ${String(input.where)}`)
      if (input.order) clauses.push(`ORDER BY ${String(input.order)}`)
      clauses.push(`LIMIT ${limit}`)
      if (offset > 0) clauses.push(`OFFSET ${offset}`)
      const rows = await runSqliteJson({ cwd, file, sql: `SELECT * FROM ${table} ${clauses.join(' ')}` })
      return { path: publicPath(cwd, file), table: input.table, schema, rows, limit, offset }
    },
  })

  add({
    name: 'search_text',
    description: 'Search one file for a literal string, capped at 50 line matches; case-insensitive by default',
    input: { path: 'string required', query: 'string required', caseSensitive: 'boolean optional' },
    async execute(input) {
      if (!input.path) throw new Error('path is required')
      if (!input.query) throw new Error('query is required')
      const file = jailPath(cwd, input.path)
      const query = String(input.query)
      const needle = input.caseSensitive === true ? query : query.toLowerCase()
      const matches = []
      const stream = createReadStream(file, { encoding: 'utf8' })
      const lines = createInterface({ input: stream, crlfDelay: Infinity })
      let lineNumber = 0
      for await (const line of lines) {
        lineNumber += 1
        const haystack = input.caseSensitive === true ? line : line.toLowerCase()
        if (haystack.indexOf(needle) !== -1) {
          matches.push({ line: lineNumber, text: line })
          if (matches.length >= 50) break
        }
      }
      stream.destroy()
      return { path: publicPath(cwd, file), matches }
    },
  })

  add({
    name: 'search_files',
    description: 'Recursively search text files under cwd for a literal string, capped at 500 matches; supports path/paths, hidden, gitignore, caseSensitive, limit, and skip; case-insensitive by default',
    input: { path: 'string optional', paths: 'array optional', query: 'string required', hidden: 'boolean optional', gitignore: 'boolean optional', caseSensitive: 'boolean optional', limit: 'number optional', skip: 'number optional' },
    async execute(input) {
      if (!input.query) throw new Error('query is required')
      const files = await textFilesForInputs(cwd, input)
      const limit = Math.min(Math.max(Number(input.limit) || MAX_SEARCH_RESULTS, 1), 500)
      const skip = Math.max(Number(input.skip) || 0, 0)
      const query = String(input.query)
      const needle = input.caseSensitive === true ? query : query.toLowerCase()
      let seen = 0
      const matches = []
      for (const file of files) {
        if (matches.length >= limit) break
        let content = ''
        try {
          content = await readFile(file, 'utf8')
        } catch {
          continue
        }
        if (content.indexOf('\u0000') !== -1) continue
        const lines = content.split(/\r?\n/)
        for (let i = 0; i < lines.length; i += 1) {
          const haystack = input.caseSensitive === true ? lines[i] : lines[i].toLowerCase()
          if (haystack.indexOf(needle) !== -1) {
            seen += 1
            if (seen <= skip) continue
            matches.push({ path: publicPath(cwd, file), line: i + 1, text: lines[i] })
            if (matches.length >= limit) break
          }
        }
      }
      return { searchedFiles: files.length, skip, limit, matches }
    },
  })

  add({
    name: 'glob_paths',
    description: 'Find files and directories under cwd with simple glob patterns; supports * and ** plus hidden, gitignore, limit, and skip',
    input: { patterns: 'string or array optional', path: 'string optional', hidden: 'boolean optional', gitignore: 'boolean optional', limit: 'number optional', skip: 'number optional' },
    async execute(input) {
      const root = jailPath(cwd, input.path || '.')
      const limit = Math.min(Math.max(Number(input.limit) || 200, 1), 2_000)
      const skip = Math.max(Number(input.skip) || 0, 0)
      const rawPatterns = Array.isArray(input.patterns) ? input.patterns : [input.patterns || '**']
      const paths = await discoverPathEntries(cwd, root, { hidden: Boolean(input.hidden), gitignore: input.gitignore, limit: MAX_SEARCH_FILES })
      const matches = paths
        .filter((entry) => matchesPattern(entry.path, rawPatterns))
        .slice(skip, skip + limit)
        .map((entry) => ({ path: entry.path, type: entry.type }))
      return { skip, limit, matches }
    },
  })

  add({
    name: 'grep_regex',
    description: 'Search text files under cwd with a JavaScript regular expression, capped at 500 matches; supports path/paths, hidden, gitignore, multiline, caseSensitive, limit, and skip; case-insensitive by default',
    input: { expr: 'string required', path: 'string optional', paths: 'array optional', hidden: 'boolean optional', gitignore: 'boolean optional', multiline: 'boolean optional', caseSensitive: 'boolean optional', limit: 'number optional', skip: 'number optional' },
    async execute(input) {
      if (!input.expr || typeof input.expr !== 'string') throw new Error('expr is required')
      const limit = Math.min(Math.max(Number(input.limit) || MAX_SEARCH_RESULTS, 1), 500)
      const skip = Math.max(Number(input.skip) || 0, 0)
      let seen = 0
      const multiline = Boolean(input.multiline) || input.expr.indexOf('\n') !== -1
      const matcher = new globalThis.RegExp(input.expr, `${input.caseSensitive ? '' : 'i'}${multiline ? 'gs' : ''}`)
      const files = await textFilesForInputs(cwd, input)
      const matches = []
      for (const file of files) {
        if (matches.length >= limit) break
        let content = ''
        try {
          content = await readFile(file, 'utf8')
        } catch {
          continue
        }
        if (content.indexOf('\u0000') !== -1) continue
        if (multiline) {
          matcher.lastIndex = 0
          let match
          while ((match = matcher.exec(content))) {
            seen += 1
            if (seen > skip) {
              const line = content.slice(0, match.index).split(/\r?\n/).length
              matches.push({ path: publicPath(cwd, file), line, text: match[0].replace(/\s+/g, ' ').slice(0, 500) })
              if (matches.length >= limit) break
            }
            if (match[0].length === 0) matcher.lastIndex += 1
          }
        } else {
          const lines = content.split(/\r?\n/)
          for (let i = 0; i < lines.length; i += 1) {
            matcher.lastIndex = 0
            if (matcher.exec(lines[i])) {
              seen += 1
              if (seen <= skip) continue
              matches.push({ path: publicPath(cwd, file), line: i + 1, text: lines[i] })
              if (matches.length >= limit) break
            }
          }
        }
      }
      return { searchedFiles: files.length, skip, limit, matches }
    },
  })

  add({
    name: 'write_file',
    description: 'Create or overwrite a UTF-8 text file under cwd; overwrites require expectedSha256 from read_file; requires --allow-write',
    input: { path: 'string required', content: 'string required', expectedSha256: 'string required for overwrite' },
    async execute(input) {
      if (!allowWrite) throw new Error('write_file requires --allow-write')
      if (!input.path) throw new Error('path is required')
      if (typeof input.content !== 'string') throw new Error('content is required')
      const file = jailPath(cwd, input.path)
      const exists = await fileExists(file)
      if (exists) {
        const current = await readFile(file, 'utf8')
        const currentHash = sha256(current)
        if (!input.expectedSha256) throw new Error('expectedSha256 is required when overwriting an existing file')
        if (input.expectedSha256 !== currentHash) throw new Error(`sha256 mismatch for ${publicPath(cwd, file)}`)
      }
      await mkdir(dirname(file), { recursive: true })
      await writeFile(file, input.content, 'utf8')
      return { path: publicPath(cwd, file), sha256: sha256(input.content), bytes: Buffer.byteLength(input.content, 'utf8') }
    },
  })

  add({
    name: 'apply_patch',
    description: 'Apply exact one-occurrence string replacements to an existing UTF-8 file; requires expectedSha256 and --allow-write',
    input: { path: 'string required', expectedSha256: 'string required', replacements: 'array of { old, new } required' },
    async execute(input) {
      if (!allowWrite) throw new Error('apply_patch requires --allow-write')
      if (!input.path) throw new Error('path is required')
      if (!input.expectedSha256) throw new Error('expectedSha256 is required')
      const file = jailPath(cwd, input.path)
      const current = await readFile(file, 'utf8')
      const currentHash = sha256(current)
      if (input.expectedSha256 !== currentHash) throw new Error(`sha256 mismatch for ${publicPath(cwd, file)}`)
      const next = applyReplacementList(current, input.replacements)
      await writeFile(file, next, 'utf8')
      return {
        path: publicPath(cwd, file),
        sha256: sha256(next),
        replacements: input.replacements.length,
        bytes: Buffer.byteLength(next, 'utf8'),
      }
    },
  })

  add({
    name: 'edit_file',
    description: 'Apply line-based edits to a UTF-8 file under cwd; requires expectedSha256 and --allow-write',
    input: { path: 'string required', expectedSha256: 'string required', ops: 'array of line edit operations required' },
    async execute(input) {
      if (!allowWrite) throw new Error('edit_file requires --allow-write')
      if (!input.path) throw new Error('path is required')
      if (!input.expectedSha256) throw new Error('expectedSha256 is required')
      const file = jailPath(cwd, input.path)
      const current = await readFile(file, 'utf8')
      const currentHash = sha256(current)
      if (input.expectedSha256 !== currentHash) throw new Error(`sha256 mismatch for ${publicPath(cwd, file)}`)
      const next = applyLineEditOps(current, input.ops)
      await writeFile(file, next, 'utf8')
      return {
        path: publicPath(cwd, file),
        sha256: sha256(next),
        ops: input.ops.length,
        bytes: Buffer.byteLength(next, 'utf8'),
      }
    },
  })

  add({
    name: 'delete_file',
    description: 'Delete one UTF-8 file under cwd; requires expectedSha256 and --allow-write',
    input: { path: 'string required', expectedSha256: 'string required' },
    async execute(input) {
      if (!allowWrite) throw new Error('delete_file requires --allow-write')
      if (!input.path) throw new Error('path is required')
      if (!input.expectedSha256) throw new Error('expectedSha256 is required')
      const file = jailPath(cwd, input.path)
      const current = await readFile(file, 'utf8')
      const currentHash = sha256(current)
      if (currentHash !== input.expectedSha256) throw new Error(`expectedSha256 mismatch: ${currentHash}`)
      await unlink(file)
      return { path: publicPath(cwd, file), deleted: true }
    },
  })

  add({
    name: 'move_file',
    description: 'Move or rename one file under cwd; requires expectedSha256 and --allow-write',
    input: { from: 'string required', to: 'string required', expectedSha256: 'string required', overwrite: 'boolean optional' },
    async execute(input) {
      if (!allowWrite) throw new Error('move_file requires --allow-write')
      if (!input.from) throw new Error('from is required')
      if (!input.to) throw new Error('to is required')
      if (!input.expectedSha256) throw new Error('expectedSha256 is required')
      const from = jailPath(cwd, input.from)
      const to = jailPath(cwd, input.to)
      const current = await readFile(from, 'utf8')
      const currentHash = sha256(current)
      if (currentHash !== input.expectedSha256) throw new Error(`expectedSha256 mismatch: ${currentHash}`)
      if (!input.overwrite && await fileExists(to)) throw new Error('destination exists')
      await mkdir(dirname(to), { recursive: true })
      await rename(from, to)
      return { from: publicPath(cwd, from), to: publicPath(cwd, to), moved: true }
    },
  })

  add({
    name: 'run_command',
    description: 'Run a shell command in cwd; requires --allow-command; supports env overrides; timeout defaults to 30s and maxes at 120s',
    input: { command: 'string required', timeoutMs: 'number optional', env: 'object optional' },
    async execute(input) {
      if (!allowCommand) throw new Error('run_command requires --allow-command')
      if (!input.command || typeof input.command !== 'string') throw new Error('command is required')
      const timeoutMs = Math.min(Math.max(Number(input.timeoutMs) || 30_000, 1_000), 120_000)
      return runShellCommand({ cwd: resolve(cwd), command: input.command, timeoutMs, env: input.env || null })
    },
  })

  add({
    name: 'run_process',
    description: 'Run one process with argv array in cwd without a shell; requires --allow-command; supports env overrides',
    input: { command: 'string required', args: 'array optional', stdin: 'string optional', timeoutMs: 'number optional', env: 'object optional' },
    async execute(input) {
      if (!allowCommand) throw new Error('run_process requires --allow-command')
      if (!input.command || typeof input.command !== 'string') throw new Error('command is required')
      const args = Array.isArray(input.args) ? input.args.map((arg) => String(arg)) : []
      const timeoutMs = Math.min(Math.max(Number(input.timeoutMs) || 30_000, 1_000), 120_000)
      return runProcess({ cwd: resolve(cwd), command: input.command, args, timeoutMs, stdin: input.stdin ?? null, env: input.env || null })
    },
  })

  add({
    name: 'node_eval',
    description: 'Run JavaScript with node --input-type=module in cwd; requires --allow-command',
    input: { code: 'string required', timeoutMs: 'number optional' },
    async execute(input) {
      if (!allowCommand) throw new Error('node_eval requires --allow-command')
      if (!input.code || typeof input.code !== 'string') throw new Error('code is required')
      const timeoutMs = Math.min(Math.max(Number(input.timeoutMs) || 30_000, 1_000), 120_000)
      return runProcess({ cwd: resolve(cwd), command: 'node', args: ['--input-type=module', '-'], timeoutMs, stdin: input.code })
    },
  })

  add({
    name: 'python_eval',
    description: 'Run Python code with python3 in cwd; requires --allow-command',
    input: { code: 'string required', timeoutMs: 'number optional' },
    async execute(input) {
      if (!allowCommand) throw new Error('python_eval requires --allow-command')
      if (!input.code || typeof input.code !== 'string') throw new Error('code is required')
      const timeoutMs = Math.min(Math.max(Number(input.timeoutMs) || 30_000, 1_000), 120_000)
      return runProcess({ cwd: resolve(cwd), command: 'python3', args: ['-'], timeoutMs, stdin: input.code })
    },
  })

  add({
    name: 'list_package_scripts',
    description: 'List package.json scripts in cwd',
    input: {},
    async execute() {
      return packageScripts(cwd)
    },
  })

  add({
    name: 'run_package_script',
    description: 'Run one existing package.json script with npm; requires --allow-command or interactive approval; supports env overrides',
    input: { script: 'string required', timeoutMs: 'number optional', env: 'object optional' },
    async execute(input) {
      if (!allowCommand) throw new Error('run_package_script requires --allow-command')
      if (!input.script || typeof input.script !== 'string') throw new Error('script is required')
      const scripts = await packageScripts(cwd)
      if (typeof scripts[input.script] !== 'string') throw new Error(`unknown package script: ${input.script}`)
      const timeoutMs = Math.min(Math.max(Number(input.timeoutMs) || 60_000, 1_000), 180_000)
      return runProcess({ cwd: resolve(cwd), command: 'npm', args: ['run', input.script], timeoutMs, env: input.env || null })
    },
  })

  add({
    name: 'git_status',
    description: 'Read git status --short for cwd',
    input: {},
    async execute() {
      return runProcess({ cwd: resolve(cwd), command: 'git', args: ['status', '--short'], timeoutMs: 30_000 })
    },
  })

  add({
    name: 'git_diff',
    description: 'Read git diff for cwd or one path under cwd',
    input: { path: 'string optional' },
    async execute(input) {
      const args = ['diff', '--']
      if (input.path) args.push(publicPath(cwd, jailPath(cwd, input.path)))
      return runProcess({ cwd: resolve(cwd), command: 'git', args, timeoutMs: 30_000 })
    },
  })

  add({
    name: 'git_log',
    description: 'Read recent git commits for cwd or one path under cwd',
    input: { limit: 'number optional', path: 'string optional' },
    async execute(input) {
      const limit = Math.min(Math.max(Number(input.limit) || 20, 1), 100)
      const args = ['log', `-${limit}`, '--oneline', '--decorate', '--']
      if (input.path) args.push(publicPath(cwd, jailPath(cwd, input.path)))
      return runProcess({ cwd: resolve(cwd), command: 'git', args, timeoutMs: 30_000 })
    },
  })

  add({
    name: 'git_show',
    description: 'Read one git object or commit summary; optional path scopes the output',
    input: { ref: 'string optional', path: 'string optional' },
    async execute(input) {
      const ref = input.ref ? String(input.ref) : 'HEAD'
      const args = ['show', '--stat', '--oneline', '--decorate', ref, '--']
      if (input.path) args.push(publicPath(cwd, jailPath(cwd, input.path)))
      return runProcess({ cwd: resolve(cwd), command: 'git', args, timeoutMs: 30_000 })
    },
  })

  add({
    name: 'fetch_url',
    description: 'Fetch one HTTP(S) URL and return text capped at maxBytes with byte count, truncation state, SHA-256, and optional timeoutMs',
    input: { url: 'string required', maxBytes: 'number optional', timeoutMs: 'number optional' },
    async execute(input) {
      if (!input.url || typeof input.url !== 'string') throw new Error('url is required')
      const url = new URL(input.url)
      if (url.protocol !== 'http:' && url.protocol !== 'https:') throw new Error('only http and https URLs are allowed')
      const maxBytes = Math.min(Math.max(Number(input.maxBytes) || 200_000, 1_000), 1_000_000)
      const timeoutMs = Math.min(Math.max(Number(input.timeoutMs) || 30_000, 1_000), 120_000)
      const response = await fetchWithTimeout(url, timeoutMs)
      const buffer = Buffer.from(await response.arrayBuffer())
      const sliced = buffer.subarray(0, maxBytes)
      return {
        url: url.toString(),
        status: response.status,
        ok: response.ok,
        contentType: response.headers.get('content-type') || null,
        bytes: buffer.length,
        sha256: sha256(buffer),
        truncated: buffer.length > sliced.length,
        text: sliced.toString('utf8'),
      }
    },
  })

  add({
    name: 'fetch_readable_url',
    description: 'Fetch one HTTP(S) URL and return simplified readable text capped at maxBytes with byte count, truncation state, SHA-256, and optional timeoutMs; supports HTML, JSON, CSV/TSV, RSS/Atom/XML, notebooks, and basic PDF text streams',
    input: { url: 'string required', maxBytes: 'number optional', timeoutMs: 'number optional' },
    async execute(input) {
      if (!input.url || typeof input.url !== 'string') throw new Error('url is required')
      const url = new URL(input.url)
      if (url.protocol !== 'http:' && url.protocol !== 'https:') throw new Error('only http and https URLs are allowed')
      const maxBytes = Math.min(Math.max(Number(input.maxBytes) || 200_000, 1_000), 1_000_000)
      const timeoutMs = Math.min(Math.max(Number(input.timeoutMs) || 30_000, 1_000), 120_000)
      const response = await fetchWithTimeout(url, timeoutMs)
      const raw = Buffer.from(await response.arrayBuffer())
      const contentType = response.headers.get('content-type') || null
      const readable = readableTextForUrlContent({ buffer: raw, contentType, urlPath: url.pathname })
      const buffer = Buffer.from(readable, 'utf8')
      const sliced = buffer.subarray(0, maxBytes)
      return {
        url: url.toString(),
        status: response.status,
        ok: response.ok,
        contentType,
        sourceBytes: raw.length,
        bytes: buffer.length,
        sha256: sha256(buffer),
        truncated: buffer.length > sliced.length,
        text: sliced.toString('utf8'),
      }
    },
  })

  add({
    name: 'save_artifact',
    description: 'Save UTF-8 content into the current session artifacts directory',
    input: { name: 'string required', content: 'string required' },
    async execute(input) {
      if (!artifactDir) throw new Error('save_artifact requires an active session')
      if (!input.name) throw new Error('name is required')
      if (typeof input.content !== 'string') throw new Error('content is required')
      const safeName = String(input.name).replace(/[^a-zA-Z0-9._-]/g, '_')
      const file = resolve(artifactDir, safeName)
      await mkdir(dirname(file), { recursive: true })
      await writeFile(file, input.content, 'utf8')
      return { path: file, bytes: Buffer.byteLength(input.content, 'utf8') }
    },
  })

  add({
    name: 'list_artifacts',
    description: 'List files in the current session artifact directory',
    input: {},
    async execute() {
      if (!artifactDir) throw new Error('list_artifacts requires an active session')
      let entries = []
      try {
        entries = await readdir(resolve(artifactDir), { withFileTypes: true })
      } catch (error) {
        if (error?.code === 'ENOENT') return { artifacts: [] }
        throw error
      }
      const artifacts = []
      for (const entry of entries) {
        if (!entry.isFile()) continue
        const file = resolve(artifactDir, entry.name)
        const info = await stat(file)
        artifacts.push({ name: entry.name, bytes: info.size, updatedAt: info.mtime.toISOString() })
      }
      return { artifacts: artifacts.sort((a, b) => a.name.localeCompare(b.name)) }
    },
  })

  add({
    name: 'read_artifact',
    description: 'Read one UTF-8 artifact from the current session artifact directory',
    input: { name: 'string required', maxBytes: 'number optional' },
    async execute(input) {
      const file = jailedArtifactPath(artifactDir, input.name)
      const maxBytes = Math.min(Math.max(Number(input.maxBytes) || MAX_READ_BYTES, 1_000), MAX_READ_BYTES)
      const content = await readFile(file, 'utf8')
      const buffer = Buffer.from(content, 'utf8')
      const sliced = buffer.subarray(0, maxBytes)
      return {
        name: publicPath(resolve(artifactDir), file),
        bytes: buffer.length,
        truncated: buffer.length > sliced.length,
        content: sliced.toString('utf8'),
        sha256: sha256(content),
      }
    },
  })

  add({
    name: 'ask_user',
    description: 'Ask the human user a question during an interactive session',
    input: { question: 'string required', options: 'array optional' },
    async execute(input) {
      if (!askUser) throw new Error('ask_user requires interactive mode')
      if (!input.question || typeof input.question !== 'string') throw new Error('question is required')
      const options = Array.isArray(input.options) ? input.options.map((option) => String(option)) : []
      const answer = await askUser({ question: input.question, options })
      return { answer }
    },
  })

  add({
    name: 'todo',
    description: 'Manage the current session todo list with init, append, start, done, drop, rm, and view operations; supports phased lists',
    input: { op: 'string required', list: 'array optional', phase: 'string optional', items: 'array optional', task: 'string optional' },
    async execute(input) {
      if (!artifactDir) throw new Error('todo requires an active session')
      const file = resolve(artifactDir, 'todo.json')
      const state = await loadTodoState(file)
      const op = input.op || 'view'
      if (op === 'init') {
        if (Array.isArray(input.list)) {
          state.phases = input.list.map((phase) => ({
            phase: String(phase.phase || phase.name || 'Tasks'),
            items: Array.isArray(phase.items) ? phase.items.map((item) => todoItem(item)) : [],
          }))
        } else {
          state.phases = [{ phase: input.phase ? String(input.phase) : 'Tasks', items: Array.isArray(input.items) ? input.items.map((item) => todoItem(item)) : [] }]
        }
      } else if (op === 'append') {
        if (!Array.isArray(input.items) || input.items.length === 0) throw new Error('items are required')
        const name = input.phase ? String(input.phase) : 'Tasks'
        let phase = state.phases.find((candidate) => candidate.phase === name)
        if (!phase) {
          phase = { phase: name, items: [] }
          state.phases.push(phase)
        }
        phase.items.push(...input.items.map((item) => todoItem(item)))
      } else if (op === 'start') {
        if (!input.task) throw new Error('task is required')
        const match = findTodo(state, input.task)
        if (!match) throw new Error(`unknown task: ${input.task}`)
        for (const item of flattenTodoItems(state)) if (item.status === 'in_progress') item.status = 'pending'
        match.item.status = 'in_progress'
      } else if (op === 'done' || op === 'drop') {
        const nextStatus = op === 'done' ? 'done' : 'dropped'
        if (input.phase && !input.task) {
          const phase = state.phases.find((candidate) => candidate.phase === String(input.phase))
          if (!phase) throw new Error(`unknown phase: ${input.phase}`)
          for (const item of phase.items) item.status = nextStatus
        } else {
          if (!input.task) throw new Error('task is required')
          const match = findTodo(state, input.task)
          if (!match) throw new Error(`unknown task: ${input.task}`)
          match.item.status = nextStatus
        }
      } else if (op === 'rm') {
        if (input.phase) state.phases = state.phases.filter((phase) => phase.phase !== String(input.phase))
        else if (input.task) {
          for (const phase of state.phases) phase.items = phase.items.filter((item) => item.text !== input.task)
        } else state.phases = []
      } else if (op !== 'view') {
        throw new Error(`unknown todo op: ${op}`)
      }
      const summary = summarizeTodos(state)
      await saveTodoState(file, state)
      return summary
    },
  })

  add({
    name: 'delegate_task',
    description: 'Run a focused subtask in a fresh Jeden session and return its result',
    input: { task: 'string required', maxSteps: 'number optional' },
    async execute(input) {
      if (!allowCommand) throw new Error('delegate_task requires --allow-command')
      if (!input.task || typeof input.task !== 'string') throw new Error('task is required')
      const maxSteps = Math.min(Math.max(Number(input.maxSteps) || 6, 1), 16)
      const result = await runProcess({
        cwd: resolve(cwd),
        command: process.env.JEDEN_NODE || 'node',
        args: [cliPath(), 'run', input.task, '--cwd', resolve(cwd), '--max-steps', String(maxSteps), '--json'],
        timeoutMs: Math.min(maxSteps * 45_000, 300_000),
      })
      let parsed = null
      try {
        parsed = JSON.parse(result.stdout)
      } catch {}
      return { ...result, delegated: parsed }
    },
  })

  add({
    name: 'memory',
    description: 'Remember and recall durable notes across Jeden sessions',
    input: { op: 'string required', text: 'string optional', query: 'string optional', tags: 'array optional', limit: 'number optional' },
    async execute(input) {
      const entries = await loadMemoryEntries()
      const op = input.op || 'recall'
      if (op === 'remember') {
        if (!input.text || typeof input.text !== 'string') throw new Error('text is required')
        const entry = {
          id: memoryId(),
          text: input.text,
          tags: Array.isArray(input.tags) ? input.tags.map((tag) => String(tag)) : [],
          createdAt: new Date().toISOString(),
        }
        entries.push(entry)
        await saveMemoryEntries(entries)
        return { entry }
      }
      if (op === 'list') {
        const limit = Math.min(Math.max(Number(input.limit) || 20, 1), 200)
        return { entries: entries.slice(-limit).reverse() }
      }
      if (op === 'recall') {
        const limit = Math.min(Math.max(Number(input.limit) || 10, 1), 100)
        const matched = entries.filter((entry) => memoryMatchesQuery(entry, input.query))
        return { entries: matched.slice(-limit).reverse(), query: input.query || null }
      }
      throw new Error(`unknown memory op: ${op}`)
    },
  })

  add({
    name: 'mcp_list_tools',
    description: 'List tools from a configured stdio MCP server',
    input: { server: 'string required', timeoutMs: 'number optional' },
    async execute(input) {
      if (!input.server || typeof input.server !== 'string') throw new Error('server is required')
      const timeoutMs = Math.min(Math.max(Number(input.timeoutMs) || 30_000, 1_000), 120_000)
      return listMcpTools({ cwd, serverName: input.server, timeoutMs })
    },
  })

  add({
    name: 'mcp_call_tool',
    description: 'Call one tool on a configured stdio MCP server',
    input: { server: 'string required', tool: 'string required', args: 'object optional', timeoutMs: 'number optional' },
    async execute(input) {
      if (!input.server || typeof input.server !== 'string') throw new Error('server is required')
      if (!input.tool || typeof input.tool !== 'string') throw new Error('tool is required')
      const timeoutMs = Math.min(Math.max(Number(input.timeoutMs) || 30_000, 1_000), 120_000)
      return callMcpTool({ cwd, serverName: input.server, toolName: input.tool, args: input.args || {}, timeoutMs })
    },
  })

  add({
    name: 'mcp_list_resources',
    description: 'List resources from a configured stdio MCP server',
    input: { server: 'string required', timeoutMs: 'number optional' },
    async execute(input) {
      if (!input.server || typeof input.server !== 'string') throw new Error('server is required')
      const timeoutMs = Math.min(Math.max(Number(input.timeoutMs) || 30_000, 1_000), 120_000)
      return listMcpResources({ cwd, serverName: input.server, timeoutMs })
    },
  })

  add({
    name: 'mcp_read_resource',
    description: 'Read one resource from a configured stdio MCP server',
    input: { server: 'string required', uri: 'string required', timeoutMs: 'number optional' },
    async execute(input) {
      if (!input.server || typeof input.server !== 'string') throw new Error('server is required')
      if (!input.uri || typeof input.uri !== 'string') throw new Error('uri is required')
      const timeoutMs = Math.min(Math.max(Number(input.timeoutMs) || 30_000, 1_000), 120_000)
      return readMcpResource({ cwd, serverName: input.server, uri: input.uri, timeoutMs })
    },
  })

  add({
    name: 'mcp_list_prompts',
    description: 'List prompts from a configured stdio MCP server',
    input: { server: 'string required', timeoutMs: 'number optional' },
    async execute(input) {
      if (!input.server || typeof input.server !== 'string') throw new Error('server is required')
      const timeoutMs = Math.min(Math.max(Number(input.timeoutMs) || 30_000, 1_000), 120_000)
      return listMcpPrompts({ cwd, serverName: input.server, timeoutMs })
    },
  })

  add({
    name: 'mcp_get_prompt',
    description: 'Get one prompt from a configured stdio MCP server',
    input: { server: 'string required', name: 'string required', args: 'object optional', timeoutMs: 'number optional' },
    async execute(input) {
      if (!input.server || typeof input.server !== 'string') throw new Error('server is required')
      if (!input.name || typeof input.name !== 'string') throw new Error('name is required')
      const timeoutMs = Math.min(Math.max(Number(input.timeoutMs) || 30_000, 1_000), 120_000)
      return getMcpPrompt({ cwd, serverName: input.server, name: input.name, args: input.args || {}, timeoutMs })
    },
  })

  for (const tool of customTools) {
    if (tools.has(tool.name)) throw new Error(`tool name conflict: ${tool.name}`)
    add(tool)
  }

  return {
    list() {
      return Array.from(tools.values()).map(({ name, description, input }) => ({ name, description, input }))
    },
    async execute(name, input) {
      const tool = tools.get(name)
      if (!tool) return { ok: false, error: `unknown tool: ${name}` }
      try {
        return { ok: true, output: await tool.execute(input || {}) }
      } catch (error) {
        return { ok: false, error: error instanceof Error ? error.message : String(error) }
      }
    },
  }
}
