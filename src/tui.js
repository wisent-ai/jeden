import { emitKeypressEvents } from 'node:readline'

const ANSI = {
  reset: '\x1b[0m',
  dim: '\x1b[2m',
  bold: '\x1b[1m',
  cyan: '\x1b[36m',
  green: '\x1b[32m',
  yellow: '\x1b[33m',
  magenta: '\x1b[35m',
  red: '\x1b[31m',
  blue: '\x1b[34m',
}

function useColor(output) {
  return Boolean(output?.isTTY) && !process.env.NO_COLOR
}

function paint(value, color, enabled) {
  if (!enabled) return String(value)
  return `${ANSI[color] || ''}${value}${ANSI.reset}`
}

function stripAnsi(value) {
  return String(value).replace(/\x1b\[[0-9;]*m/g, '')
}

function visibleLength(value) {
  return stripAnsi(value).length
}

function padVisible(value, width) {
  const extra = Math.max(width - visibleLength(value), 0)
  return `${value}${' '.repeat(extra)}`
}

function wrapLine(line, width) {
  const plain = stripAnsi(line)
  if (plain.length <= width) return [line]
  const out = []
  let rest = plain
  while (rest.length > width) {
    out.push(rest.slice(0, width))
    rest = rest.slice(width)
  }
  out.push(rest)
  return out
}

function box(title, rows, width, colorEnabled) {
  const cleanTitle = ` ${title} `
  const inner = Math.max(width - 4, cleanTitle.length + 2, 8)
  const normalized = rows.flatMap((row) => String(row).split('\n')).flatMap((row) => wrapLine(row, inner))
  const top = `${paint('╭', 'cyan', colorEnabled)}${paint(cleanTitle, 'bold', colorEnabled)}${paint('─'.repeat(Math.max(inner + 2 - cleanTitle.length, 0)), 'cyan', colorEnabled)}${paint('╮', 'cyan', colorEnabled)}`
  const body = normalized.map((row) => `${paint('│', 'cyan', colorEnabled)} ${padVisible(row, inner)} ${paint('│', 'cyan', colorEnabled)}`)
  const bottom = `${paint('╰', 'cyan', colorEnabled)}${paint('─'.repeat(inner + 2), 'cyan', colorEnabled)}${paint('╯', 'cyan', colorEnabled)}`
  return [top, ...body, bottom]
}

function roleColor(role) {
  if (role === 'assistant') return 'magenta'
  if (role === 'error') return 'red'
  if (role === 'system') return 'yellow'
  return 'cyan'
}

function formatMessage(message, width, colorEnabled) {
  const color = roleColor(message.role)
  const title = message.role === 'assistant' ? 'jeden' : message.role
  return box(title, String(message.text || '').split('\n'), width, colorEnabled).map((line) => paint(line, color, colorEnabled))
}

function withCursorMarker(value, cursorIndex, colorEnabled) {
  const text = String(value)
  const index = Math.max(0, Math.min(cursorIndex, text.length))
  return `${text.slice(0, index)}${paint('▌', 'yellow', colorEnabled)}${text.slice(index)}`
}

export function renderTerminalFrame({ cwd, sessionPath, writeStatus, commandStatus, messages = [], inputText = '', cursorIndex = 0, mode = 'input', busy = false, columns = 100, rows = 30, color = false }) {
  const width = Math.max(Math.min(columns, 120), 50)
  const header = box('Jeden', [
    `${paint('cwd', 'dim', color)} ${cwd}`,
    `${paint('session', 'dim', color)} ${sessionPath}`,
    `${paint('write', 'dim', color)} ${paint(writeStatus, writeStatus === 'enabled' ? 'green' : 'yellow', color)}   ${paint('command', 'dim', color)} ${paint(commandStatus, commandStatus === 'enabled' ? 'green' : 'yellow', color)}`,
    `${paint('visual edit', 'dim', color)} ${paint('enabled', 'green', color)} ${paint('(approval-gated)', 'dim', color)}`,
    `${paint('state', 'dim', color)} ${busy ? paint('thinking', 'yellow', color) : paint('ready', 'green', color)}`,
  ], width, color)

  const inputTitle = mode === 'confirm' ? 'approve y/N' : 'you'
  const visibleInput = mode === 'input' ? withCursorMarker(inputText, cursorIndex, color) : inputText
  const inputRows = visibleInput.length > 0 ? visibleInput.split('\n') : [paint('▌', 'yellow', color)]
  const inputBox = box(inputTitle, inputRows, width, color)
  const reserved = header.length + inputBox.length + 2
  const availableMessageRows = Math.max(rows - reserved, 3)
  const messageLines = messages.flatMap((message) => formatMessage(message, width, color))
  const visibleMessages = messageLines.slice(-availableMessageRows)
  return [
    '\x1b[2J\x1b[H\x1b[?25l',
    ...header,
    '',
    ...visibleMessages,
    ...Array.from({ length: Math.max(availableMessageRows - visibleMessages.length, 0) }, () => ''),
    ...inputBox,
    paint(mode === 'confirm' ? 'Press y to approve, n/esc/ctrl-c to deny.' : 'Enter sends. Ctrl-J inserts newline. ←/→/Home/End edit. ↑/↓ history. Ctrl-C exits.', 'dim', color),
    '\x1b[?25h',
  ].join('\n')
}

export class TerminalTui {
  constructor({ input, output, cwd, sessionPath, writeStatus, commandStatus }) {
    this.input = input
    this.output = output
    this.cwd = cwd
    this.sessionPath = sessionPath
    this.writeStatus = writeStatus
    this.commandStatus = commandStatus
    this.color = useColor(output)
    this.messages = []
    this.inputText = ''
    this.mode = 'input'
    this.busy = false
    this.pending = null
    this.keyHandler = this.onKeypress.bind(this)
    this.cursorIndex = 0
    this.history = []
    this.historyIndex = null
    this.historyDraft = ''
    this.resizeHandler = () => this.render()
  }

  start() {
    emitKeypressEvents(this.input)
    if (this.input.isTTY) this.input.setRawMode(true)
    this.input.resume()
    this.input.on('keypress', this.keyHandler)
    process.on('SIGWINCH', this.resizeHandler)
    this.render()
  }

  close() {
    this.input.off('keypress', this.keyHandler)
    process.off('SIGWINCH', this.resizeHandler)
    if (this.input.isTTY) this.input.setRawMode(false)
    this.output.write('\x1b[?25h\n')
  }

  render() {
    this.output.write(renderTerminalFrame({
      cwd: this.cwd,
      sessionPath: this.sessionPath,
      writeStatus: this.writeStatus,
      commandStatus: this.commandStatus,
      messages: this.messages,
      inputText: this.inputText,
      cursorIndex: this.cursorIndex,
      mode: this.mode,
      busy: this.busy,
      columns: this.output.columns || 100,
      rows: this.output.rows || 30,
      color: this.color,
    }))
  }

  push(role, text) {
    this.messages.push({ role, text })
    this.render()
  }

  setBusy(value) {
    this.busy = Boolean(value)
    this.render()
  }

  clearInput() {
    this.inputText = ''
    this.cursorIndex = 0
    this.historyIndex = null
    this.historyDraft = ''
  }

  prompt(label = 'you') {
    this.mode = 'input'
    this.clearInput()
    this.promptLabel = label
    this.render()
    return new Promise((resolve) => {
      this.pending = { type: 'input', resolve }
    })
  }

  confirm({ tool, kind, input }) {
    const payload = JSON.stringify(input, null, 2)
    const preview = payload.length > 1200 ? `${payload.slice(0, 1200)}…` : payload
    this.mode = 'confirm'
    this.inputText = `${kind} tool: ${tool}\n${preview}`
    this.cursorIndex = this.inputText.length
    this.render()
    return new Promise((resolve) => {
      this.pending = { type: 'confirm', resolve }
    })
  }

  ask({ question, options }) {
    const suffix = options.length > 0 ? ` (${options.join('/')})` : ''
    this.push('system', `${question}${suffix}`)
    return this.prompt('answer')
  }

  submitInput() {
    const value = this.inputText.trim()
    if (value) this.history.push(value)
    this.clearInput()
    const pending = this.pending
    this.pending = null
    if (value) this.messages.push({ role: 'user', text: value })
    this.render()
    pending?.resolve(value)
  }

  insertText(value) {
    const text = String(value)
    this.inputText = `${this.inputText.slice(0, this.cursorIndex)}${text}${this.inputText.slice(this.cursorIndex)}`
    this.cursorIndex += text.length
    this.historyIndex = null
    this.render()
  }

  moveHistory(delta) {
    if (this.history.length === 0) return
    if (this.historyIndex === null) {
      this.historyDraft = this.inputText
      this.historyIndex = delta < 0 ? this.history.length - 1 : null
    } else {
      this.historyIndex += delta
    }
    if (this.historyIndex === null || this.historyIndex >= this.history.length) {
      this.historyIndex = null
      this.inputText = this.historyDraft
    } else {
      this.historyIndex = Math.max(0, this.historyIndex)
      this.inputText = this.history[this.historyIndex]
    }
    this.cursorIndex = this.inputText.length
    this.render()
  }

  onKeypress(str, key = {}) {
    if (key.ctrl && key.name === 'c') {
      const pending = this.pending
      this.pending = null
      this.clearInput()
      this.mode = 'input'
      this.render()
      pending?.resolve(pending?.type === 'confirm' ? false : '/exit')
      return
    }
    if (this.mode === 'confirm') {
      if (key.name === 'escape' || str === 'n' || str === 'N') {
        const pending = this.pending
        this.pending = null
        this.mode = 'input'
        this.clearInput()
        this.render()
        pending?.resolve(false)
        return
      }
      if (str === 'y' || str === 'Y') {
        const pending = this.pending
        this.pending = null
        this.mode = 'input'
        this.clearInput()
        this.render()
        pending?.resolve(true)
      }
      return
    }
    if (key.name === 'left') {
      this.cursorIndex = Math.max(0, this.cursorIndex - 1)
      this.render()
      return
    }
    if (key.name === 'right') {
      this.cursorIndex = Math.min(this.inputText.length, this.cursorIndex + 1)
      this.render()
      return
    }
    if (key.name === 'home' || (key.ctrl && key.name === 'a')) {
      this.cursorIndex = 0
      this.render()
      return
    }
    if (key.name === 'end' || (key.ctrl && key.name === 'e')) {
      this.cursorIndex = this.inputText.length
      this.render()
      return
    }
    if (key.name === 'up') {
      this.moveHistory(-1)
      return
    }
    if (key.name === 'down') {
      this.moveHistory(1)
      return
    }
    if (key.name === 'return') {
      this.submitInput()
      return
    }
    if (key.ctrl && key.name === 'j') {
      this.insertText('\n')
      this.render()
      return
    }
    if (key.name === 'backspace') {
      if (this.cursorIndex > 0) {
        this.inputText = `${this.inputText.slice(0, this.cursorIndex - 1)}${this.inputText.slice(this.cursorIndex)}`
        this.cursorIndex -= 1
        this.historyIndex = null
        this.render()
      }
      return
    }
    if (key.name === 'delete') {
      if (this.cursorIndex < this.inputText.length) {
        this.inputText = `${this.inputText.slice(0, this.cursorIndex)}${this.inputText.slice(this.cursorIndex + 1)}`
        this.historyIndex = null
        this.render()
      }
      return
    }
    if (str && !key.ctrl && !key.meta) {
      this.insertText(str)
    }
  }
}

export function createTerminalTui(options) {
  if (!options.input?.isTTY || !options.output?.isTTY) return null
  return new TerminalTui(options)
}
