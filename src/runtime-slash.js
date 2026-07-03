import { spawn } from 'node:child_process'
import { randomBytes } from 'node:crypto'
import { appendFile, mkdir, open, readFile, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { loadJedenConfig } from './config.js'
import { createNewSession, dropSessionAndCreateNew, resumeSessionIntoNew } from './session.js'

function ok(text) { return { handled: true, role: 'system', text } }
function err(text) { return { handled: true, role: 'error', text } }
function nowIso() { return new Date().toISOString() }
function lines(values) { return values.filter((value) => value !== null && value !== undefined && value !== '').join('\n') }

function splitArgs(value) {
  const args = []
  let current = ''
  let quote = null
  for (const char of String(value || '')) {
    if (quote) {
      if (char === quote) quote = null
      else current += char
      continue
    }
    if (char === '"' || char === "'") { quote = char; continue }
    if (/\s/.test(char)) {
      if (current) { args.push(current); current = '' }
      continue
    }
    current += char
  }
  if (current) args.push(current)
  return args
}

function localId(prefix) {
  return `${prefix}-${new Date().toISOString().replace(/[:.]/g, '-')}-${randomBytes(3).toString('hex')}`
}

function runtimeState(context) {
  const root = context.modeState || context.slashState || (context.slashState = {})
  root.slash ||= {}
  root.slash.jobs ||= []
  return root.slash
}

function cwdFor(context) {
  return resolve(context.args?.cwd || process.cwd())
}

function sessionRootFor(context) {
  return context.recorder?.path ? dirname(context.recorder.path()) : undefined
}

async function readJsonObject(file) {
  try {
    const parsed = JSON.parse(await readFile(file, 'utf8'))
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) return parsed
    return {}
  } catch (error) {
    if (error?.code === 'ENOENT') return {}
    throw error
  }
}

async function writeProjectConfig(cwd, patch) {
  const file = resolve(cwd, '.jeden', 'config.json')
  await mkdir(dirname(file), { recursive: true })
  const current = await readJsonObject(file)
  await writeFile(file, `${JSON.stringify({ ...current, ...patch }, null, 2)}\n`, 'utf8')
  return file
}

async function handleBrowser(parsed, context) {
  const [mode] = splitArgs(parsed.args)
  const cwd = cwdFor(context)
  const config = await loadJedenConfig({ cwd })
  const current = config.browserMode || config.browser?.mode || 'headless'
  if (!mode) return ok(`Browser mode preference: ${current}. Use /browser headless or /browser visible to persist a local preference.`)
  const modes = new Set(['headless', 'visible'])
  if (!modes.has(mode)) return err('Usage: /browser [headless|visible]')
  const file = await writeProjectConfig(cwd, { browserMode: mode })
  return ok(lines([
    `Browser mode preference set to ${mode}.`,
    `Config: ${file}`,
    'Local preference only: browser controller availability still depends on installed local tools or MCP adapters.',
  ]))
}

async function startNewInteractiveSession(context) {
  if (typeof context.setRecorder !== 'function') return null
  try {
    const created = await createNewSession({ root: sessionRootFor(context), cwd: cwdFor(context) })
    context.setRecorder(created.recorder)
    context.installPriorMessages?.([])
    return ok(lines([
      `Started new session: ${created.path}`,
      'Replay context cleared.',
    ]))
  } catch (error) {
    return err(`/new failed: ${error instanceof Error ? error.message : String(error)}`)
  }
}

async function rotateActiveSession(context, label) {
  if (!context.recorder?.path) return err(`${label} requires an active session recorder.`)
  if (typeof context.setRecorder !== 'function') return err(`${label} cannot replace the active recorder in this runtime context; use the interactive CLI or delete the session after exit.`)
  try {
    const previousPath = context.recorder.path()
    const rotated = await dropSessionAndCreateNew({ recorder: context.recorder, root: sessionRootFor(context), cwd: cwdFor(context) })
    context.setRecorder(rotated.recorder)
    context.installPriorMessages?.([])
    return ok(lines([
      `Deleted session: ${rotated.deleted.path || previousPath}`,
      `Started new session: ${rotated.path}`,
    ]))
  } catch (error) {
    return err(`${label} failed: ${error instanceof Error ? error.message : String(error)}`)
  }
}

async function resumeInteractiveSession(parsed, context) {
  const [idOrPath] = splitArgs(parsed.args)
  if (!idOrPath) return null
  if (typeof context.setRecorder !== 'function' || typeof context.installPriorMessages !== 'function') {
    return err('/resume cannot install replay context in this runtime context; use `jeden resume <session> "<task>"` from the CLI.')
  }
  try {
    const resumed = await resumeSessionIntoNew({ idOrPath, root: sessionRootFor(context), cwd: cwdFor(context) })
    context.setRecorder(resumed.recorder)
    context.installPriorMessages(resumed.priorMessages)
    return ok(lines([
      `Resumed session ${resumed.previous.id} into active interactive context.`,
      `Previous session: ${resumed.previous.path}`,
      `New recorder: ${resumed.path}`,
      `Replay messages installed: ${resumed.priorMessages.length}`,
    ]))
  } catch (error) {
    return err(`/resume failed: ${error instanceof Error ? error.message : String(error)}`)
  }
}

