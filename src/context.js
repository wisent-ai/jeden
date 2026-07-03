import { readFile, stat } from 'node:fs/promises'
import { homedir } from 'node:os'
import { dirname, relative, resolve } from 'node:path'

const MAX_CONTEXT_BYTES = 64_000
const PROJECT_CONTEXT_FILES = [
  'JEDEN.md',
  'AGENTS.md',
  'CLAUDE.md',
  'RULES.md',
  '.omp/AGENTS.md',
  '.omp/RULES.md',
  '.jeden/instructions.md',
  '.jeden/context.md',
  '.jeden/rules.jsonl',
]

const USER_CONTEXT_FILES = [
  '.jeden/instructions.md',
  '.jeden/context.md',
]

function under(root, target) {
  return target === root || target.slice(0, root.length + 1) === `${root}/`
}


function ancestorDirs(root) {
  const start = resolve(root)
  const home = resolve(homedir())
  const floor = under(home, start) ? home : dirname(start)
  const dirs = []
  let current = start
  for (;;) {
    dirs.push(current)
    if (current === floor) break
    const parent = dirname(current)
    if (parent === current) break
    current = parent
  }
  return dirs.reverse()
}

function labelPath(base, dir, file) {
  const relDir = relative(base, dir)
  return relDir ? `${relDir}/${file}` : file
}

async function readContextImport(root, sourceDir, specifier) {
  if (!specifier || specifier.slice(0, 1) === '/') return null
  const file = resolve(sourceDir, specifier)
  if (!under(root, file)) return null
  try {
    const info = await stat(file)
    if (!info.isFile() || info.size > MAX_CONTEXT_BYTES) return null
    return { path: relative(root, file), content: await readFile(file, 'utf8') }
  } catch (error) {
    if (error?.code === 'ENOENT' || error?.code === 'ENOTDIR') return null
    throw error
  }
}

async function expandContextImports(root, relativePath, content) {
  const sourceDir = dirname(resolve(root, relativePath))
  const imports = []
  for (const line of content.split(/\r?\n/)) {
    const match = /^@([^\s]+)\s*$/.exec(line.trim())
    if (!match) continue
    const imported = await readContextImport(root, sourceDir, match[1])
    if (imported) imports.push(imported)
  }
  if (imports.length === 0) return content
  const sections = imports.map((item) => `# Imported ${item.path}\n${item.content.trim()}`)
  return `${content.trim()}\n\n${sections.join('\n\n')}\n`
}
async function readContextFile(root, relativePath, label = relativePath) {
  const file = resolve(root, relativePath)
  if (!under(root, file)) return null
  try {
    const info = await stat(file)
    if (!info.isFile() || info.size > MAX_CONTEXT_BYTES) return null
    const content = await expandContextImports(root, relativePath, await readFile(file, 'utf8'))
    return { path: label, content }
  } catch (error) {
    if (error?.code === 'ENOENT' || error?.code === 'ENOTDIR') return null
    throw error
  }
}

export async function loadProjectContext({ cwd = process.cwd() } = {}) {
  const root = resolve(cwd)
  const loaded = []
  const seen = new Set()
  for (const file of USER_CONTEXT_FILES) {
    const context = await readContextFile(homedir(), file, `~/${file}`)
    if (context) {
      loaded.push(context)
      seen.add(context.path)
    }
  }
  for (const dir of ancestorDirs(root)) {
    for (const file of PROJECT_CONTEXT_FILES) {
      const label = labelPath(root, dir, file)
      if (seen.has(label)) continue
      const context = await readContextFile(dir, file, label)
      if (context) {
        loaded.push(context)
        seen.add(context.path)
      }
    }
  }
  return loaded
}

export function formatProjectContext(contextFiles) {
  if (!contextFiles || contextFiles.length === 0) return ''
  const sections = contextFiles.map((file) => `# ${file.path}\n${file.content.trim()}`)
  return `Project context files:\n\n${sections.join('\n\n')}`
}
