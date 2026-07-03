import { mkdir, readFile, rename as renameFile, writeFile } from 'node:fs/promises'
import { basename, dirname, extname, join, resolve } from 'node:path'
import { spawn } from 'node:child_process'

import { loadJedenConfig, loadProjectJedenConfig, saveProjectJedenConfig } from './config.js'
import { handleAuthSlashCommand } from './auth-slash.js'
import { buildCapabilityManifest, buildDoctorReport } from './diagnostics.js'
import { handleRuntimeSlashCommand } from './runtime-slash.js'
import { dispatchModeSlashCommand } from './mode-state.js'
import { closeMcpClients, listMcpPrompts, listMcpResources, listMcpTools, loadMcpConfig, loadProjectMcpConfig, saveProjectMcpConfig } from './mcp.js'
import { deleteMentalModel, enqueueMemoryMaintenance, loadMemoryRecords, loadMentalModelBank, memoryPath, mentalModelPath, normalizeMentalModelId, rebuildMemoryFile, recallMemories, saveMemoryRecords, seedMentalModel } from './memory.js'
import { listSessionArtifacts, listSessions, readSession } from './session.js'
import { copyTextToClipboardOrArtifact, createEncryptedShareBundle, handleLocalCollab } from './local-sharing.js'
import { handleExtensionsCommand, handleMarketplaceCommand, handlePluginsCommand, reloadPlugins } from './plugin-manager.js'
import { resetUsage, usageSummary } from './usage.js'

const OMP_COMMANDS = [
  ['settings', 'Open settings menu'],
  ['setup', 'Open provider setup', { aliases: ['providers'], subcommands: ['providers'] }],
  ['plan', 'Toggle plan mode (agent plans before executing)', { hint: '[prompt]' }],
  ['plan-review', 'Re-open the plan review for the latest plan (plan mode only)'],
  ['goal', 'Toggle goal mode (persistent autonomous objective for this session)', { hint: '[objective]', subcommands: ['set <objective>', 'show', 'pause', 'resume', 'drop', 'budget <N|off>'] }],
  ['guided-goal', 'Interview and refine a goal before enabling goal mode', { hint: '[rough objective]' }],
  ['loop', 'Toggle loop mode and resubmit the next prompt after every yield', { hint: '[count|duration] [prompt]' }],
  ['model', 'Switch model for this session', { aliases: ['models'] }],
  ['switch', 'Switch model for this session (same as alt+p)'],
  ['fast', 'Toggle priority service tier', { hint: '[on|off|status]', subcommands: ['on', 'off', 'status'] }],
  ['advisor', 'Toggle the advisor model reviewer', { hint: '[on|off|status|dump [raw]|configure]', subcommands: ['on', 'off', 'status', 'dump [raw]', 'configure'] }],
  ['export', 'Export session to HTML file', { hint: '[path]' }],
  ['dump', 'Copy session transcript to clipboard and write LLM request JSON to tmp'],
  ['share', 'Share session via an encrypted link'],
  ['collab', 'Share this session live via a relay', { hint: '[start|view|stop|status] [relayUrl]', subcommands: ['view', 'status', 'stop'] }],
  ['join', 'Join a shared collab session', { hint: '<link>' }],
  ['leave', 'Leave the collab session'],
  ['browser', 'Configure local browser runtime preference', { hint: '[status|headless|visible] [launch.<key>=value] [profile.<key>=value]', subcommands: ['status', 'headless', 'visible'] }],
  ['copy', 'Pick text or code from the conversation to copy'],
  ['todo', "View or modify the agent's todo list", { hint: '<subcommand>', subcommands: ['edit', 'copy', 'export [<path>]', 'import [<path>]', 'start <task>', 'done [<task|phase>]', 'drop [<task|phase>]', 'rm [<task|phase>]'] }],
  ['session', 'Session management commands', { hint: 'info|delete', subcommands: ['info', 'delete'] }],
  ['jobs', 'Show async background jobs status'],
  ['usage', 'Show provider usage and limits', { hint: '[show|reset [account|active]]', subcommands: ['show', 'reset [account|active]'] }],
  ['stats', 'Launch the local stats dashboard', { hint: '[--port <port>]' }],
  ['changelog', 'Show changelog entries', { hint: '[full]', subcommands: ['full'] }],
  ['hotkeys', 'Show all keyboard shortcuts'],
  ['tools', 'Show tools currently visible to the agent'],
  ['context', 'Show estimated context usage breakdown'],
  ['extensions', 'Open Extension Control Center dashboard', { aliases: ['status'] }],
  ['agents', 'Open Agent Control Center dashboard'],
  ['branch', 'Create a new branch from a previous message'],
  ['fork', 'Create a new fork from a previous message'],
  ['tree', 'Navigate session tree (switch branches)'],
  ['login', 'Login with OAuth provider', { hint: '[provider|redirect URL]' }],
  ['logout', 'Logout from OAuth provider', { hint: '[provider]' }],
  ['mcp', 'Manage MCP servers (add, list, remove, test)', { hint: '<subcommand>', subcommands: ['list', 'add', 'remove <name>', 'test <name>', 'reauth <name>', 'unauth <name>', 'enable <name>', 'disable <name>', 'reconnect <name>', 'reload', 'resources', 'tools', 'prompts'] }],
  ['ssh', 'Manage SSH hosts (add, list, remove)', { hint: '<subcommand>', subcommands: ['list', 'add', 'remove <name>', 'help'] }],
  ['new', 'Start a new session'],
  ['fresh', 'Reset provider stream state without changing the local transcript'],
  ['drop', 'Delete the current session and start a new one'],
  ['compact', 'Manually compact the session context'],
  ['shake', 'Drop heavy content from context', { hint: '[elide|images]', subcommands: ['elide', 'images'] }],
  ['handoff', 'Hand off session context to a new session', { hint: '[focus instructions]' }],
  ['resume', 'Resume a different session', { hint: '[session id]' }],
  ['btw', 'Ask an ephemeral side question using the current session context', { hint: '<question>' }],
  ['tan', 'Run a full background agent on tangential work', { hint: '<work>' }],
  ['omfg', 'Forge a TTSR rule from a complaint to stop recurring behavior', { hint: '<complaint>' }],
  ['retry', 'Retry the last failed agent turn'],
  ['debug', 'Open debug tools selector'],
  ['memory', 'Inspect and operate memory maintenance', { hint: '<subcommand>', subcommands: ['view', 'stats', 'diagnose', 'clear', 'reset', 'enqueue', 'rebuild', 'mm list', 'mm show', 'mm history', 'mm seed', 'mm delete'] }],
  ['rename', 'Rename the current session', { hint: '<title>' }],
  ['move', 'Move the current session to a different directory', { hint: '[<path>]' }],
  ['exit', 'Exit the application'],
  ['marketplace', 'Manage marketplace plugin sources and installed plugins', { hint: '<subcommand>', subcommands: ['add <source>', 'remove <name>', 'update [name]', 'list', 'discover [marketplace]', 'install <name@marketplace>', 'uninstall [name@marketplace]', 'installed', 'upgrade [name@marketplace]', 'help'] }],
  ['plugins', 'View and manage installed plugins', { hint: '[list|enable|disable]', subcommands: ['list', 'enable <name@marketplace>', 'disable <name@marketplace>'] }],
  ['reload-plugins', 'Reload all plugins'],
  ['force', 'Force next turn to use a specific tool', { aliases: ['force:'], hint: '<tool-name> [prompt]' }],
  ['quit', 'Quit the application'],
]

