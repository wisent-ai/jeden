import { readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'

const roots = ['src', 'scripts']
let failed = false

function walk(dir) {
  let entries = []
  try {
    entries = readdirSync(dir)
  } catch {
    return
  }
  for (const entry of entries) {
    const file = join(dir, entry)
    const st = statSync(file)
    if (st.isDirectory()) {
      if (entry === 'node_modules' || entry === '.git') continue
      walk(file)
      continue
    }
    if (!file.endsWith('.js') && !file.endsWith('.mjs')) continue
    const result = spawnSync(process.execPath, ['--check', file], { stdio: 'inherit' })
    if (result.status !== 0) failed = true
  }
}

for (const root of roots) walk(root)
if (failed) process.exit(1)
console.log('syntax ok')
