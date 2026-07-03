import test from 'node:test'
import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'

import { TerminalTui, renderTerminalFrame } from '../src/tui.js'

class FakeInput extends EventEmitter {
  constructor() {
    super()
    this.isTTY = true
    this.raw = false
    this.resumed = false
  }

  resume() {
    this.resumed = true
  }

  setRawMode(value) {
    this.raw = value
  }
}

function createFakeOutput() {
  const writes = []
  return {
    isTTY: true,
    columns: 72,
    rows: 20,
    writes,
    write(value) {
      writes.push(String(value))
    },
  }
}

function createTui() {
  const input = new FakeInput()
  const output = createFakeOutput()
  const tui = new TerminalTui({
    input,
    output,
    cwd: '.',
    sessionPath: '/session/current',
    writeStatus: 'ask',
    commandStatus: 'ask',
  })
  return { input, output, tui }
}

test('renderTerminalFrame draws OMP-style welcome chrome when transcript is empty', () => {
  const frame = renderTerminalFrame({
    cwd: '.',
    sessionPath: '/session/current',
    writeStatus: 'ask',
    commandStatus: 'ask',
    inputText: '',
    cursorIndex: 0,
    columns: 100,
    rows: 32,
    color: false,
  })

  assert.match(frame, /Jeden v0\.1\.0/)
  assert.match(frame, /Welcome back!/)
  assert.match(frame, /Tips/)
  assert.match(frame, /LSP Servers/)
  assert.match(frame, /No LSP servers/)
  assert.match(frame, /Recent sessions/)
  assert.match(frame, /No recent sessions/)
  assert.match(frame, /jeden > ⬢ default/)
})

test('renderTerminalFrame draws transcript, cursor, and editor hints', () => {
  const frame = renderTerminalFrame({
    cwd: '.',
    sessionPath: '/session/current',
    writeStatus: 'ask',
    commandStatus: 'ask',
    messages: [{ role: 'assistant', text: 'hello' }],
    inputText: 'draft',
    cursorIndex: 2,
    columns: 80,
    rows: 24,
    color: false,
  })

  assert.match(frame, /╭ jeden /)
  assert.match(frame, /hello/)
  assert.match(frame, /jeden > ⬢ default/)
  assert.match(frame, /dr▌aft/)
  assert.match(frame, /arrows\/Home\/End edit/)
})

test('TerminalTui installs raw key handling and restores raw mode on close', () => {
  const { input, tui } = createTui()

  tui.start()
  assert.equal(input.raw, true)
  assert.equal(input.resumed, true)
  assert.equal(input.listenerCount('keypress'), 1)

  tui.close()
  assert.equal(input.raw, false)
  assert.equal(input.listenerCount('keypress'), 0)
})

test('TerminalTui edits inside the input buffer with cursor keys', () => {
  const { tui } = createTui()

  tui.onKeypress('a', {})
  tui.onKeypress('c', {})
  tui.onKeypress('', { name: 'left' })
  tui.onKeypress('b', {})
  assert.equal(tui.inputText, 'abc')
  assert.equal(tui.cursorIndex, 2)

  tui.onKeypress('', { name: 'delete' })
  assert.equal(tui.inputText, 'ab')
  assert.equal(tui.cursorIndex, 2)

  tui.onKeypress('', { name: 'home' })
  tui.onKeypress('>', {})
  assert.equal(tui.inputText, '>ab')
  assert.equal(tui.cursorIndex, 1)

  tui.onKeypress('', { name: 'end' })
  tui.onKeypress('!', {})
  assert.equal(tui.inputText, '>ab!')
  assert.equal(tui.cursorIndex, 4)
})

test('TerminalTui stores submitted prompts and navigates command history', () => {
  const { tui } = createTui()
  let submitted = null

  tui.inputText = 'first command'
  tui.cursorIndex = tui.inputText.length
  tui.pending = { type: 'input', resolve(value) { submitted = value } }
  tui.onKeypress('\r', { name: 'return' })

  assert.equal(submitted, 'first command')
  assert.deepEqual(tui.history, ['first command'])
  assert.equal(tui.inputText, '')

  tui.inputText = 'draft'
  tui.cursorIndex = tui.inputText.length
  tui.onKeypress('', { name: 'up' })
  assert.equal(tui.inputText, 'first command')
  assert.equal(tui.cursorIndex, 'first command'.length)

  tui.onKeypress('', { name: 'down' })
  assert.equal(tui.inputText, 'draft')
  assert.equal(tui.cursorIndex, 'draft'.length)
})

test('TerminalTui treats PTY newline input as submit while Ctrl-J remains multiline insert', () => {
  const { tui } = createTui()
  let submitted = null

  tui.inputText = 'from pty'
  tui.cursorIndex = tui.inputText.length
  tui.pending = { type: 'input', resolve(value) { submitted = value } }
  tui.onKeypress('\n', {})
  assert.equal(submitted, 'from pty')

  tui.onKeypress('a', {})
  tui.onKeypress('\n', { ctrl: true, name: 'j' })
  tui.onKeypress('b', {})
  assert.equal(tui.inputText, 'a\nb')
})

test('TerminalTui confirm mode accepts y and rejects n, escape, and ctrl-c without leaving payload text', () => {
  for (const [str, key, expected] of [
    ['y', {}, true],
    ['n', {}, false],
    ['', { name: 'escape' }, false],
    ['\u0003', { ctrl: true, name: 'c' }, false],
  ]) {
    const { tui } = createTui()
    let resolved = null
    tui.mode = 'confirm'
    tui.inputText = 'write tool: edit\nlarge payload'
    tui.cursorIndex = tui.inputText.length
    tui.pending = { type: 'confirm', resolve(value) { resolved = value } }

    tui.onKeypress(str, key)

    assert.equal(resolved, expected)
    assert.equal(tui.mode, 'input')
    assert.equal(tui.inputText, '')
    assert.equal(tui.cursorIndex, 0)
  }
})

test('TerminalTui resize handler redraws the frame', () => {
  const { output, tui } = createTui()
  const before = output.writes.length

  tui.resizeHandler()

  assert.equal(output.writes.length, before + 1)
})
