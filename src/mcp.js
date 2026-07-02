import { readFile } from 'node:fs/promises'
import { spawn } from 'node:child_process'
import { homedir } from 'node:os'
import { join, resolve } from 'node:path'

const MCP_PROTOCOL_VERSION = '2024-11-05'

async function readJson(file) {
  try {
    return JSON.parse(await readFile(file, 'utf8'))
  } catch (error) {
    if (error?.code === 'ENOENT') return {}
    throw error
  }
}

export async function loadMcpConfig({ cwd = process.cwd() } = {}) {
  const user = await readJson(join(homedir(), '.jeden', 'mcp.json'))
  const project = await readJson(resolve(cwd, '.jeden', 'mcp.json'))
  return {
    mcpServers: {
      ...(user.mcpServers || {}),
      ...(project.mcpServers || {}),
    },
    disabledServers: [...(user.disabledServers || []), ...(project.disabledServers || [])],
  }
}

function encodeMessage(message) {
  const body = Buffer.from(JSON.stringify(message), 'utf8')
  return Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, 'utf8'), body])
}

function parseMessages(state, chunk) {
  state.buffer = Buffer.concat([state.buffer, chunk])
  const messages = []
  for (;;) {
    const headerEnd = state.buffer.indexOf('\r\n\r\n')
    if (headerEnd === -1) break
    const header = state.buffer.subarray(0, headerEnd).toString('utf8')
    const prefix = 'Content-Length:'
    const line = header.split('\r\n').find((item) => item.slice(0, prefix.length).toLowerCase() === prefix.toLowerCase())
    if (!line) throw new Error('MCP response missing Content-Length')
    const length = Number(line.slice(prefix.length).trim())
    if (!Number.isInteger(length) || length < 0) throw new Error('invalid MCP Content-Length')
    const bodyStart = headerEnd + 4
    const bodyEnd = bodyStart + length
    if (state.buffer.length < bodyEnd) break
    const body = state.buffer.subarray(bodyStart, bodyEnd).toString('utf8')
    messages.push(JSON.parse(body))
    state.buffer = state.buffer.subarray(bodyEnd)
  }
  return messages
}

function startServer(server, cwd) {
  if (!server || typeof server !== 'object') throw new Error('server config is required')
  if ((server.type || 'stdio') !== 'stdio') throw new Error('only stdio MCP servers are supported')
  if (!server.command || typeof server.command !== 'string') throw new Error('server.command is required')
  const args = Array.isArray(server.args) ? server.args.map((item) => String(item)) : []
  const serverCwd = server.cwd ? resolve(cwd, String(server.cwd)) : resolve(cwd)
  return spawn(server.command, args, { cwd: serverCwd, env: { ...process.env, ...(server.env || {}) }, stdio: ['pipe', 'pipe', 'pipe'] })
}

