#!/usr/bin/env node

import { loadEnvFiles } from './env.js'
import { runJeden } from './runner.js'

function usage() {
  return `Usage:
  jeden run "task" [--cwd path] [--allow-write] [--max-steps n]

Environment:
  WISENT_APP_AGENT_AUTH_SECRET  required for model-router calls
  WISENT_APP_AGENT_ID           default: wisent-app
  MODEL_ROUTER_URL              default: production Wisent router
  JEDEN_MODEL                   default: claude-code-subscription
`
}

function parseArgs(argv) {
  const [command, ...rest] = argv
  if (!command || command === '-h' || command === '--help') return { help: true }
  if (command !== 'run') throw new Error(`unknown command: ${command}`)

  let task = ''
  let cwd = process.cwd()
  let allowWrite = false
  let maxSteps = 8

  for (let i = 0; i < rest.length; i += 1) {
    const arg = rest[i]
    if (arg === '--cwd') {
      cwd = rest[++i]
      if (!cwd) throw new Error('--cwd requires a value')
      continue
    }
    if (arg === '--allow-write') {
      allowWrite = true
      continue
    }
    if (arg === '--max-steps') {
      const raw = rest[++i]
      maxSteps = Number(raw)
      if (!Number.isInteger(maxSteps) || maxSteps < 1 || maxSteps > 32) throw new Error('--max-steps must be an integer between 1 and 32')
      continue
    }
    if (arg.startsWith('--')) throw new Error(`unknown option: ${arg}`)
    task = task ? `${task} ${arg}` : arg
  }

  if (!task.trim()) throw new Error('run requires a task')
  return { command, task, cwd, allowWrite, maxSteps }
}

async function main() {
  const args = parseArgs(process.argv.slice(2))
  if (args.help) {
    process.stdout.write(usage())
    return
  }
  loadEnvFiles({ dirs: [process.cwd(), args.cwd] })
  const result = await runJeden(args)
  process.stdout.write(`${result.text}\n`)
}

main().catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`)
  process.exit(1)
})