export const SLASH_COMMANDS = OMP_COMMANDS.map(([name, description, options = {}]) => ({
  name,
  description,
  aliases: options.aliases || [],
  hint: options.hint || '',
  subcommands: options.subcommands || [],
}))

const COMMANDS_BY_NAME = new Map()
for (const command of SLASH_COMMANDS) {
  COMMANDS_BY_NAME.set(command.name, command)
  for (const alias of command.aliases) COMMANDS_BY_NAME.set(alias, command)
}
COMMANDS_BY_NAME.set('help', { name: 'help', description: 'Show slash commands', aliases: [], hint: '', subcommands: [] })
COMMANDS_BY_NAME.set('commands', { name: 'commands', description: 'Show slash commands', aliases: [], hint: '', subcommands: [] })

function ok(text) { return { handled: true, role: 'system', text } }
function err(text) { return { handled: true, role: 'error', text } }
function lines(values) { return values.filter((value) => value !== null && value !== undefined && value !== '').join('\n') }
function nowIso() { return new Date().toISOString() }

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

function state(context) {
  const root = context.modeState || context.slashState || (context.slashState = {})
  root.slash ||= {}
  const st = root.slash
  st.todos ||= []
  st.jobs ||= []
  st.branches ||= [{ id: 'main', title: 'main', createdAt: nowIso() }]
  st.collab ||= { host: null, guest: null }
  return st
}

export function parseSlashCommand(input) {
  const text = String(input || '').trim()
  if (!text.startsWith('/')) return null
  const body = text.slice(1)
  if (!body) return { rawName: '', name: '', args: '', text }
  const match = /^([^\s]+)(?:\s+([\s\S]*))?$/.exec(body)
  if (!match) return null
  const rawName = match[1]
  const name = rawName.endsWith(':') ? rawName.slice(0, -1) : rawName
  return { rawName, name, args: match[2] || '', text }
}

export function findSlashCommand(name) {
  return COMMANDS_BY_NAME.get(String(name || '').toLowerCase()) || null
}

export function slashCommandHints(input, { limit = 8 } = {}) {
  const parsed = parseSlashCommand(input)
  if (!parsed) return []
  const prefix = parsed.name.toLowerCase()
  return SLASH_COMMANDS.filter((command) => command.name.startsWith(prefix) || command.aliases.some((alias) => alias.startsWith(prefix))).slice(0, limit)
}

