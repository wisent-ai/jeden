import { spawn } from 'node:child_process'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { toolCapability } from './tools.js'

function runHookProcess({ runnerPath, event, payload, timeoutMs }) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [runnerPath, event], { env: process.env })
    let stdout = ''
    let stderr = ''
    let timedOut = false
    const timer = setTimeout(() => {
      timedOut = true
      child.kill('SIGTERM')
    }, timeoutMs)
    child.stdout.on('data', (chunk) => { stdout += chunk.toString('utf8') })
    child.stderr.on('data', (chunk) => { stderr += chunk.toString('utf8') })
    child.on('close', (code, signal) => {
      clearTimeout(timer)
      resolve({ code, signal, timedOut, stdout, stderr })
    })
    child.stdin.end(JSON.stringify(payload || {}))
  })
}

function parseDecision(result, event) {
  if (result.timedOut) return { decision: 'block', reason: `${event} hook runner timed out` }
  const raw = String(result.stdout || '').trim()
  if (!raw) {
    if (result.code) return { decision: 'block', reason: result.stderr || `${event} hook runner exited ${result.code}` }
    return { decision: 'pass' }
  }
  try {
    const parsed = JSON.parse(raw)
    if (parsed?.decision === 'block') return { decision: 'block', reason: parsed.reason || `${event} blocked`, raw: parsed }
    return { decision: 'pass', raw: parsed }
  } catch {
    if (result.code) return { decision: 'block', reason: result.stderr || raw }
    return { decision: 'pass', raw }
  }
}

export function createSharedHookRunner({
  runnerPath = process.env.JEDEN_HOOK_RUNNER || join(homedir(), '.shared-hooks', 'run-hook.mjs'),
  timeoutMs = 45_000,
  enabled = process.env.JEDEN_HOOKS !== '0',
} = {}) {
  return {
    async run(event, payload) {
      if (!enabled) return { decision: 'pass', disabled: true }
      const result = await runHookProcess({ runnerPath, event, payload, timeoutMs })
      return parseDecision(result, event)
    },
  }
}

export function toolHookEvent(tool) {
  return `pre_tool_use:${toolCapability(tool).hook || 'read'}`
}

export function postToolHookEvent(tool) {
  const hook = toolCapability(tool).postHook
  return hook ? `post_tool_use:${hook}` : null
}
