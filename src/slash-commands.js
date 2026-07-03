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
  ['browser', 'Toggle browser headless vs visible mode', { hint: '[headless|visible]', subcommands: ['headless', 'visible'] }],
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
  ['marketplace', 'Manage marketplace plugin sources and installed plugins', { hint: '<subcommand>', subcommands: ['add <source>', 'remove <name>', 'update [name]', 'list', 'discover [marketplace]', 'uninstall [name@marketplace]', 'installed', 'upgrade [name@marketplace]', 'help'] }],
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

const JEDEN_IMPLEMENTED = new Set(['exit', 'quit', 'model', 'models', 'session', 'tools', 'hotkeys', 'help', 'commands'])

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
  const matched = SLASH_COMMANDS.filter((command) => {
    if (command.name.startsWith(prefix)) return true
    return command.aliases.some((alias) => alias.startsWith(prefix))
  })
  return matched.slice(0, limit)
}

export function formatSlashCommand(command) {
  const aliasText = command.aliases.length ? ` (alias: ${command.aliases.map((alias) => `/${alias}`).join(', ')})` : ''
  const hint = command.hint ? ` ${command.hint}` : ''
  return `/${command.name}${hint}${aliasText} — ${command.description}`
}

export function formatSlashCommandList({ includeImplementation = true } = {}) {
  const lines = ['OMP slash commands known to Jeden:']
  for (const command of SLASH_COMMANDS) {
    const implemented = JEDEN_IMPLEMENTED.has(command.name) || command.aliases.some((alias) => JEDEN_IMPLEMENTED.has(alias))
    const status = includeImplementation ? (implemented ? 'local' : 'OMP-only') : ''
    lines.push(`${status ? `[${status}] ` : ''}${formatSlashCommand(command)}`)
    if (command.subcommands.length) lines.push(`    subcommands: ${command.subcommands.join(', ')}`)
  }
  lines.push('', 'Local Jeden commands: /help, /commands, /model [name], /models [name], /session info, /tools, /hotkeys, /exit, /quit.')
  lines.push('OMP-only commands are intercepted and reported instead of being sent to the model.')
  return lines.join('\n')
}

function localHotkeysText() {
  return [
    'Jeden interactive hotkeys:',
    'Enter submits the prompt.',
    'Ctrl-J inserts a newline.',
    'Left/Right/Home/End edit inside the prompt.',
    'Up/Down navigate prompt history.',
    'Ctrl-C exits input mode or denies an approval prompt.',
  ].join('\n')
}

function unsupportedText(command) {
  return [
    `/${command.name} exists in OMP: ${command.description}`,
    'Jeden does not implement this runtime feature yet, so the command was not sent to the model.',
    'Use /help to see which slash commands are local in Jeden versus OMP-only.',
  ].join('\n')
}

export async function dispatchSlashCommand(input, context = {}) {
  const parsed = parseSlashCommand(input)
  if (!parsed) return { handled: false }

  const command = findSlashCommand(parsed.name)
  const canonical = command?.name || parsed.name.toLowerCase()

  if (canonical === 'exit' || canonical === 'quit') return { handled: true, exit: true }

  if (canonical === 'help' || canonical === 'commands') {
    return { handled: true, role: 'system', text: formatSlashCommandList() }
  }

  if (canonical === 'model') {
    const nextModel = parsed.args.trim()
    if (nextModel) {
      context.args.model = nextModel
      context.setModel?.(nextModel)
      return { handled: true, role: 'system', text: `Model route set to ${nextModel}.` }
    }
    return { handled: true, role: 'system', text: `Current model route: ${context.args?.model || process.env.JEDEN_MODEL || process.env.MODEL || 'default'}.` }
  }

  if (canonical === 'session') {
    const verb = parsed.args.trim().toLowerCase() || 'info'
    if (verb === 'info') {
      return { handled: true, role: 'system', text: [`Session: ${context.recorder?.path?.() || '(not started)'}`, `Workspace: ${context.args?.cwd || process.cwd()}`].join('\n') }
    }
    return { handled: true, role: 'system', text: 'Jeden supports /session info in interactive mode. Destructive session deletion is only exposed through explicit CLI commands.' }
  }

  if (canonical === 'tools') {
    const registry = context.createToolRegistry?.({ cwd: context.args?.cwd, allowWrite: context.args?.allowWrite, allowCommand: context.args?.allowCommand, artifactDir: context.recorder?.artifactDir?.() })
    const tools = registry?.list?.() || []
    const lines = ['Tools visible to Jeden:']
    for (const tool of tools) lines.push(`- ${tool.name}: ${tool.description}`)
    return { handled: true, role: 'system', text: lines.join('\n') }
  }

  if (canonical === 'hotkeys') return { handled: true, role: 'system', text: localHotkeysText() }

  if (command) return { handled: true, role: 'system', text: unsupportedText(command) }

  return { handled: true, role: 'error', text: `Unknown slash command /${parsed.rawName}. Use /help to list OMP-compatible commands known to Jeden.` }
}