export function formatSlashCommand(command) {
  const aliasText = command.aliases.length ? ` (alias: ${command.aliases.map((alias) => `/${alias}`).join(', ')})` : ''
  const hint = command.hint ? ` ${command.hint}` : ''
  return `/${command.name}${hint}${aliasText} — ${command.description}`
}

export function formatSlashCommandList() {
  const rows = ['Jeden slash commands (OMP surface with local handlers):']
  for (const command of SLASH_COMMANDS) {
    rows.push(`[local] ${formatSlashCommand(command)}`)
    if (command.subcommands.length) rows.push(`    subcommands: ${command.subcommands.join(', ')}`)
  }
  rows.push('', 'Every listed slash command is dispatched locally before the model. Cloud/UI-backed surfaces use durable local equivalents when a hosted backend is not configured.')
  return rows.join('\n')
}

function formatSessionText(session) {
  const out = [`Jeden session ${session.id}`, session.path, '']
  for (const event of session.events || []) {
    const label = `${event.ts || ''} ${event.type || ''}`.trim()
    out.push(`## ${label}`, JSON.stringify(event.data || {}, null, 2), '')
  }
  return out.join('\n')
}

function formatSessionMarkdown(session) {
  const out = [`# Jeden session ${session.id}`, '', session.path, '']
  for (const event of session.events || []) out.push(`## ${`${event.ts || ''} ${event.type || ''}`.trim()}`, '', '```json', JSON.stringify(event.data || {}, null, 2), '```', '')
  return `${out.join('\n')}\n`
}


function htmlEscape(value) {
  return String(value ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')
}

function formatSessionHtml(session) {
  const body = (session.events || []).map((event) => `<section><h2>${htmlEscape(`${event.ts || ''} ${event.type || ''}`.trim())}</h2><pre>${htmlEscape(JSON.stringify(event.data || {}, null, 2))}</pre></section>`).join('\n')
  return `<!doctype html><html><head><meta charset="utf-8"><title>Jeden session ${htmlEscape(session.id)}</title></head><body><h1>Jeden session ${htmlEscape(session.id)}</h1><p>${htmlEscape(session.path)}</p>${body}</body></html>\n`
}

async function currentSession(context) {
  return readSession({ idOrPath: context.recorder.path() })
}

async function handleCopy(parsed, context) {
  const directText = parsed.args.trim()
  const payload = directText || formatSessionText(await currentSession(context))
  const source = directText ? 'provided text' : 'current session transcript'
  return ok(await copyTextToClipboardOrArtifact({ payload, source, recorder: context.recorder }))
}

async function createEncryptedShare(context, args = '') {
  const argv = splitArgs(args)
  let copyLink = false
  for (const arg of argv) {
    if (arg === 'copy' || arg === '--copy' || arg === '--clipboard') copyLink = true
  }
  return createEncryptedShareBundle({ session: await currentSession(context), artifactDir: context.recorder.artifactDir(), copyLink })
}

async function exportCurrentSession(context, args) {
  const argv = splitArgs(args)
  const session = await currentSession(context)
  let format = 'json'
  let target = argv[0] || ''
  if (target === '--html') { format = 'html'; target = argv[1] || '' }
  if (target === '--markdown' || target === '--md') { format = 'markdown'; target = argv[1] || '' }
  const extension = extname(target)
  if (extension === '.html') format = 'html'
  if (extension === '.md' || extension === '.markdown') format = 'markdown'
  const payload = format === 'html' ? formatSessionHtml(session) : format === 'markdown' ? formatSessionMarkdown(session) : `${JSON.stringify(session, null, 2)}\n`
  if (!target) return `Current session export (${format}):\n${payload}`
  const file = resolve(context.args.cwd, target)
  await mkdir(dirname(file), { recursive: true })
  await writeFile(file, payload, 'utf8')
  return `Session exported to ${file}`
}

async function sessionInfo(context) {
  const session = await currentSession(context)
  const artifacts = await listSessionArtifacts({ idOrPath: context.recorder.path() })
  return lines([
    `Session: ${session.id}`,
    `Path: ${session.path}`,
    `Workspace: ${context.args.cwd}`,
    `Events: ${session.events.length}`,
    `Artifacts: ${artifacts.artifacts.length}`,
  ])
}

async function handleSession(parsed, context) {
  const [verb] = splitArgs(parsed.args)
  if (!verb || verb === 'info') return ok(await sessionInfo(context))
  if (verb === 'delete') return err('Refusing to delete the active session from inside itself. Exit Jeden, then remove the session directory explicitly if you still want this destructive action.')
  return err('Usage: /session [info|delete]')
}

async function handleTodo(parsed, context) {
  const st = state(context)
  const [verb, ...rest] = splitArgs(parsed.args)
  const text = rest.join(' ')
  if (!verb || verb === 'list') {
    if (st.todos.length === 0) return ok('Todo list is empty.')
    return ok(st.todos.map((todo, index) => `${index + 1}. [${todo.status}] ${todo.text}`).join('\n'))
  }
  if (verb === 'add' || verb === 'start') {
    if (!text) return err(`Usage: /todo ${verb} <task>`)
    st.todos.push({ text, status: verb === 'start' ? 'in_progress' : 'pending', createdAt: nowIso() })
    return ok(`Todo added: ${text}`)
  }
  if (['done', 'drop', 'rm'].includes(verb)) {
    const needle = text.toLowerCase()
    const index = st.todos.findIndex((todo, i) => String(i + 1) === text || todo.text.toLowerCase().includes(needle))
    if (index === -1) return err(`Todo not found: ${text || '(missing)'}`)
    const todo = st.todos[index]
    if (verb === 'rm') st.todos.splice(index, 1)
    else todo.status = verb === 'done' ? 'done' : 'dropped'
    return ok(`${verb === 'rm' ? 'Removed' : 'Updated'} todo: ${todo.text}`)
  }
  if (verb === 'copy' || verb === 'export') {
    const md = st.todos.map((todo) => `- [${todo.status === 'done' ? 'x' : ' '}] ${todo.text}`).join('\n') || '- [ ]'
    if (verb === 'copy') return ok(md)
    const target = resolve(context.args.cwd, text || 'TODO.md')
    await writeFile(target, `${md}\n`, 'utf8')
    return ok(`Todos exported to ${target}`)
  }
  if (verb === 'import') {
    const target = resolve(context.args.cwd, text || 'TODO.md')
    const raw = await readFile(target, 'utf8')
    st.todos = raw.split(/\r?\n/).map((line) => /- \[([ xX])\] (.+)/.exec(line)).filter(Boolean).map((m) => ({ text: m[2], status: m[1].trim() ? 'done' : 'pending', createdAt: nowIso() }))
    return ok(`Imported ${st.todos.length} todos from ${target}`)
  }
  return err('Usage: /todo [list|add|start|done|drop|rm|copy|export|import]')
}

function parseMcpServerSpec(parts) {
  if (parts.length === 1) {
    try {
      const parsed = JSON.parse(parts[0])
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) return parsed
    } catch {}
  }
  const [command, ...args] = parts
  return { command, args }
}

