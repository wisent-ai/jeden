import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { spawn } from 'node:child_process'
import { homedir } from 'node:os'
import { dirname, join, resolve } from 'node:path'

const MCP_PROTOCOL_VERSION = '2024-11-05'

function localHomeDir() {
  return process.env.HOME || homedir()
}


async function readJson(file) {
  try {
    return JSON.parse(await readFile(file, 'utf8'))
  } catch (error) {
    if (error?.code === 'ENOENT') return {}
    throw error
  }
}
export function projectMcpConfigPath({ cwd = process.cwd() } = {}) {
  return resolve(cwd, '.jeden', 'mcp.json')
}

export async function loadProjectMcpConfig({ cwd = process.cwd() } = {}) {
  return readJson(projectMcpConfigPath({ cwd }))
}

export async function saveProjectMcpConfig(config, { cwd = process.cwd() } = {}) {
  const file = projectMcpConfigPath({ cwd })
  await mkdir(dirname(file), { recursive: true })
  await writeFile(file, `${JSON.stringify(config || {}, null, 2)}\n`, 'utf8')
  return file
}


export async function loadMcpConfig({ cwd = process.cwd() } = {}) {
  const user = await readJson(join(localHomeDir(), '.jeden', 'mcp.json'))
  const project = await readJson(resolve(cwd, '.jeden', 'mcp.json'))
  return {
    mcpServers: {
      ...(user.mcpServers || {}),
      ...(project.mcpServers || {}),
    },
    disabledServers: [...(user.disabledServers || []), ...(project.disabledServers || [])],
  }
}

function nativeMcpToolName(serverName, toolName) {
  const server = String(serverName).replace(/[^a-zA-Z0-9_]/g, '_')
  const tool = String(toolName).replace(/[^a-zA-Z0-9_]/g, '_')
  return `mcp__${server}__${tool}`
}

function normalizeInputSchema(schema) {
  if (!schema || typeof schema !== 'object') return {}
  if (schema.type === 'object' && schema.properties && typeof schema.properties === 'object') return schema
  return { type: 'object', properties: {}, additionalProperties: true }
}

