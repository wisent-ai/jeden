import { readdir, readFile, writeFile } from 'node:fs/promises'
import { createReadStream } from 'node:fs'
import { createInterface } from 'node:readline'
import { resolve, relative, sep } from 'node:path'

const MAX_READ_BYTES = 512_000
const MAX_SEARCH_RESULTS = 50

function jailPath(cwd, inputPath) {
  const root = resolve(cwd)
  const target = resolve(root, String(inputPath || '.'))
  const rel = relative(root, target)
  if (rel === '' || (!rel.startsWith('..') && !rel.includes(`..${sep}`) && !resolve(rel).startsWith(sep))) return target
  throw new Error(`path escapes cwd: ${inputPath}`)
}

function publicPath(cwd, target) {
  const rel = relative(resolve(cwd), target)
  return rel || '.'
}

export function createToolRegistry({ cwd = process.cwd(), allowWrite = false } = {}) {
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
    description: 'Read a UTF-8 text file under cwd, capped at 512KB',
    input: { path: 'string required' },
    async execute(input) {
      if (!input.path) throw new Error('path is required')
      const file = jailPath(cwd, input.path)
      const content = await readFile(file, 'utf8')
      if (Buffer.byteLength(content, 'utf8') > MAX_READ_BYTES) throw new Error('file exceeds 512KB read cap')
      return { path: publicPath(cwd, file), content }
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
        if (line.includes(String(input.query))) {
          matches.push({ line: lineNumber, text: line })
          if (matches.length >= MAX_SEARCH_RESULTS) break
        }
      }
      stream.destroy()
      return { path: publicPath(cwd, file), matches }
    },
  })

  add({
    name: 'write_file',
    description: 'Create or overwrite a UTF-8 text file under cwd; requires --allow-write',
    input: { path: 'string required', content: 'string required' },
    async execute(input) {
      if (!allowWrite) throw new Error('write_file requires --allow-write')
      if (!input.path) throw new Error('path is required')
      if (typeof input.content !== 'string') throw new Error('content is required')
      const file = jailPath(cwd, input.path)
      await writeFile(file, input.content, 'utf8')
      return { path: publicPath(cwd, file), bytes: Buffer.byteLength(input.content, 'utf8') }
    },
  })

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