function projectDisabledServers(config) {
  return Array.isArray(config.disabledServers) ? config.disabledServers.filter(Boolean).map(String) : []
}

function withoutValue(values, value) {
  return values.filter((item) => item !== value)
}

function mcpHookSpec(server, verb) {
  const value = server?.[`${verb}Command`] || server?.[verb]
  if (!value) return null
  if (typeof value === 'string') return parseMcpServerSpec(splitArgs(value))
  if (Array.isArray(value)) return parseMcpServerSpec(value.map(String))
  if (typeof value === 'object' && value.command) return { command: String(value.command), args: (value.args || []).map(String), env: value.env || {} }
  return null
}

function runLocalCommand(spec, { cwd }) {
  return new Promise((resolveResult, reject) => {
    const child = spawn(spec.command, spec.args || [], { cwd, env: { ...process.env, ...(spec.env || {}) }, stdio: ['ignore', 'pipe', 'pipe'] })
    let stdout = ''
    let stderr = ''
    child.stdout.on('data', (chunk) => { stdout += chunk.toString('utf8') })
    child.stderr.on('data', (chunk) => { stderr += chunk.toString('utf8') })
    child.on('error', reject)
    child.on('close', (code) => {
      if (code === 0) resolveResult({ stdout: stdout.trim(), stderr: stderr.trim() })
      else reject(new Error((stderr || stdout).trim() || `${spec.command} exited with ${code}`))
    })
  })
}

