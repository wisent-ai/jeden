#!/usr/bin/env node

import { createInterface } from 'node:readline/promises'
import { stdin as input, stdout as output } from 'node:process'
import { writeFile } from 'node:fs/promises'
import { loadEnvFiles } from './env.js'
import { runJeden } from './runner.js'
import { SessionRecorder, listSessions, readSession, listSessionArtifacts, readSessionArtifact } from './session.js'
import { createSharedHookRunner } from './hooks.js'
import { createToolRegistry } from './tools.js'
import { applyConfigEnv, loadJedenConfig } from './config.js'
import { loadCustomTools } from './custom-tools.js'


function usage() {
  return `Usage:
  jeden [--cwd path] [--model name] [--allow-write] [--allow-command] [--max-steps n]
  jeden run "task" [--cwd path] [--model name] [--allow-write] [--allow-command] [--max-steps n]
  jeden resume <session-id-or-path> "task" [--cwd path] [--model name] [--allow-write] [--allow-command] [--max-steps n]
  jeden sessions [limit]
  jeden search-sessions <query> [limit]
  jeden show <session-id-or-path>
  jeden export <session-id-or-path> [output.json]
  jeden export <session-id-or-path> --html [output.html]
  jeden artifacts <session-id-or-path>
  jeden artifact <session-id-or-path> <name> [output]
  jeden tools [--cwd path]
  jeden config [--cwd path]
  jeden doctor [--cwd path]

Environment:
  WISENT_APP_AGENT_AUTH_SECRET  required for model-router calls
  WISENT_APP_AGENT_ID           default: wisent-app
  MODEL_ROUTER_URL              default: production Wisent router
  JEDEN_MODEL                   default: claude-code-subscription
`
}

function parseSharedOptions(rest) {
  let cwd = process.cwd()
  let allowWrite = false
  let allowCommand = false
  let maxSteps = 8
  let model = null
  const positionals = []

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
    if (arg === '--allow-command') {
      allowCommand = true
      continue
    }
    if (arg === '--model') {
      model = rest[++i]
      if (!model) throw new Error('--model requires a value')
      continue
    }
    if (arg === '--max-steps') {
      const raw = rest[++i]
      maxSteps = Number(raw)
      if (!Number.isInteger(maxSteps) || maxSteps < 1 || maxSteps > 32) throw new Error('--max-steps must be an integer between 1 and 32')
      continue
    }
    if (arg.slice(0, 2) === '--') throw new Error(`unknown option: ${arg}`)
    positionals.push(arg)
  }

  return { cwd, allowWrite, allowCommand, maxSteps, model, positionals }
}

function parseArgs(argv) {
  const [first, ...rest] = argv
  if (first === '-h' || first === '--help') return { help: true }
  if (!first) return { command: 'interactive', ...parseSharedOptions([]) }
  if (first === 'run') {
    const parsed = parseSharedOptions(rest)
    const task = parsed.positionals.join(' ').trim()
    if (!task) throw new Error('run requires a task')
    return { command: 'run', task, ...parsed }
  }
  if (first === 'resume') {
    const idOrPath = rest[0]
    if (!idOrPath) throw new Error('resume requires a session id or path')
    const parsed = parseSharedOptions(rest.slice(1))
    const task = parsed.positionals.join(' ').trim()
    if (!task) throw new Error('resume requires a task')
    return { command: 'resume', idOrPath, task, ...parsed }
  }
  if (first === 'sessions') {
    const limit = rest[0] ? Number(rest[0]) : 20
    if (!Number.isInteger(limit) || limit < 1 || limit > 200) throw new Error('sessions limit must be 1..200')
    return { command: 'sessions', limit }
  }
  if (first === 'search-sessions') {
    const query = rest[0]
    if (!query) throw new Error('search-sessions requires a query')
    const limit = rest[1] ? Number(rest[1]) : 50
    if (!Number.isInteger(limit) || limit < 1 || limit > 200) throw new Error('search-sessions limit must be 1..200')
    return { command: 'search-sessions', query, limit }
  }
  if (first === 'show') {
    const idOrPath = rest[0]
    if (!idOrPath) throw new Error('show requires a session id or path')
    return { command: 'show', idOrPath }
  }
  if (first === 'export') {
    const idOrPath = rest[0]
    if (!idOrPath) throw new Error('export requires a session id or path')
    const outputPath = rest[1] === '--html' ? rest[2] || null : rest[1] || null
    const format = rest[1] === '--html' ? 'html' : 'json'
    return { command: 'export', idOrPath, outputPath, format }
  }
  if (first === 'artifacts') {
    const idOrPath = rest[0]
    if (!idOrPath) throw new Error('artifacts requires a session id or path')
    return { command: 'artifacts', idOrPath }
  }
  if (first === 'artifact') {
    const idOrPath = rest[0]
    const name = rest[1]
    if (!idOrPath || !name) throw new Error('artifact requires a session id or path and artifact name')
    return { command: 'artifact', idOrPath, name, outputPath: rest[2] || null }
  }
  if (first === 'tools') {
    return { command: 'tools', ...parseSharedOptions(rest) }
  }
  if (first === 'config') {
    return { command: 'config', ...parseSharedOptions(rest) }
  }
  if (first === 'doctor') {
    return { command: 'doctor', ...parseSharedOptions(rest) }
  }
  if (first.slice(0, 2) === '--') return { command: 'interactive', ...parseSharedOptions(argv) }
  throw new Error(`unknown command: ${first}`)
}

