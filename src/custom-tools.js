import { readdir, readFile } from 'node:fs/promises'
import { spawn } from 'node:child_process'
import { homedir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

const MAX_OUTPUT_BYTES = 100_000

function isLoadableModule(name) {
  return name.endsWith('.js') || name.endsWith('.mjs')
}

function unique(values) {
  return [...new Set(values)]
}

function toolDirs(cwd) {
  return unique([
    join(homedir(), '.jeden', 'tools'),
    join(resolve(cwd), '.jeden', 'tools'),
  ])
}

async function listToolFiles(dir) {
  try {
    const entries = await readdir(dir, { withFileTypes: true })
    return entries
      .filter((entry) => entry.isFile() && isLoadableModule(entry.name))
      .map((entry) => join(dir, entry.name))
      .sort()
  } catch (error) {
    if (error?.code === 'ENOENT' || error?.code === 'ENOTDIR') return []
    throw error
  }
}

function cap(value) {
  if (value.length <= MAX_OUTPUT_BYTES) return value
  return value.slice(0, MAX_OUTPUT_BYTES)
}

function exec(command, args = [], options = {}) {
  if (!command || typeof command !== 'string') throw new Error('command is required')
  if (!Array.isArray(args) || args.some((arg) => typeof arg !== 'string')) throw new Error('args must be strings')
  const cwd = options.cwd ? resolve(String(options.cwd)) : process.cwd()
  const timeoutMs = Math.min(Math.max(Number(options.timeoutMs) || 30_000, 1_000), 180_000)
  return new Promise((resolvePromise) => {
    const child = spawn(command, args, { cwd, env: { ...process.env, ...(options.env || {}) } })
    let stdout = ''
    let stderr = ''
    let timedOut = false
    const timer = setTimeout(() => {
      timedOut = true
      child.kill('SIGTERM')
    }, timeoutMs)
    child.stdout.on('data', (chunk) => { stdout = cap(stdout + chunk.toString('utf8')) })
    child.stderr.on('data', (chunk) => { stderr = cap(stderr + chunk.toString('utf8')) })
    child.on('close', (code, signal) => {
      clearTimeout(timer)
      resolvePromise({ code, signal, timedOut, stdout, stderr })
    })
  })
}

function isToolNameCharacter(code) {
  return (
    (code >= 48 && code <= 57) ||
    (code >= 65 && code <= 90) ||
    (code >= 97 && code <= 122) ||
    code === 45 ||
    code === 46 ||
    code === 95
  )
}

function isToolName(value) {
  if (value.length < 1 || value.length > 80) return false
  for (let i = 0; i < value.length; i += 1) {
    if (!isToolNameCharacter(value.charCodeAt(i))) return false
  }
  return true
}

function normalizeTool(raw, source) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('tool must be an object')
  if (!raw.name || typeof raw.name !== 'string') throw new Error('tool.name is required')
  if (!isToolName(raw.name)) throw new Error(`invalid tool name: ${raw.name}`)
  if (!raw.description || typeof raw.description !== 'string') throw new Error(`tool.description is required for ${raw.name}`)
  if (typeof raw.execute !== 'function') throw new Error(`tool.execute is required for ${raw.name}`)
  return {
    name: raw.name,
    description: raw.description,
    input: raw.input && typeof raw.input === 'object' && !Array.isArray(raw.input) ? raw.input : {},
    source,
    execute: raw.execute,
  }
}

async function loadToolFile(file, api) {
  const url = `${pathToFileURL(file).href}?mtime=${Date.now()}`
  const module = await import(url)
  const factory = module.default || module.tool || module.tools
  const produced = typeof factory === 'function' ? await factory(api) : factory
  const list = Array.isArray(produced) ? produced : [produced]
  return list.map((tool) => normalizeTool(tool, file))
}

export async function discoverCustomToolFiles({ cwd = process.cwd() } = {}) {
  const dirs = toolDirs(cwd)
  const nested = await Promise.all(dirs.map((dir) => listToolFiles(dir)))
  return nested.flat()
}

export async function loadCustomTools({ cwd = process.cwd(), builtInToolNames = [], allowCommand = false } = {}) {
  const files = await discoverCustomToolFiles({ cwd })
  const errors = []
  const tools = []
  const seen = new Set(builtInToolNames)
  const api = {
    cwd: resolve(cwd),
    exec: (command, args, options = {}) => {
      if (!allowCommand) throw new Error('custom tool exec requires --allow-command')
      return exec(command, args, { cwd: resolve(cwd), ...options })
    },
    readText: (path) => readFile(resolve(cwd, path), 'utf8'),
    dirname,
  }

  for (const file of files) {
    try {
      const loaded = await loadToolFile(file, api)
      for (const tool of loaded) {
        if (seen.has(tool.name)) throw new Error(`tool name conflict: ${tool.name}`)
        seen.add(tool.name)
        tools.push(tool)
      }
    } catch (error) {
      errors.push({ path: file, error: error instanceof Error ? error.message : String(error) })
    }
  }

  return { tools, errors }
}
