import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'

function nowIso() { return new Date().toISOString() }

export function usagePath({ cwd = process.cwd() } = {}) {
  return join(resolve(cwd), '.jeden', 'usage.json')
}

async function readUsage(file) {
  try {
    return JSON.parse(await readFile(file, 'utf8'))
  } catch (error) {
    if (error?.code === 'ENOENT') return { version: 1, events: [] }
    throw error
  }
}

async function writeUsage(file, usage) {
  await mkdir(dirname(file), { recursive: true })
  await writeFile(file, `${JSON.stringify(usage, null, 2)}\n`, 'utf8')
  return file
}

function estimateTokens(value) {
  const text = typeof value === 'string' ? value : JSON.stringify(value || '')
  return Math.max(1, Math.ceil(text.length / 4))
}

export async function recordUsageEvent({ cwd = process.cwd(), model = 'default', serviceTier = '', messages = [], output = '', step = 0 } = {}) {
  const file = usagePath({ cwd })
  const usage = await readUsage(file)
  const inputTokens = estimateTokens(messages)
  const outputTokens = estimateTokens(output)
  usage.events ||= []
  usage.events.push({ at: nowIso(), model, serviceTier: serviceTier || null, step, inputTokens, outputTokens, totalTokens: inputTokens + outputTokens })
  usage.updatedAt = nowIso()
  await writeUsage(file, usage)
  return { file, event: usage.events[usage.events.length - 1] }
}

export async function usageSummary({ cwd = process.cwd() } = {}) {
  const file = usagePath({ cwd })
  const usage = await readUsage(file)
  const events = Array.isArray(usage.events) ? usage.events : []
  const totals = events.reduce((acc, event) => {
    acc.inputTokens += Number(event.inputTokens) || 0
    acc.outputTokens += Number(event.outputTokens) || 0
    acc.totalTokens += Number(event.totalTokens) || 0
    const model = event.model || 'default'
    acc.byModel[model] ||= { calls: 0, inputTokens: 0, outputTokens: 0, totalTokens: 0 }
    acc.byModel[model].calls += 1
    acc.byModel[model].inputTokens += Number(event.inputTokens) || 0
    acc.byModel[model].outputTokens += Number(event.outputTokens) || 0
    acc.byModel[model].totalTokens += Number(event.totalTokens) || 0
    return acc
  }, { calls: events.length, inputTokens: 0, outputTokens: 0, totalTokens: 0, byModel: {} })
  return { file, updatedAt: usage.updatedAt || null, totals, recent: events.slice(-10) }
}

export async function resetUsage({ cwd = process.cwd() } = {}) {
  const file = usagePath({ cwd })
  await writeUsage(file, { version: 1, updatedAt: nowIso(), events: [] })
  return { file }
}