function tanArgs(task, context) {
  const args = context.args || {}
  const cliPath = fileURLToPath(new URL('./cli.js', import.meta.url))
  const out = [cliPath, 'run', task, '--cwd', cwdFor(context), '--json']
  if (args.model) out.push('--model', String(args.model))
  if (args.maxTokens) out.push('--max-tokens', String(args.maxTokens))
  if (args.maxSteps) out.push('--max-steps', String(args.maxSteps))
  if (args.allowWrite) out.push('--allow-write')
  if (args.allowCommand) out.push('--allow-command')
  if (args.selfRepair) out.push('--self-repair')
  if (args.selfRepairOwnCode) out.push('--self-repair-own-code')
  return out
}

async function writeJobMetadata(file, metadata) {
  await writeFile(file, `${JSON.stringify(metadata, null, 2)}\n`, 'utf8')
}

async function startTanJob(parsed, context) {
  const task = parsed.args.trim()
  if (!task) return err('Usage: /tan <work>')
  if (!context.recorder?.artifactDir) return err('/tan requires an active session artifact directory.')
  const jobId = localId('tan')
  const dir = join(context.recorder.artifactDir(), 'tan-jobs')
  await mkdir(dir, { recursive: true })
  const stdoutPath = join(dir, `${jobId}.stdout.log`)
  const stderrPath = join(dir, `${jobId}.stderr.log`)
  const metadataPath = join(dir, `${jobId}.json`)
  let stdoutHandle = null
  let stderrHandle = null
  const commandArgs = tanArgs(task, context)
  const metadata = {
    id: jobId,
    kind: 'tan',
    status: 'starting',
    task,
    cwd: cwdFor(context),
    startedAt: nowIso(),
    command: ['jeden', 'run', task],
    spawn: [process.execPath, ...commandArgs],
    stdoutPath,
    stderrPath,
    metadataPath,
    parentSession: context.recorder.path?.() || null,
  }
  try {
    stdoutHandle = await open(stdoutPath, 'a')
    stderrHandle = await open(stderrPath, 'a')
    const child = spawn(process.execPath, commandArgs, {
      cwd: cwdFor(context),
      detached: true,
      stdio: ['ignore', stdoutHandle.fd, stderrHandle.fd],
      env: process.env,
    })
    metadata.pid = child.pid
    metadata.status = 'running'
    await writeJobMetadata(metadataPath, metadata)
    child.once('exit', async (code, signal) => {
      const done = { ...metadata, status: code === 0 ? 'completed' : 'failed', exitCode: code, signal, finishedAt: nowIso() }
      try { await writeJobMetadata(metadataPath, done) } catch {}
    })
    child.once('error', async (error) => {
      const failed = { ...metadata, status: 'failed', error: error.message, finishedAt: nowIso() }
      try { await writeJobMetadata(metadataPath, failed) } catch {}
    })
    child.unref()
    runtimeState(context).jobs.push({ id: jobId, pid: child.pid, status: 'running', metadataPath, stdoutPath, stderrPath, task, startedAt: metadata.startedAt })
    return ok(lines([
      `Started detached tan job ${jobId}.`,
      `PID: ${child.pid}`,
      `Metadata: ${metadataPath}`,
      `Stdout: ${stdoutPath}`,
      `Stderr: ${stderrPath}`,
    ]))
  } catch (error) {
    metadata.status = 'failed'
    metadata.error = error instanceof Error ? error.message : String(error)
    metadata.finishedAt = nowIso()
    await writeJobMetadata(metadataPath, metadata)
    return err(`/tan failed: ${metadata.error}\nMetadata: ${metadataPath}`)
  } finally {
    if (stdoutHandle) await stdoutHandle.close().catch(() => {})
    if (stderrHandle) await stderrHandle.close().catch(() => {})
  }
}

async function appendLocalRule(parsed, context) {
  const complaint = parsed.args.trim()
  if (!complaint) return err('Usage: /omfg <complaint>')
  const file = resolve(cwdFor(context), '.jeden', 'rules.jsonl')
  await mkdir(dirname(file), { recursive: true })
  const record = {
    id: localId('rule'),
    kind: 'omfg-rule',
    createdAt: nowIso(),
    cwd: cwdFor(context),
    complaint,
    rule: `When this situation recurs, avoid the behavior described here: ${complaint}`,
    source: '/omfg',
  }
  await appendFile(file, `${JSON.stringify(record)}\n`, 'utf8')
  return ok(lines([
    `Appended local rule record ${record.id}.`,
    `Rules file: ${file}`,
  ]))
}

export async function handleRuntimeSlashCommand(canonical, parsed, context = {}) {
  if (canonical === 'browser') return handleBrowser(parsed, context)
  if (canonical === 'new') return startNewInteractiveSession(context)
  if (canonical === 'fresh') return ok('No persistent provider stream exists in Jeden to reset. Current local transcript is unchanged; use /new to start a new local session.')
  if (canonical === 'drop') return rotateActiveSession(context, '/drop')
  if (canonical === 'session') {
    const [verb] = splitArgs(parsed.args)
    if (verb === 'delete') return rotateActiveSession(context, '/session delete')
    return null
  }
  if (canonical === 'resume') return resumeInteractiveSession(parsed, context)
  if (canonical === 'tan') return startTanJob(parsed, context)
  if (canonical === 'omfg') return appendLocalRule(parsed, context)
  return null
}