async function loadRuntimeEnv(args) {
  loadEnvFiles({ dirs: [process.cwd(), args.cwd] })
  const config = await loadJedenConfig({ cwd: args.cwd || process.cwd() })
  applyConfigEnv(config)
  if (args.model) process.env.JEDEN_MODEL = String(args.model)
  return config
}
async function runUserPromptHook({ hookRunner, task, cwd, recorder }) {
  const result = await hookRunner.run('user_prompt_submit', {
    runtime: 'jeden',
    project_dir: cwd,
    user_message: task,
  })
  await recorder?.record('hook', { event: 'user_prompt_submit', result })
  if (result.decision === 'block') throw new Error(`user prompt hook blocked: ${result.reason}`)
}


async function runOnce(args) {
  const recorder = new SessionRecorder({ cwd: args.cwd })
  const hookRunner = createSharedHookRunner()
  await runUserPromptHook({ hookRunner, task: args.task, cwd: args.cwd, recorder })
  const result = await runJeden({ ...args, recorder, hookRunner })
  process.stdout.write(`${result.text}\n`)
  process.stderr.write(`[session] ${result.sessionPath}\n`)
}

function sessionContext(session) {
  const parts = []
  for (const event of session.events) {
    if (event.type === 'user') parts.push(`User: ${event.data?.task || ''}`)
    else if (event.type === 'final') parts.push(`Assistant: ${event.data?.text || ''}`)
    else if (event.type === 'tool_result') parts.push(`Tool ${event.data?.tool || ''}: ${JSON.stringify(event.data?.result || {})}`)
  }
  return parts.slice(-40).join('\n\n')
}

async function runResume(args) {
  const previous = await readSession({ idOrPath: args.idOrPath })
  const recorder = new SessionRecorder({ cwd: args.cwd })
  const hookRunner = createSharedHookRunner()
  await runUserPromptHook({ hookRunner, task: args.task, cwd: args.cwd, recorder })
  await recorder.record('resumed_from', { id: previous.id, path: previous.path })
  const result = await runJeden({ ...args, recorder, hookRunner, priorContext: sessionContext(previous) })
  process.stdout.write(`${result.text}\n`)
  process.stderr.write(`[session] ${result.sessionPath}\n`)
}

function approvalPrompt({ tool, kind, input }) {
  const payload = JSON.stringify(input, null, 2)
  const preview = payload.length > 1200 ? `${payload.slice(0, 1200)}…` : payload
  return `\nApproval required for ${kind} tool ${tool}:\n${preview}\nApprove? [y/N] `
}