export async function withMcpServer({ server, cwd = process.cwd(), timeoutMs = 30_000 }, callback) {
  const child = startServer(server, cwd)
  const state = { buffer: Buffer.alloc(0), nextId: 1, pending: new Map(), stderr: '' }
  function rejectPending(error) {
    for (const pending of state.pending.values()) pending.reject(error)
    state.pending.clear()
  }

  let closed = false
  let timedOut = false
  let exited = false
  const timer = setTimeout(() => {
    timedOut = true
    const error = new Error(`MCP server timed out after ${timeoutMs}ms`)
    rejectPending(error)
    child.kill('SIGTERM')
    setTimeout(() => {
      if (!exited) child.kill('SIGKILL')
    }, 1_000)
  }, timeoutMs)

  child.stderr.on('data', (chunk) => {
    state.stderr += chunk.toString('utf8')
    if (state.stderr.length > 100_000) state.stderr = state.stderr.slice(0, 100_000)
  })
  child.stdout.on('data', (chunk) => {
    for (const message of parseMessages(state, chunk)) {
      if (Object.prototype.hasOwnProperty.call(message, 'id') && state.pending.has(message.id)) {
        const pending = state.pending.get(message.id)
        state.pending.delete(message.id)
        pending.resolve(message)
      }
    }
  })
  child.on('close', () => {
    exited = true
    closed = true
    rejectPending(new Error(timedOut ? `MCP server timed out after ${timeoutMs}ms` : 'MCP server closed'))
  })
  child.on('error', (error) => {
    exited = true
    closed = true
    rejectPending(error)
  })

  function sendRaw(message) {
    child.stdin.write(encodeMessage(message))
  }
  function request(method, params = {}) {
    if (closed || timedOut) throw new Error(timedOut ? `MCP server timed out after ${timeoutMs}ms` : 'MCP server is closed')
    const id = state.nextId
    state.nextId += 1
    const message = { jsonrpc: '2.0', id, method, params }
    return new Promise((resolvePromise, rejectPromise) => {
      state.pending.set(id, { resolve: resolvePromise, reject: rejectPromise })
      sendRaw(message)
    }).then((response) => {
      if (response.error) throw new Error(response.error.message || JSON.stringify(response.error))
      return response.result
    })
  }
  function notify(method, params = {}) {
    sendRaw({ jsonrpc: '2.0', method, params })
  }

  try {
    await request('initialize', {
      protocolVersion: MCP_PROTOCOL_VERSION,
      capabilities: {},
      clientInfo: { name: 'jeden', version: '0.1.0' },
    })
    notify('notifications/initialized')
    return await callback({ request, notify, stderr: () => state.stderr })
  } finally {
    clearTimeout(timer)
    child.kill('SIGTERM')
  }
}

async function configuredServer({ cwd, serverName }) {
  const config = await loadMcpConfig({ cwd })
  const disabled = new Set(config.disabledServers || [])
  if (disabled.has(serverName)) throw new Error(`disabled MCP server: ${serverName}`)
  const server = config.mcpServers?.[serverName]
  if (!server) throw new Error(`unknown MCP server: ${serverName}`)
  return server
}

export async function listMcpTools({ cwd = process.cwd(), serverName, timeoutMs } = {}) {
  const server = await configuredServer({ cwd, serverName })
  return withMcpServer({ server, cwd, timeoutMs }, async (client) => client.request('tools/list'))
}

export async function callMcpTool({ cwd = process.cwd(), serverName, toolName, args = {}, timeoutMs } = {}) {
  const server = await configuredServer({ cwd, serverName })
  if (!toolName) throw new Error('toolName is required')
  return withMcpServer({ server, cwd, timeoutMs }, async (client) => client.request('tools/call', { name: toolName, arguments: args }))
}

export async function listMcpResources({ cwd = process.cwd(), serverName, timeoutMs } = {}) {
  const server = await configuredServer({ cwd, serverName })
  return withMcpServer({ server, cwd, timeoutMs }, async (client) => client.request('resources/list'))
}

export async function readMcpResource({ cwd = process.cwd(), serverName, uri, timeoutMs } = {}) {
  const server = await configuredServer({ cwd, serverName })
  if (!uri) throw new Error('uri is required')
  return withMcpServer({ server, cwd, timeoutMs }, async (client) => client.request('resources/read', { uri }))
}

export async function listMcpPrompts({ cwd = process.cwd(), serverName, timeoutMs } = {}) {
  const server = await configuredServer({ cwd, serverName })
  return withMcpServer({ server, cwd, timeoutMs }, async (client) => client.request('prompts/list'))
}

export async function getMcpPrompt({ cwd = process.cwd(), serverName, name, args = {}, timeoutMs } = {}) {
  const server = await configuredServer({ cwd, serverName })
  if (!name) throw new Error('name is required')
  return withMcpServer({ server, cwd, timeoutMs }, async (client) => client.request('prompts/get', { name, arguments: args }))
}