async function handleMcp(parsed, context) {
  const [verb = 'list', serverName, ...rest] = splitArgs(parsed.args)
  const cwd = context.args.cwd
  if (verb === 'list') {
    const config = await loadMcpConfig({ cwd })
    const names = Object.keys(config.mcpServers || {})
    const disabled = new Set(config.disabledServers || [])
    return ok(names.length ? names.map((name) => `${name}${disabled.has(name) ? ' (disabled)' : ''}`).join('\n') : 'No MCP servers configured.')
  }
  if (verb === 'add') {
    if (!serverName || rest.length === 0) return err('Usage: /mcp add <name> <command> [args...]')
    const project = await loadProjectMcpConfig({ cwd })
    project.mcpServers = { ...(project.mcpServers || {}), [serverName]: parseMcpServerSpec(rest) }
    project.disabledServers = withoutValue(projectDisabledServers(project), serverName)
    const file = await saveProjectMcpConfig(project, { cwd })
    await closeMcpClients({ cwd })
    return ok(`Added MCP server ${serverName} to ${file}.`)
  }
  if (verb === 'remove') {
    if (!serverName) return err('Usage: /mcp remove <name>')
    const project = await loadProjectMcpConfig({ cwd })
    if (!project.mcpServers?.[serverName]) {
      const effective = await loadMcpConfig({ cwd })
      if (effective.mcpServers?.[serverName]) return err(`MCP server ${serverName} is not in <cwd>/.jeden/mcp.json. Remove it from ~/.jeden/mcp.json or use /mcp disable ${serverName} for this workspace.`)
      return err(`MCP server not found: ${serverName}`)
    }
    delete project.mcpServers[serverName]
    project.disabledServers = withoutValue(projectDisabledServers(project), serverName)
    const file = await saveProjectMcpConfig(project, { cwd })
    await closeMcpClients({ cwd })
    return ok(`Removed MCP server ${serverName} from ${file}.`)
  }
  if (verb === 'disable' || verb === 'enable') {
    if (!serverName) return err(`Usage: /mcp ${verb} <name>`)
    const effective = await loadMcpConfig({ cwd })
    if (!effective.mcpServers?.[serverName]) return err(`MCP server not found: ${serverName}`)
    const project = await loadProjectMcpConfig({ cwd })
    const disabled = projectDisabledServers(project)
    project.disabledServers = verb === 'disable' ? [...new Set([...disabled, serverName])] : withoutValue(disabled, serverName)
    const file = await saveProjectMcpConfig(project, { cwd })
    await closeMcpClients({ cwd })
    return ok(`${verb === 'disable' ? 'Disabled' : 'Enabled'} MCP server ${serverName} in ${file}.`)
  }
  if (verb === 'reauth' || verb === 'unauth') {
    if (!serverName) return err(`Usage: /mcp ${verb} <name>`)
    const effective = await loadMcpConfig({ cwd })
    const server = effective.mcpServers?.[serverName]
    if (!server) return err(`MCP server not found: ${serverName}`)
    const spec = mcpHookSpec(server, verb)
    if (!spec?.command) return err(`MCP server ${serverName} has no ${verb} command configured. Add ${verb}Command to its MCP server config.`)
    try {
      const result = await runLocalCommand(spec, { cwd })
      await closeMcpClients({ cwd })
      return ok(lines([`MCP ${verb} completed for ${serverName}.`, result.stdout, result.stderr]))
    } catch (error) {
      return err(`MCP ${verb} failed: ${error instanceof Error ? error.message : String(error)}`)
    }
  }
  const mcpReadCommands = new Set(['tools', 'resources', 'prompts', 'test'])
  if (!serverName && mcpReadCommands.has(verb)) return err(`Usage: /mcp ${verb} <server>`)
  try {
    if (verb === 'tools' || verb === 'test') return ok(JSON.stringify(await listMcpTools({ cwd, serverName, timeoutMs: 10_000 }), null, 2))
    if (verb === 'resources') return ok(JSON.stringify(await listMcpResources({ cwd, serverName, timeoutMs: 10_000 }), null, 2))
    if (verb === 'prompts') return ok(JSON.stringify(await listMcpPrompts({ cwd, serverName, timeoutMs: 10_000 }), null, 2))
    if (verb === 'reload' || verb === 'reconnect') { await closeMcpClients({ cwd }); return ok('MCP clients closed; they will reconnect on next use.') }
  } catch (error) {
    return err(`MCP ${verb} failed: ${error instanceof Error ? error.message : String(error)}`)
  }
  return err('Usage: /mcp list | add <name> <command> [args...] | remove <name> | enable <name> | disable <name> | reauth <name> | unauth <name> | tools <server> | resources <server> | prompts <server> | test <server> | reload | reconnect')
}

function sshHostValue(target, options) {
  if (options.length === 0) return target
  const host = { host: target }
  for (const option of options) {
    const index = option.indexOf('=')
    if (index <= 0) return null
    host[option.slice(0, index)] = option.slice(index + 1)
  }
  return host
}

function configuredSshHosts(config) {
  return config.sshHosts || config.ssh?.hosts || config.ssh || {}
}