export async function loadMcpToolAdapters({ cwd = process.cwd(), timeoutMs = 30_000 } = {}) {
  const config = await loadMcpConfig({ cwd })
  const disabled = new Set(config.disabledServers || [])
  const tools = []
  const errors = []
  const used = new Set()
  for (const serverName of Object.keys(config.mcpServers || {})) {
    if (disabled.has(serverName)) continue
    try {
      const listed = await listMcpTools({ cwd, serverName, timeoutMs })
      for (const tool of listed.tools || []) {
        if (!tool?.name) continue
        const nativeName = nativeMcpToolName(serverName, tool.name)
        if (used.has(nativeName)) {
          errors.push({ server: serverName, tool: tool.name, error: `duplicate native MCP tool name: ${nativeName}` })
          continue
        }
        used.add(nativeName)
        tools.push({
          name: nativeName,
          description: tool.description || `MCP tool ${tool.name} from ${serverName}`,
          input: normalizeInputSchema(tool.inputSchema),
          async execute(input) {
            return callMcpTool({ cwd, serverName, toolName: tool.name, args: input || {}, timeoutMs })
          },
        })
      }
    } catch (error) {
      errors.push({ server: serverName, error: error instanceof Error ? error.message : String(error) })
    }
  }
  return { tools, errors }
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

function createMcpClient({ server, cwd = process.cwd(), timeoutMs = 30_000, onClose = null }) {
  const child = startServer(server, cwd)
  const state = { buffer: Buffer.alloc(0), nextId: 1, pending: new Map(), stderr: '' }
  let closed = false
  let exited = false
  function rejectPending(error) {
    for (const pending of state.pending.values()) {
      clearTimeout(pending.timer)
      pending.reject(error)
    }
    state.pending.clear()
  }
  function close(error = new Error('MCP server closed')) {
    if (closed) return
    closed = true
    rejectPending(error)
    child.kill('SIGTERM')
    setTimeout(() => {
      if (!exited) child.kill('SIGKILL')
    }, 1_000)
    onClose?.()
  }

  child.stderr.on('data', (chunk) => {
    state.stderr += chunk.toString('utf8')
    if (state.stderr.length > 100_000) state.stderr = state.stderr.slice(0, 100_000)
  })
  child.stdout.on('data', (chunk) => {
    for (const message of parseMessages(state, chunk)) {
      if (Object.prototype.hasOwnProperty.call(message, 'id') && state.pending.has(message.id)) {
        const pending = state.pending.get(message.id)
        state.pending.delete(message.id)
        clearTimeout(pending.timer)
        pending.resolve(message)
      }
    }
  })
  child.on('close', () => {
    exited = true
    close(new Error('MCP server closed'))
  })
  child.on('error', (error) => {
    exited = true
    close(error)
  })

  function sendRaw(message) {
    child.stdin.write(encodeMessage(message))
  }
  function request(method, params = {}, requestTimeoutMs = timeoutMs) {
    if (closed) throw new Error('MCP server is closed')
    const id = state.nextId
    state.nextId += 1
    const message = { jsonrpc: '2.0', id, method, params }
    return new Promise((resolvePromise, rejectPromise) => {
      const timer = setTimeout(() => {
        state.pending.delete(id)
        const error = new Error(`MCP request timed out after ${requestTimeoutMs}ms`)
        rejectPromise(error)
        close(error)
      }, requestTimeoutMs)
      state.pending.set(id, { resolve: resolvePromise, reject: rejectPromise, timer })
      sendRaw(message)
    }).then((response) => {
      if (response.error) throw new Error(response.error.message || JSON.stringify(response.error))
      return response.result
    })
  }
  function notify(method, params = {}) {
    sendRaw({ jsonrpc: '2.0', method, params })
  }
  async function initialize() {
    await request('initialize', {
      protocolVersion: MCP_PROTOCOL_VERSION,
      capabilities: {},
      clientInfo: { name: 'jeden', version: '0.1.0' },
    })
    notify('notifications/initialized')
    return api
  }
  const api = { request, notify, stderr: () => state.stderr, close, initialize }
  return api
}

export async function withMcpServer({ server, cwd = process.cwd(), timeoutMs = 30_000 }, callback) {
  const client = createMcpClient({ server, cwd, timeoutMs })
  try {
    await client.initialize()
    return await callback(client)
  } finally {
    client.close()
  }
}

const MCP_CLIENTS = new Map()

function mcpClientKey(cwd, serverName) {
  return `${resolve(cwd)}\u0000${serverName}`
}

async function getMcpClient({ cwd = process.cwd(), serverName, timeoutMs = 30_000 } = {}) {
  const key = mcpClientKey(cwd, serverName)
  const cached = MCP_CLIENTS.get(key)
  if (cached) return cached
  const server = await configuredServer({ cwd, serverName })
  const client = createMcpClient({
    server,
    cwd,
    timeoutMs,
    onClose: () => MCP_CLIENTS.delete(key),
  })
  MCP_CLIENTS.set(key, client)
  try {
    await client.initialize()
    return client
  } catch (error) {
    MCP_CLIENTS.delete(key)
    client.close()
    throw error
  }
}

export async function closeMcpClients({ cwd = null } = {}) {
  const prefix = cwd ? `${resolve(cwd)}\u0000` : null
  for (const [key, client] of Array.from(MCP_CLIENTS.entries())) {
    if (prefix && key.slice(0, prefix.length) !== prefix) continue
    MCP_CLIENTS.delete(key)
    client.close()
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
  const client = await getMcpClient({ cwd, serverName, timeoutMs })
  return client.request('tools/list', {}, timeoutMs)
}

export async function callMcpTool({ cwd = process.cwd(), serverName, toolName, args = {}, timeoutMs } = {}) {
  if (!toolName) throw new Error('toolName is required')
  const client = await getMcpClient({ cwd, serverName, timeoutMs })
  return client.request('tools/call', { name: toolName, arguments: args }, timeoutMs)
}

export async function listMcpResources({ cwd = process.cwd(), serverName, timeoutMs } = {}) {
  const client = await getMcpClient({ cwd, serverName, timeoutMs })
  return client.request('resources/list', {}, timeoutMs)
}

export async function readMcpResource({ cwd = process.cwd(), serverName, uri, timeoutMs } = {}) {
  if (!uri) throw new Error('uri is required')
  const client = await getMcpClient({ cwd, serverName, timeoutMs })
  return client.request('resources/read', { uri }, timeoutMs)
}

export async function listMcpPrompts({ cwd = process.cwd(), serverName, timeoutMs } = {}) {
  const client = await getMcpClient({ cwd, serverName, timeoutMs })
  return client.request('prompts/list', {}, timeoutMs)
}

export async function getMcpPrompt({ cwd = process.cwd(), serverName, name, args = {}, timeoutMs } = {}) {
  if (!name) throw new Error('name is required')
  const client = await getMcpClient({ cwd, serverName, timeoutMs })
  return client.request('prompts/get', { name, arguments: args }, timeoutMs)
}