async function runInteractive(args) {
  const recorder = new SessionRecorder({ cwd: args.cwd })
  await recorder.ensure()
  process.stdout.write(`Jeden session: ${recorder.path()}\n`)
  process.stdout.write(`cwd: ${args.cwd}\n`)
  process.stdout.write(`write: ${args.allowWrite ? 'enabled' : 'disabled'}, command: ${args.allowCommand ? 'enabled' : 'disabled'}\n`)
  process.stdout.write('Type /exit to quit.\n\n')

  const hookRunner = createSharedHookRunner()
  const rl = createInterface({ input, output })
  const approveTool = async (request) => {
    const answer = (await rl.question(approvalPrompt(request))).trim().toLowerCase()
    return answer === 'y' || answer === 'yes'
  }
  const askUser = async ({ question, options }) => {
    const suffix = options.length > 0 ? ` (${options.join('/')})` : ''
    return (await rl.question(`${question}${suffix}\n> `)).trim()
  }

  try {
    for (;;) {
      const task = (await rl.question('jeden> ')).trim()
      if (!task) continue
      if (task === '/exit' || task === '/quit') break
      try {
        await runUserPromptHook({ hookRunner, task, cwd: args.cwd, recorder })
        const result = await runJeden({ ...args, task, recorder, approveTool, askUser, hookRunner })
        process.stdout.write(`${result.text}\n`)
      } catch (error) {
        process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`)
        await recorder.record('error', { message: error instanceof Error ? error.message : String(error) })
      }
    }
  } finally {
    rl.close()
  }
}

async function showSessions(limit) {
  const sessions = await listSessions({ limit })
  for (const session of sessions) {
    process.stdout.write(`${session.id}\t${session.updatedAt || '-'}\t${session.cwd || '-'}\n`)
  }
}

async function showSession(idOrPath) {
  const session = await readSession({ idOrPath })
  for (const event of session.events) {
    const label = `${event.ts || ''} ${event.type || ''}`.trim()
    let body = ''
    if (event.type === 'user') body = event.data?.task || ''
    else if (event.type === 'final') body = event.data?.text || ''
    else if (event.type === 'tool_result') body = `${event.data?.tool || ''} ${JSON.stringify(event.data?.result || {})}`
    else if (event.type === 'hook') body = `${event.data?.event || ''} ${JSON.stringify(event.data?.result || {})}`
    else body = JSON.stringify(event.data || {})
    process.stdout.write(`${label}\n${body}\n\n`)
  }
}

function sessionEventText(event) {
  return JSON.stringify(event.data || {})
}

async function searchSessions(query, limit) {
  const needle = String(query).toLowerCase()
  const sessions = await listSessions({ limit })
  for (const row of sessions) {
    let session
    try {
      session = await readSession({ idOrPath: row.path })
    } catch {
      continue
    }
    for (const event of session.events) {
      const text = sessionEventText(event)
      const at = text.toLowerCase().indexOf(needle)
      if (at === -1) continue
      const start = Math.max(at - 80, 0)
      const snippet = text.slice(start, at + needle.length + 160).replace(/\s+/g, ' ')
      process.stdout.write(`${session.id}\t${event.ts || ''}\t${event.type || ''}\t${snippet}\n`)
      break
    }
  }
}

function htmlEscape(value) {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

function renderSessionHtml(session) {
  const events = session.events.map((event) => {
    const label = htmlEscape(`${event.ts || ''} ${event.type || ''}`.trim())
    const body = htmlEscape(JSON.stringify(event.data || {}, null, 2))
    return `<section class="event"><h2>${label}</h2><pre>${body}</pre></section>`
  }).join('\n')
  return `<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <title>Jeden session ${htmlEscape(session.id)}</title>
  <style>
    body { font-family: ui-sans-serif, system-ui, sans-serif; margin: 2rem; background: #fafafa; color: #111; }
    .event { border: 1px solid #ddd; border-radius: 8px; background: white; margin: 1rem 0; padding: 1rem; }
    h1 { margin-bottom: 0; }
    h2 { font-size: 0.9rem; color: #555; margin-top: 0; }
    pre { white-space: pre-wrap; overflow-wrap: anywhere; }
  </style>
</head>
<body>
  <h1>Jeden session ${htmlEscape(session.id)}</h1>
  <p>${htmlEscape(session.path)}</p>
  ${events}
</body>
</html>
`
}

async function exportSession(idOrPath, outputPath, format = 'json') {
  const session = await readSession({ idOrPath })
  const payload = format === 'html' ? renderSessionHtml(session) : `${JSON.stringify(session, null, 2)}\n`
  if (outputPath) {
    await writeFile(outputPath, payload, 'utf8')
    process.stdout.write(`${outputPath}\n`)
  } else {
    process.stdout.write(payload)
  }
}

async function showArtifacts(idOrPath) {
  const listing = await listSessionArtifacts({ idOrPath })
  for (const artifact of listing.artifacts) {
    process.stdout.write(`${artifact.name}\t${artifact.bytes}\t${artifact.updatedAt}\n`)
  }
}

async function showArtifact(idOrPath, name, outputPath) {
  const artifact = await readSessionArtifact({ idOrPath, name })
  if (outputPath) {
    await writeFile(outputPath, artifact.content, 'utf8')
    process.stdout.write(`${outputPath}\n`)
  } else {
    process.stdout.write(artifact.content)
    if (!artifact.content.endsWith('\n')) process.stdout.write('\n')
  }
}

async function showTools(args) {
  const builtIn = createToolRegistry({ cwd: args.cwd }).list()
  const custom = await loadCustomTools({ cwd: args.cwd, builtInToolNames: builtIn.map((tool) => tool.name) })
  const tools = createToolRegistry({ cwd: args.cwd, customTools: custom.tools }).list()
  for (const tool of tools) {
    process.stdout.write(`${tool.name}\t${tool.description}\n`)
  }
  for (const error of custom.errors) {
    process.stderr.write(`custom tool skipped: ${error.path}: ${error.error}\n`)
  }
}

async function showConfig(args) {
  const config = await loadJedenConfig({ cwd: args.cwd })
  process.stdout.write(`${JSON.stringify(config, null, 2)}\n`)
}

async function showDoctor(args) {
  const config = await loadJedenConfig({ cwd: args.cwd })
  const builtIn = createToolRegistry({ cwd: args.cwd }).list()
  const custom = await loadCustomTools({ cwd: args.cwd, builtInToolNames: builtIn.map((tool) => tool.name) })
  const report = {
    ok: custom.errors.length === 0,
    cwd: args.cwd,
    node: process.version,
    model: process.env.JEDEN_MODEL || config.model,
    modelRouterUrl: config.modelRouterUrl,
    agentId: config.agentId,
    env: {
      hasAuthSecret: Boolean(process.env.WISENT_APP_AGENT_AUTH_SECRET),
      hasModelRouterUrl: Boolean(process.env.MODEL_ROUTER_URL),
      hasAgentId: Boolean(process.env.WISENT_APP_AGENT_ID),
    },
    tools: {
      builtIn: builtIn.length,
      custom: custom.tools.length,
      errors: custom.errors,
    },
  }
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`)
}



async function main() {
  const args = parseArgs(process.argv.slice(2))
  if (args.command === 'search-sessions') {
    await searchSessions(args.query, args.limit)
    return
  }
  if (args.help) {
    process.stdout.write(usage())
    return
  }
  await loadRuntimeEnv(args)
  if (args.command === 'sessions') {
    await showSessions(args.limit)
    return
  }
  if (args.command === 'show') {
    await showSession(args.idOrPath)
    return
  }
  if (args.command === 'export') {
    await exportSession(args.idOrPath, args.outputPath, args.format)
    return
  }
  if (args.command === 'artifacts') {
    await showArtifacts(args.idOrPath)
    return
  }
  if (args.command === 'artifact') {
    await showArtifact(args.idOrPath, args.name, args.outputPath)
    return
  }
  if (args.command === 'config') {
    await showConfig(args)
    return
  }
  if (args.command === 'doctor') {
    await showDoctor(args)
    return
  }
  if (args.command === 'tools') {
    await showTools(args)
    return
  }
  if (args.command === 'resume') {
    await runResume(args)
    return
  }
  if (args.command === 'run') {
    await runOnce(args)
    return
  }
  await runInteractive(args)
}

main().catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`)
  process.exit(1)
})
