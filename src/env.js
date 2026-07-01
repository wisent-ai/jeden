import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const ENV_FILES = ['.env', '.env.local', '.env.production', '.env.vercel']

function parseValue(raw) {
  let value = raw.trim()
  const commentIndex = value.search(/\s+#/)
  if (commentIndex !== -1) value = value.slice(0, commentIndex).trim()
  if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
    value = value.slice(1, -1)
  }
  return value.replace(/\\n/g, '\n')
}

export function loadEnvFiles({ dirs = [process.cwd()] } = {}) {
  const loaded = []
  const uniqueDirs = Array.from(new Set(dirs.filter(Boolean).map((dir) => resolve(dir))))

  for (const dir of uniqueDirs) {
    for (const name of ENV_FILES) {
      const file = resolve(dir, name)
      let text = ''
      try {
        text = readFileSync(file, 'utf8')
      } catch (error) {
        if (error?.code === 'ENOENT') continue
        throw error
      }
      for (const line of text.split(/\r?\n/)) {
        const trimmed = line.trim()
        if (!trimmed || trimmed.startsWith('#')) continue
        const index = trimmed.indexOf('=')
        if (index === -1) continue
        const key = trimmed.slice(0, index).trim()
        const value = parseValue(trimmed.slice(index + 1))
        if (!key || process.env[key]) continue
        process.env[key] = value
        loaded.push(key)
      }
    }
  }

  return Array.from(new Set(loaded)).sort()
}
