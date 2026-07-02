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

function normalizedDecision(parsed, fallback = 'pass') {
  const decision = parsed?.decision === 'block' ? 'block' : fallback
  const out = { decision, raw: parsed }
  if (parsed?.reason) out.reason = String(parsed.reason)
  if (parsed?.message) out.message = String(parsed.message)
  if (parsed?.userMessage) out.userMessage = String(parsed.userMessage)
  if (parsed?.additionalContext) out.additionalContext = String(parsed.additionalContext)
  if (parsed?.toolInput && typeof parsed.toolInput === 'object' && !Array.isArray(parsed.toolInput)) out.toolInput = parsed.toolInput
  return out
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
    const normalized = normalizedDecision(parsed)
    if (normalized.decision === 'block') return { ...normalized, reason: normalized.reason || `${event} blocked` }
    return normalized
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
