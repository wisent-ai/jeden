import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, readFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { dispatchSlashCommand, formatSlashCommandList, slashCommandHints, SLASH_COMMANDS } from '../src/slash-commands.js'
import { createModeState } from '../src/mode-state.js'

function fakeContext(cwd = '/tmp/work') {
  const tools = [
    { name: 'read_file', description: 'Read files' },
    { name: 'edit', description: 'Edit files' },
  ]
  return {
    args: { cwd, model: 'default', allowWrite: false, allowCommand: false },
    modeState: createModeState(),
    recorder: {
      path() { return '/tmp/session' },
      artifactDir() { return '/tmp/session/artifacts' },
    },
    createToolRegistry() {
      return { list: () => tools }
    },
  }
}

test('slash registry mirrors known OMP command surface', () => {
  const names = SLASH_COMMANDS.map((command) => command.name)
  for (const expected of ['settings', 'setup', 'plan', 'goal', 'model', 'todo', 'session', 'tools', 'mcp', 'memory', 'exit', 'quit']) {
    assert.ok(names.includes(expected), `missing /${expected}`)
  }
  assert.equal(names.length, 58)
  assert.match(formatSlashCommandList(), /\[local\] \/model/)
  assert.match(formatSlashCommandList(), /Jeden slash commands/)
})

test('slash hints match the typed prefix and aliases', () => {
  assert.deepEqual(slashCommandHints('/mod').map((command) => command.name), ['model'])
  assert.deepEqual(slashCommandHints('/providers').map((command) => command.name), ['setup'])
  assert.ok(slashCommandHints('/').length > 5)
})

test('slash dispatcher handles local commands before model execution', async () => {
  const context = fakeContext()

  const help = await dispatchSlashCommand('/help', context)
  assert.equal(help.handled, true)
  assert.match(help.text, /Jeden slash commands/)

  const model = await dispatchSlashCommand('/model gpt-5.5', context)
  assert.equal(model.handled, true)
  assert.equal(context.args.model, 'gpt-5.5')
  assert.match(model.text, /gpt-5\.5/)

  const tools = await dispatchSlashCommand('/tools', context)
  assert.equal(tools.handled, true)
  assert.match(tools.text, /read_file/)

  const settings = await dispatchSlashCommand('/settings', context)
  assert.equal(settings.handled, true)
  assert.match(settings.text, /Jeden settings status/)
  assert.match(settings.text, /Workspace:/)

  const exit = await dispatchSlashCommand('/exit', context)
  assert.equal(exit.handled, true)
  assert.equal(exit.exit, true)
})

test('mcp slash command mutates isolated project mcp config', async () => {
  const cwd = await mkdtemp(join(tmpdir(), 'jeden-mcp-'))
  const context = fakeContext(cwd)

  const added = await dispatchSlashCommand('/mcp add local node server.js --stdio', context)
  assert.equal(added.handled, true)
  assert.equal(added.role, 'system')
  assert.match(added.text, /Added MCP server local/)

  const afterAdd = JSON.parse(await readFile(join(cwd, '.jeden', 'mcp.json'), 'utf8'))
  assert.deepEqual(afterAdd.mcpServers.local, { command: 'node', args: ['server.js', '--stdio'] })

  const disabled = await dispatchSlashCommand('/mcp disable local', context)
  assert.equal(disabled.role, 'system')
  assert.deepEqual(JSON.parse(await readFile(join(cwd, '.jeden', 'mcp.json'), 'utf8')).disabledServers, ['local'])

  const enabled = await dispatchSlashCommand('/mcp enable local', context)
  assert.equal(enabled.role, 'system')
  assert.deepEqual(JSON.parse(await readFile(join(cwd, '.jeden', 'mcp.json'), 'utf8')).disabledServers, [])

  const removed = await dispatchSlashCommand('/mcp remove local', context)
  assert.equal(removed.role, 'system')
  assert.deepEqual(JSON.parse(await readFile(join(cwd, '.jeden', 'mcp.json'), 'utf8')).mcpServers, {})
})

test('ssh slash command mutates isolated project config ssh hosts', async () => {
  const cwd = await mkdtemp(join(tmpdir(), 'jeden-ssh-'))
  const context = fakeContext(cwd)

  const added = await dispatchSlashCommand('/ssh add prod deploy@example.com port=22 identityFile=~/.ssh/prod', context)
  assert.equal(added.handled, true)
  assert.equal(added.role, 'system')

  const afterAdd = JSON.parse(await readFile(join(cwd, '.jeden', 'config.json'), 'utf8'))
  assert.deepEqual(afterAdd.sshHosts.prod, { host: 'deploy@example.com', port: '22', identityFile: '~/.ssh/prod' })

  const listed = await dispatchSlashCommand('/ssh list', context)
  assert.equal(listed.role, 'system')
  assert.match(listed.text, /prod/)

  const removed = await dispatchSlashCommand('/ssh remove prod', context)
  assert.equal(removed.role, 'system')
  assert.deepEqual(JSON.parse(await readFile(join(cwd, '.jeden', 'config.json'), 'utf8')).sshHosts, {})
})
