import test from 'node:test'
import assert from 'node:assert/strict'

import { dispatchSlashCommand, formatSlashCommandList, slashCommandHints, SLASH_COMMANDS } from '../src/slash-commands.js'
import { createModeState } from '../src/mode-state.js'

function fakeContext() {
  const tools = [
    { name: 'read_file', description: 'Read files' },
    { name: 'edit', description: 'Edit files' },
  ]
  return {
    args: { cwd: '/tmp/work', model: 'default', allowWrite: false, allowCommand: false },
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