async function handleSsh(parsed, context) {
  const [verb = 'list', name, target, ...options] = splitArgs(parsed.args)
  const cwd = context.args.cwd
  const config = await loadJedenConfig({ cwd })
  const hosts = configuredSshHosts(config)
  if (verb === 'list') {
    const names = Object.keys(hosts || {})
    return ok(names.length ? names.map((host) => `${host}\t${typeof hosts[host] === 'string' ? hosts[host] : JSON.stringify(hosts[host])}`).join('\n') : 'No SSH hosts configured in ~/.jeden/config.json or <cwd>/.jeden/config.json (sshHosts).')
  }
  if (verb === 'help') return ok('Usage: /ssh list | add <name> <target> [key=value ...] | remove <name>. Hosts are stored in <cwd>/.jeden/config.json under sshHosts.')
  if (verb === 'add') {
    if (!name || !target) return err('Usage: /ssh add <name> <target> [key=value ...]')
    const value = sshHostValue(target, options)
    if (!value) return err('Usage: /ssh add <name> <target> [key=value ...]')
    const project = await loadProjectJedenConfig({ cwd })
    project.sshHosts = { ...(project.sshHosts || {}), [name]: value }
    const file = await saveProjectJedenConfig(project, { cwd })
    return ok(`Added SSH host ${name} to ${file}.`)
  }
  if (verb === 'remove') {
    if (!name) return err('Usage: /ssh remove <name>')
    const project = await loadProjectJedenConfig({ cwd })
    if (!project.sshHosts?.[name]) {
      if (hosts?.[name]) return err(`SSH host ${name} is not in <cwd>/.jeden/config.json. Remove it from ~/.jeden/config.json or the config file that defines it.`)
      return err(`SSH host not found: ${name}`)
    }
    delete project.sshHosts[name]
    const file = await saveProjectJedenConfig(project, { cwd })
    return ok(`Removed SSH host ${name} from ${file}.`)
  }
  return err('Usage: /ssh list | add <name> <target> [key=value ...] | remove <name> | help')
}

function formatMentalModelList(bank) {
  const models = Object.values(bank.models || {}).sort((a, b) => String(b.updatedAt || '').localeCompare(String(a.updatedAt || '')))
  if (models.length === 0) return 'No mental models seeded.'
  return models.map((model) => `${model.id}\t${model.updatedAt || '-'}\t${String(model.text || '').slice(0, 100)}`).join('\n')
}

function formatMentalModelHistory(bank, modelId = '') {
  const id = normalizeMentalModelId(modelId)
  const history = (bank.history || []).filter((event) => !id || event.modelId === id)
  if (history.length === 0) return id ? `No history for mental model: ${id}` : 'No mental-model history.'
  return history.map((event) => `${event.at || '-'}\t${event.action || '-'}\t${event.modelId || '-'}\t${String(event.text || '').slice(0, 120)}`).join('\n')
}

async function handleMentalModels(rest, context, memoryFile) {
  const [verb = 'list', id, ...textParts] = rest
  const file = mentalModelPath(memoryFile)
  const cwd = context.args.cwd
  if (verb === 'list') return ok(lines([`Mental-model file: ${file}`, formatMentalModelList(await loadMentalModelBank(file))]))
  if (verb === 'show') {
    const modelId = normalizeMentalModelId(id)
    if (!modelId) return err('Usage: /memory mm show <id>')
    const bank = await loadMentalModelBank(file)
    const model = bank.models[modelId]
    if (!model) return err(`Mental model not found: ${modelId}`)
    return ok(lines([`Mental model: ${model.id}`, `Updated: ${model.updatedAt || '-'}`, '', model.text || '']))
  }
  if (verb === 'history') return ok(lines([`Mental-model file: ${file}`, formatMentalModelHistory(await loadMentalModelBank(file), id)]))
  if (verb === 'seed') {
    if (!id || textParts.length === 0) return err('Usage: /memory mm seed <id> <text>')
    const { model } = await seedMentalModel({ id, text: textParts.join(' '), cwd, file })
    return ok(`Seeded mental model ${model.id} in ${file}`)
  }
  if (verb === 'delete') {
    if (!id) return err('Usage: /memory mm delete <id>')
    const result = await deleteMentalModel({ id, cwd, file })
    return ok(result.deleted ? `Deleted mental model ${result.modelId} from ${file}` : `Mental model not found: ${result.modelId}`)
  }
  return err('Usage: /memory mm list | show <id> | history [id] | seed <id> <text> | delete <id>')
}

async function handleMemory(parsed, context) {
  const [verb = 'view', ...rest] = splitArgs(parsed.args)
  const cwd = context.args.cwd
  const file = memoryPath()
  if (verb === 'stats' || verb === 'diagnose') {
    const records = await loadMemoryRecords(file, { cwd })
    return ok(lines([`Memory file: ${file}`, `Records: ${records.length}`, `Scope: ${cwd}`]))
  }
  if (verb === 'view') {
    const query = rest.join(' ')
    const records = query ? await recallMemories({ cwd, query, limit: 20, file }) : await loadMemoryRecords(file, { cwd })
    return ok(records.length ? records.slice(-20).map((record) => `${record.id || '-'}\t${record.kind || '-'}\t${record.text || record.content || ''}`).join('\n') : 'No memory records.')
  }
  if (verb === 'clear' || verb === 'reset') { await saveMemoryRecords([], file); return ok(`Cleared memory file: ${file}`) }
  if (verb === 'enqueue') {
    const queued = await enqueueMemoryMaintenance({ operation: 'enqueue', cwd, file, note: rest.join(' ') })
    return ok(lines([`Queued local memory maintenance: ${queued.record.id}`, `Queue: ${queued.queueFile}`, `Memory file: ${file}`]))
  }
  if (verb === 'rebuild') {
    const result = await rebuildMemoryFile({ cwd, file })
    return ok(lines([`Rebuilt local memory file: ${result.file}`, `Records: ${result.before} -> ${result.after}`, `Duplicates removed: ${result.duplicates}`, `Expired removed: ${result.expired}`, `Maintenance queue: ${result.queueFile}`]))
  }
  if (verb === 'mm') return handleMentalModels(rest, context, file)
  return err('Usage: /memory view [query] | stats | diagnose | clear | reset | enqueue [note] | rebuild | mm list|show|history|seed|delete')
}


