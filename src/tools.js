import { readdir, readFile, writeFile, stat } from 'node:fs/promises'
import { createReadStream } from 'node:fs'
import { createHash } from 'node:crypto'
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { resolve, relative, dirname } from 'node:path'
import { mkdir } from 'node:fs/promises'

const MAX_READ_BYTES = 512_000
const MAX_SEARCH_RESULTS = 100
const MAX_SEARCH_FILES = 2_000
const SKIP_DIRS = new Set(['.git', 'node_modules', '.next', 'dist', 'build', 'coverage', '.vercel'])

function sha256(content) {
  return createHash('sha256').update(content).digest('hex')
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

async function fileExists(path) {
  try {
    const info = await stat(path)
    return info.isFile()
  } catch (error) {
    if (error?.code === 'ENOENT') return false
    throw error
  }
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


async function walkFiles(root, start, out) {
  if (out.length >= MAX_SEARCH_FILES) return
  const entries = await readdir(start, { withFileTypes: true })
  for (const entry of entries) {
    if (out.length >= MAX_SEARCH_FILES) return
    if (entry.name.slice(0, 1) === '.' && entry.name !== '.env.local') continue
    const full = resolve(start, entry.name)
    if (entry.isDirectory()) {
      if (SKIP_DIRS.has(entry.name)) continue
      await walkFiles(root, full, out)
      continue
    }
    if (entry.isFile()) out.push(full)
  }
}

async function walkPaths(root, start, out, options = {}) {
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

function lineWindow(content, range) {
  if (!range) return { content, startLine: 1, endLine: content.split(/\r?\n/).length }
  const parts = String(range).split('-')
  const start = Math.max(Number(parts[0]) || 1, 1)
  const lines = content.split(/\r?\n/)
  const end = Math.min(parts[1] ? Number(parts[1]) || start : start, lines.length)
  const selected = lines.slice(start - 1, end)
  return { content: selected.join('\n'), startLine: start, endLine: end }
}

function runShellCommand({ cwd, command, timeoutMs }) {
  return new Promise((resolvePromise) => {
    const child = spawn('/bin/sh', ['-lc', command], { cwd, env: process.env })
    let stdout = ''
    let stderr = ''
    let timedOut = false
    const timer = setTimeout(() => {
      timedOut = true
      child.kill('SIGTERM')
    }, timeoutMs)
    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString('utf8')
      if (stdout.length > 100_000) stdout = stdout.slice(0, 100_000)
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString('utf8')
      if (stderr.length > 100_000) stderr = stderr.slice(0, 100_000)
    })
    child.on('close', (code, signal) => {
      clearTimeout(timer)
      resolvePromise({ code, signal, timedOut, stdout, stderr })
    })
  })
}

function runProcess({ cwd, command, args, timeoutMs }) {
  return new Promise((resolvePromise) => {
    const child = spawn(command, args, { cwd, env: process.env })
    let stdout = ''
    let stderr = ''
    let timedOut = false
    const timer = setTimeout(() => {
      timedOut = true
      child.kill('SIGTERM')
    }, timeoutMs)
    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString('utf8')
      if (stdout.length > 100_000) stdout = stdout.slice(0, 100_000)
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString('utf8')
      if (stderr.length > 100_000) stderr = stderr.slice(0, 100_000)
    })
    child.on('close', (code, signal) => {
      clearTimeout(timer)
      resolvePromise({ code, signal, timedOut, stdout, stderr })
    })
  })
}

async function packageScripts(cwd) {
  const file = jailPath(cwd, 'package.json')
  const pkg = JSON.parse(await readFile(file, 'utf8'))
  const scripts = pkg?.scripts && typeof pkg.scripts === 'object' && !Array.isArray(pkg.scripts) ? pkg.scripts : {}
  return Object.fromEntries(Object.entries(scripts).filter((entry) => typeof entry[1] === 'string'))
}


