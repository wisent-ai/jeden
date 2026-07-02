import { access, mkdir, rm, writeFile, readFile } from 'node:fs/promises'
import { constants } from 'node:fs'
import { execFile } from 'node:child_process'
import { homedir } from 'node:os'
import { dirname, join, resolve } from 'node:path'

import { loadJedenConfig } from './config.js'
import { loadCustomTools } from './custom-tools.js'
import { modelRouterConfig } from './model-router.js'
import { closeMcpClients, loadMcpToolAdapters } from './mcp.js'
import { defaultSessionRoot } from './session.js'
import { createToolRegistry } from './tools.js'

async function packageVersion() {
  try {
    const raw = await readFile(new URL('../package.json', import.meta.url), 'utf8')
    return JSON.parse(raw).version || null
  } catch {
    return null
  }
}

function check(id, ok, details = {}, fatal = false) {
  return { id, ok: Boolean(ok), fatal: Boolean(fatal), ...details }
}

async function commandAvailable(command, args = ['--version']) {
  return await new Promise((resolvePromise) => {
    execFile(command, args, { timeout: 3000 }, (error, stdout, stderr) => {
      resolvePromise({ ok: !error, stdout: String(stdout || '').trim(), stderr: String(stderr || '').trim(), code: error?.code ?? 0 })
    })
  })
}

async function writableDirectoryProbe(dir) {
  const root = resolve(dir)
  const probe = join(root, `.jeden-doctor-${process.pid}-${Date.now()}`)
  try {
    await mkdir(root, { recursive: true })
    await writeFile(probe, 'ok', 'utf8')
    await rm(probe, { force: true })
    return { ok: true, path: root }
  } catch (error) {
    return { ok: false, path: root, error: error.message }
  }
}

async function readableDirectoryProbe(dir) {
  const root = resolve(dir)
  try {
    await access(root, constants.R_OK)
    return { ok: true, path: root }
  } catch (error) {
    return { ok: false, path: root, error: error.message }
  }
}

function memoryFilePath() {
  return process.env.JEDEN_MEMORY_FILE ? resolve(process.env.JEDEN_MEMORY_FILE) : join(homedir(), '.jeden', 'memory.jsonl')
}

async function loadToolSurface({ cwd }) {
  const builtIn = createToolRegistry({ cwd }).list()
  const custom = await loadCustomTools({ cwd, builtInToolNames: builtIn.map((tool) => tool.name) })
  const mcp = await loadMcpToolAdapters({ cwd })
  const tools = createToolRegistry({ cwd, customTools: [...custom.tools, ...mcp.tools] }).list()
  return { builtIn, custom, mcp, tools }
}

export async function buildCapabilityManifest({ cwd = process.cwd() } = {}) {
  const root = resolve(cwd)
  try {
    const config = await loadJedenConfig({ cwd: root })
    const { builtIn, custom, mcp, tools } = await loadToolSurface({ cwd: root })
    const route = modelRouterConfig({ ...process.env, MODEL_ROUTER_URL: process.env.MODEL_ROUTER_URL || config.modelRouterUrl, WISENT_APP_AGENT_ID: process.env.WISENT_APP_AGENT_ID || config.agentId, JEDEN_MODEL: process.env.JEDEN_MODEL || config.model })
    return {
      version: await packageVersion(),
      cwd: root,
      model: route.model,
      modelRouterUrl: route.url,
      agentId: route.agentId,
      tools: {
        total: tools.length,
        builtIn: builtIn.map((tool) => ({ name: tool.name, description: tool.description, input: tool.input || {} })),
        custom: custom.tools.map((tool) => ({ name: tool.name, description: tool.description, input: tool.input || {} })),
        mcp: mcp.tools.map((tool) => ({ name: tool.name, description: tool.description, input: tool.input || {} })),
        all: tools.map((tool) => ({ name: tool.name, description: tool.description, input: tool.input || {} })),
      },
      errors: {
        custom: custom.errors,
        mcp: mcp.errors,
      },
    }
  } finally {
    await closeMcpClients({ cwd: root })
  }
}

export async function buildDoctorReport({ cwd = process.cwd() } = {}) {
  const root = resolve(cwd)
  try {
    const config = await loadJedenConfig({ cwd: root })
    const route = modelRouterConfig({ ...process.env, MODEL_ROUTER_URL: process.env.MODEL_ROUTER_URL || config.modelRouterUrl, WISENT_APP_AGENT_ID: process.env.WISENT_APP_AGENT_ID || config.agentId, JEDEN_MODEL: process.env.JEDEN_MODEL || config.model })
    const { builtIn, custom, mcp, tools } = await loadToolSurface({ cwd: root })
    const sessionProbe = await writableDirectoryProbe(defaultSessionRoot())
    const memoryProbe = await writableDirectoryProbe(dirname(memoryFilePath()))
    const cwdProbe = await readableDirectoryProbe(root)
    const nodeCheck = check('runtime.node', Number(process.versions.node.split('.')[0]) >= 20, { version: process.version }, true)
    const dependencies = {
      git: await commandAvailable('git'),
      npm: await commandAvailable('npm'),
      python3: await commandAvailable('python3'),
      sqlite3: await commandAvailable('sqlite3'),
    }
    const checks = [
      nodeCheck,
      check('filesystem.cwd.readable', cwdProbe.ok, cwdProbe, true),
      check('filesystem.sessions.writable', sessionProbe.ok, sessionProbe, true),
      check('filesystem.memoryParent.writable', memoryProbe.ok, memoryProbe, true),
      check('config.model.resolved', Boolean(route.model), { model: route.model }, true),
      check('config.modelRouterUrl.resolved', Boolean(route.url), { url: route.url }, true),
      check('config.agentId.resolved', Boolean(route.agentId), { agentId: route.agentId }, true),
      check('secret.modelRouterAuth.present', Boolean(process.env.WISENT_APP_AGENT_AUTH_SECRET), { present: Boolean(process.env.WISENT_APP_AGENT_AUTH_SECRET) }, true),
      check('tools.custom.load', custom.errors.length === 0, { errors: custom.errors }, custom.errors.length > 0),
      check('tools.mcp.load', mcp.errors.length === 0, { errors: mcp.errors }, mcp.errors.length > 0),
      check('dependency.git.available', dependencies.git.ok, dependencies.git, false),
      check('dependency.npm.available', dependencies.npm.ok, dependencies.npm, false),
      check('dependency.python3.available', dependencies.python3.ok, dependencies.python3, false),
      check('dependency.sqlite3.available', dependencies.sqlite3.ok, dependencies.sqlite3, false),
    ]
    return {
      ok: checks.every((item) => item.ok || !item.fatal),
      version: await packageVersion(),
      cwd: root,
      node: process.version,
      model: route.model,
      modelRouterUrl: route.url,
      agentId: route.agentId,
      checks,
      tools: {
        builtIn: builtIn.length,
        custom: custom.tools.length,
        mcp: mcp.tools.length,
        total: tools.length,
      },
    }
  } finally {
    await closeMcpClients({ cwd: root })
  }
}