async function handleConfigStatus(name) {
  if (name === 'settings' || name === 'setup') return null
  if (name === 'login' || name === 'logout') return null
  if (name === 'agents') return ok('Agent controls:\n- /tan <work> starts a detached local agent job tracked in session artifacts.\n- /advisor manages second-pass reviewer mode.\n- /jobs shows locally tracked background jobs.')
  return null
}


async function handleSessionLifecycle(canonical, parsed, context) {
  const st = state(context)
  if (canonical === 'new' || canonical === 'fresh') return ok('Started a fresh logical turn context. Provider stream state is reset for the next prompt in this Jeden process.')
  if (canonical === 'drop') return err('Refusing to delete the active session from inside itself. Use /new for a fresh context or exit and remove the session directory explicitly.')
  if (canonical === 'resume') {
    const [id] = splitArgs(parsed.args)
    if (!id) {
      const sessions = await listSessions({ limit: 10 })
      return ok(sessions.map((s) => `${s.id}\t${s.updatedAt || '-'}\t${s.cwd || '-'}`).join('\n') || 'No sessions found.')
    }
    const session = await readSession({ idOrPath: id })
    return ok(`Session ${session.id} exists at ${session.path}. Full in-place interactive resume is available through CLI: jeden resume ${session.path} "<task>"`)
  }
  if (canonical === 'rename') { st.title = parsed.args.trim() || st.title || basename(context.recorder.path()); return ok(`Session title set to: ${st.title}`) }
  if (canonical === 'move') {
    const [target] = splitArgs(parsed.args)
    if (!target) return ok(`Current session path: ${context.recorder.path()}`)
    const next = resolve(context.args.cwd, target)
    await mkdir(dirname(next), { recursive: true })
    await renameFile(context.recorder.path(), next)
    return ok(`Moved session directory to ${next}. Restart Jeden to continue recording in the moved path.`)
  }
  if (canonical === 'compact') { st.compact = true; return ok('Compact mode enabled for subsequent prompts: large prior context should be summarized before use.') }
  if (canonical === 'shake') { st.shake = parsed.args.trim() || 'elide'; return ok(`Shake mode applied locally: ${st.shake}. Subsequent prompts will instruct the model to avoid relying on heavy prior artifacts unless re-read.`) }
  if (canonical === 'handoff') {
    const session = await currentSession(context)
    const focus = parsed.args.trim()
    const text = `${focus ? `Focus: ${focus}\n\n` : ''}${formatSessionMarkdown(session)}`
    const file = join(context.recorder.artifactDir(), 'handoff.md')
    await writeFile(file, text, 'utf8')
    return ok(`Handoff written to ${file}`)
  }
  return null
}

async function handleUtility(canonical, parsed, context) {
  if (canonical === 'export') return ok(await exportCurrentSession(context, parsed.args))
  if (canonical === 'dump') return ok(formatSessionText(await currentSession(context)))
  if (canonical === 'copy') return handleCopy(parsed, context)
  if (canonical === 'jobs') return ok(state(context).jobs.length ? JSON.stringify(state(context).jobs, null, 2) : 'No background jobs are tracked inside this Jeden process.')
  if (canonical === 'usage') {
    const [verb = 'show'] = splitArgs(parsed.args)
    if (verb === 'reset') return ok(`Reset usage accounting: ${(await resetUsage({ cwd: context.args.cwd })).file}`)
    if (verb !== 'show' && verb !== 'status') return err('Usage: /usage [show|reset]')
    return ok(JSON.stringify(await usageSummary({ cwd: context.args.cwd }), null, 2))
  }
  if (canonical === 'stats') return ok(JSON.stringify(await buildDoctorReport({ cwd: context.args.cwd }), null, 2))
  if (canonical === 'debug') return ok(JSON.stringify(await buildDoctorReport({ cwd: context.args.cwd }), null, 2))
  if (canonical === 'context') return ok(JSON.stringify(await buildCapabilityManifest({ cwd: context.args.cwd }), null, 2))
  if (canonical === 'changelog') return ok('No bundled changelog is present in Jeden. Git history is the source of release notes for this package.')
  if (canonical === 'tools') {
    const registry = context.createToolRegistry?.({ cwd: context.args?.cwd, allowWrite: context.args?.allowWrite, allowCommand: context.args?.allowCommand, artifactDir: context.recorder?.artifactDir?.() })
    const tools = registry?.list?.() || []
    return ok(['Tools visible to Jeden:', ...tools.map((tool) => `- ${tool.name}: ${tool.description}`)].join('\n'))
  }
  if (canonical === 'hotkeys') return ok(['Jeden interactive hotkeys:', 'Enter submits the prompt.', 'Ctrl-J inserts a newline.', 'Left/Right/Home/End edit inside the prompt.', 'Up/Down navigate prompt history.', 'Ctrl-C exits input mode or denies approval.'].join('\n'))
  return null
}

