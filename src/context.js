import { readFile, stat } from 'node:fs/promises'
import { resolve } from 'node:path'

const MAX_CONTEXT_BYTES = 64_000
const CONTEXT_FILES = [
  'JEDEN.md',
  'AGENTS.md',
  'CLAUDE.md',
  '.jeden/instructions.md',
  '.jeden/context.md',
]

function under(root, target) {
  return target === root || target.slice(0, root.length + 1) === `${root}/`
}

async function readContextFile(root, relativePath) {
  const file = resolve(root, relativePath)
  if (!under(root, file)) return null
  try {
    const info = await stat(file)
    if (!info.isFile() || info.size > MAX_CONTEXT_BYTES) return null
    return { path: relativePath, content: await readFile(file, 'utf8') }
  } catch (error) {
    if (error?.code === 'ENOENT' || error?.code === 'ENOTDIR') return null
    throw error
  }
}

export async function loadProjectContext({ cwd = process.cwd() } = {}) {
  const root = resolve(cwd)
  const loaded = []
  for (const file of CONTEXT_FILES) {
    const context = await readContextFile(root, file)
    if (context) loaded.push(context)
  }
  return loaded
}

export function formatProjectContext(contextFiles) {
  if (!contextFiles || contextFiles.length === 0) return ''
  const sections = contextFiles.map((file) => `# ${file.path}\n${file.content.trim()}`)
  return `Project context files:\n\n${sections.join('\n\n')}`
}