export function createToolRegistry({ cwd = process.cwd(), allowWrite = false, allowCommand = false, artifactDir = null, customTools = [] } = {}) {
  const tools = new Map()

  function add(definition) {
    tools.set(definition.name, definition)
  }

  add({
    name: 'list_dir',
    description: 'List one directory under cwd',
    input: { path: 'string optional' },
    async execute(input) {
      const dir = jailPath(cwd, input.path || '.')
      const entries = await readdir(dir, { withFileTypes: true })
      return entries
        .map((entry) => ({ name: entry.name, type: entry.isDirectory() ? 'dir' : entry.isFile() ? 'file' : 'other' }))
        .sort((a, b) => a.name.localeCompare(b.name))
    },
  })

  add({
    name: 'read_file',
    description: 'Read a UTF-8 text file under cwd, capped at 512KB; optional range uses 1-based lines like 10-30',
    input: { path: 'string required', range: 'string optional' },
    async execute(input) {
      if (!input.path) throw new Error('path is required')
      const file = jailPath(cwd, input.path)
      const content = await readFile(file, 'utf8')
      if (Buffer.byteLength(content, 'utf8') > MAX_READ_BYTES) throw new Error('file exceeds 512KB read cap')
      const selected = lineWindow(content, input.range)
      return { path: publicPath(cwd, file), sha256: sha256(content), range: input.range || null, startLine: selected.startLine, endLine: selected.endLine, content: selected.content }
    },
  })

  add({
    name: 'search_text',
    description: 'Search one file for a literal string, capped at 50 line matches',
    input: { path: 'string required', query: 'string required' },
    async execute(input) {
      if (!input.path) throw new Error('path is required')
      if (!input.query) throw new Error('query is required')
      const file = jailPath(cwd, input.path)
      const matches = []
      const stream = createReadStream(file, { encoding: 'utf8' })
      const lines = createInterface({ input: stream, crlfDelay: Infinity })
      let lineNumber = 0
      for await (const line of lines) {
        lineNumber += 1
        if (line.indexOf(String(input.query)) !== -1) {
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
    description: 'Recursively search text files under cwd for a literal string, capped at 100 matches',
    input: { path: 'string optional', query: 'string required' },
    async execute(input) {
      if (!input.query) throw new Error('query is required')
      const root = jailPath(cwd, input.path || '.')
      const files = []
      await walkFiles(resolve(cwd), root, files)
      const matches = []
      for (const file of files) {
        if (matches.length >= MAX_SEARCH_RESULTS) break
        let content = ''
        try {
          content = await readFile(file, 'utf8')
        } catch {
          continue
        }
        if (content.indexOf('\u0000') !== -1) continue
        const lines = content.split(/\r?\n/)
        for (let i = 0; i < lines.length; i += 1) {
          if (lines[i].indexOf(String(input.query)) !== -1) {
            matches.push({ path: publicPath(cwd, file), line: i + 1, text: lines[i] })
            if (matches.length >= MAX_SEARCH_RESULTS) break
          }
        }
      }
      return { searchedFiles: files.length, matches }
    },
  })

  add({
    name: 'glob_paths',
    description: 'Find files and directories under cwd with simple glob patterns; supports * and **',
    input: { patterns: 'string or array optional', path: 'string optional', hidden: 'boolean optional', limit: 'number optional' },
    async execute(input) {
      const root = jailPath(cwd, input.path || '.')
      const limit = Math.min(Math.max(Number(input.limit) || 200, 1), 2_000)
      const rawPatterns = Array.isArray(input.patterns) ? input.patterns : [input.patterns || '**']
      const paths = []
      await walkPaths(resolve(cwd), root, paths, { hidden: Boolean(input.hidden), limit })
      const matches = paths
        .filter((entry) => matchesPattern(entry.path, rawPatterns))
        .slice(0, limit)
        .map((entry) => ({ path: entry.path, type: entry.type }))
      return { matches }
    },
  })

  add({
    name: 'grep_regex',
    description: 'Search text files under cwd with a JavaScript regular expression, capped at 100 matches',
    input: { expr: 'string required', path: 'string optional', caseSensitive: 'boolean optional', limit: 'number optional' },
    async execute(input) {
      if (!input.expr || typeof input.expr !== 'string') throw new Error('expr is required')
      const root = jailPath(cwd, input.path || '.')
      const limit = Math.min(Math.max(Number(input.limit) || MAX_SEARCH_RESULTS, 1), 500)
      const matcher = new globalThis.RegExp(input.expr, input.caseSensitive ? '' : 'i')
      const entries = []
      await walkPaths(resolve(cwd), root, entries, { hidden: false, limit: MAX_SEARCH_FILES })
      const files = entries.filter((entry) => entry.type === 'file').map((entry) => entry.absolute)
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
          matcher.lastIndex = 0
          if (matcher.exec(lines[i])) {
            matches.push({ path: publicPath(cwd, file), line: i + 1, text: lines[i] })
            if (matches.length >= limit) break
          }
        }
      }
      return { searchedFiles: files.length, matches }
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
    name: 'run_command',
    description: 'Run a shell command in cwd; requires --allow-command; timeout defaults to 30s and maxes at 120s',
    input: { command: 'string required', timeoutMs: 'number optional' },
    async execute(input) {
      if (!allowCommand) throw new Error('run_command requires --allow-command')
      if (!input.command || typeof input.command !== 'string') throw new Error('command is required')
      const timeoutMs = Math.min(Math.max(Number(input.timeoutMs) || 30_000, 1_000), 120_000)
      return runShellCommand({ cwd: resolve(cwd), command: input.command, timeoutMs })
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
    description: 'Run one existing package.json script with npm; requires --allow-command or interactive approval',
    input: { script: 'string required', timeoutMs: 'number optional' },
    async execute(input) {
      if (!allowCommand) throw new Error('run_package_script requires --allow-command')
      if (!input.script || typeof input.script !== 'string') throw new Error('script is required')
      const scripts = await packageScripts(cwd)
      if (typeof scripts[input.script] !== 'string') throw new Error(`unknown package script: ${input.script}`)
      const timeoutMs = Math.min(Math.max(Number(input.timeoutMs) || 60_000, 1_000), 180_000)
      return runProcess({ cwd: resolve(cwd), command: 'npm', args: ['run', input.script], timeoutMs })
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
    name: 'fetch_url',
    description: 'Fetch one HTTP(S) URL and return text capped at maxBytes',
    input: { url: 'string required', maxBytes: 'number optional' },
    async execute(input) {
      if (!input.url || typeof input.url !== 'string') throw new Error('url is required')
      const url = new URL(input.url)
      if (url.protocol !== 'http:' && url.protocol !== 'https:') throw new Error('only http and https URLs are allowed')
      const maxBytes = Math.min(Math.max(Number(input.maxBytes) || 200_000, 1_000), 1_000_000)
      const response = await fetch(url)
      const buffer = Buffer.from(await response.arrayBuffer())
      const sliced = buffer.subarray(0, maxBytes)
      return {
        url: url.toString(),
        status: response.status,
        ok: response.ok,
        contentType: response.headers.get('content-type') || null,
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