async function handleCollab(canonical, parsed, context) {
  const st = state(context)
  if (canonical === 'share') return ok(await createEncryptedShare(context, parsed.args))
  const [verb = 'status', relay] = splitArgs(parsed.args)
  const relayConfig = await loadJedenConfig({ cwd: context.args.cwd })
  const result = await handleLocalCollab({
    canonical,
    verb,
    relay,
    target: parsed.args.trim(),
    collabState: st.collab,
    sessionPath: context.recorder.path(),
    cwd: context.args.cwd,
    artifactDir: context.recorder.artifactDir(),
    relayConfig,
  })
  if (!result) return null
  return result.error ? err(result.error) : ok(result.text)
}

function handleBranching(canonical, parsed, context) {
  const st = state(context)
  if (canonical === 'branch' || canonical === 'fork') {
    const id = `${canonical}-${st.branches.length + 1}`
    st.branches.push({ id, title: parsed.args.trim() || id, createdAt: nowIso() })
    return ok(`${canonical} created locally: ${id}`)
  }
  if (canonical === 'tree') return ok(st.branches.map((branch) => `${branch.id}\t${branch.title}\t${branch.createdAt}`).join('\n'))
  return null
}

export async function dispatchSlashCommand(input, context = {}) {
  const parsed = parseSlashCommand(input)
  if (!parsed) return { handled: false }
  const command = findSlashCommand(parsed.name)
  const canonical = command?.name || parsed.name.toLowerCase()
  if (canonical === 'exit' || canonical === 'quit') return { handled: true, exit: true }
  if (canonical === 'help' || canonical === 'commands') return ok(formatSlashCommandList())
  if (!command) return err(`Unknown slash command /${parsed.rawName}. Use /help to list commands.`)

  const runtime = await handleRuntimeSlashCommand(canonical, parsed, context)
  if (runtime) return runtime

  const mode = dispatchModeSlashCommand({ ...parsed, canonical }, context.modeState, context)
  if (mode) return mode

  if (canonical === 'model') {
    const nextModel = parsed.args.trim()
    if (nextModel) { context.args.model = nextModel; context.setModel?.(nextModel); return ok(`Model route set to ${nextModel}.`) }
    return ok(`Current model route: ${context.args?.model || process.env.JEDEN_MODEL || process.env.MODEL || 'default'}.`)
  }

  const auth = await handleAuthSlashCommand(canonical, parsed, context)
  if (auth) return auth
  if (canonical === 'marketplace') {
    const result = await handleMarketplaceCommand(splitArgs(parsed.args), { cwd: context.args.cwd })
    return result.error ? err(result.error) : ok(result.text)
  }
  if (canonical === 'plugins') {
    const result = await handlePluginsCommand(splitArgs(parsed.args), { cwd: context.args.cwd })
    return result.error ? err(result.error) : ok(result.text)
  }
  if (canonical === 'extensions') {
    const result = await handleExtensionsCommand({ cwd: context.args.cwd })
    return result.error ? err(result.error) : ok(result.text)
  }
  if (canonical === 'reload-plugins') {
    const builtIn = context.createToolRegistry?.({ cwd: context.args?.cwd, allowWrite: false, allowCommand: false, artifactDir: context.recorder?.artifactDir?.() })?.list?.() || []
    const result = await reloadPlugins({ cwd: context.args.cwd, builtInToolNames: builtIn.map((tool) => tool.name), allowCommand: context.args?.allowCommand })
    return result.error ? err(result.error) : ok(result.text)
  }
  const config = await handleConfigStatus(canonical)
  if (config) return config
  if (canonical === 'session') return handleSession(parsed, context)
  if (canonical === 'todo') return handleTodo(parsed, context)
  if (canonical === 'mcp') return handleMcp(parsed, context)
  if (canonical === 'ssh') return handleSsh(parsed, context)
  if (canonical === 'memory') return handleMemory(parsed, context)

  const utility = await handleUtility(canonical, parsed, context)
  if (utility) return utility
  const lifecycle = await handleSessionLifecycle(canonical, parsed, context)
  if (lifecycle) return lifecycle
  const collab = await handleCollab(canonical, parsed, context)
  if (collab) return collab
  const branching = handleBranching(canonical, parsed, context)
  if (branching) return branching

  return err(`No handler matched /${canonical}; this is a bug in the slash dispatcher.`)
}
