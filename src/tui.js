import { emitKeypressEvents } from 'node:readline'
import { BRAND, WISENT_MARK } from './brand.js'
import { formatSlashCommand, slashCommandHints } from './slash-commands.js'

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
  const title = message.role === 'assistant' ? BRAND.assistantTitle : message.role
  return box(title, String(message.text || '').split('\n'), width, colorEnabled).map((line) => paint(line, color, colorEnabled))
}

function withCursorMarker(value, cursorIndex, colorEnabled) {
  const text = String(value)
  const index = Math.max(0, Math.min(cursorIndex, text.length))
  return `${text.slice(0, index)}${paint('▌', 'yellow', colorEnabled)}${text.slice(index)}`
}

function center(value, width) {
  const text = String(value)
  const extra = Math.max(width - visibleLength(text), 0)
  const left = Math.floor(extra / 2)
  return `${' '.repeat(left)}${text}${' '.repeat(extra - left)}`
}

function divider(width) {
  return '─'.repeat(Math.max(width, 0))
}

function welcomePanel({ width, model = 'default', cwd = '.', writeStatus = 'ask', commandStatus = 'ask', color }) {
  const title = `${BRAND.product} ${BRAND.app} ${BRAND.version}`
  const markWidth = Math.max(...WISENT_MARK.map((line) => visibleLength(line)))
  const mark = (index) => padVisible(WISENT_MARK[index], markWidth)
  const rows = [
    `${BRAND.product} private agent harness`,
    `Model route: ${model || 'default'}`,
    `Workspace: ${cwd}`,
    '',
    `${mark(0)}   Controls`,
    `${mark(1)}   # prompt actions   / commands`,
    `${mark(2)}   ! shell            $ node/python`,
    `${mark(3)}   Enter sends        Ctrl-J newline`,
    `${mark(4)}   arrows/Home/End edit`,
    '',
    `Tool gates: write ${writeStatus} · command ${commandStatus}`,
    'Sessions: local history and artifacts',
    'MCP: adapters loaded through Wisent registry',
  ]
  return box(title, rows, width, color)
}

function brandHeader({ width, model = 'default', cwd = '.', color }) {
  const inner = Math.max(width - 4, 8)
  const contentWidth = Math.max(inner - 2, 1)
  const label = `${WISENT_MARK[0]}  ${BRAND.product} ${BRAND.app} ${BRAND.version} · ${model || 'default'} · ${cwd}`
  const row = visibleLength(label) > contentWidth ? `${stripAnsi(label).slice(0, Math.max(contentWidth - 1, 0))}…` : label
  return `${paint('╭─', 'cyan', color)} ${padVisible(row, contentWidth)} ${paint('─╮', 'cyan', color)}`
}


function slashHintPanel({ inputText, width, color }) {
  const text = String(inputText || '')
  if (!text.startsWith('/') || text.includes('\n')) return []
  const hints = slashCommandHints(text, { limit: 6 })
  if (hints.length === 0) return []
  return box('slash commands', hints.map((command) => formatSlashCommand(command)), width, color)
}

function compactPrompt({ width, model = 'default', cwd, writeStatus, commandStatus, inputText, cursorIndex, mode, busy, color }) {
  const inner = Math.max(width - 2, 48)
  const state = busy ? paint('thinking', 'yellow', color) : paint('ready', 'green', color)
  const label = mode === 'confirm'
    ? ` approve ${state} `
    : ` wisent > ${model || 'default'} · ${state} > ${cwd} > write ${writeStatus} > command ${commandStatus} › `
  const safeLabel = visibleLength(label) > inner - 4 ? `${stripAnsi(label).slice(0, inner - 7)}… ▶ ` : label
  const top = `${paint('╭──', 'cyan', color)}${safeLabel}${paint(divider(Math.max(inner - visibleLength(safeLabel) - 2, 0)), 'cyan', color)}${paint('╮', 'cyan', color)}`
  const visibleInput = mode === 'input' ? withCursorMarker(inputText, cursorIndex, color) : inputText
  const inputRows = (visibleInput.length > 0 ? visibleInput : paint('▌', 'yellow', color)).split('\n').flatMap((row) => wrapLine(row, inner - 4))
  const rows = inputRows.slice(0, 4)
  const body = rows.slice(0, -1).map((row) => `${paint('│', 'cyan', color)} ${padVisible(row, inner - 2)} ${paint('│', 'cyan', color)}`)
  const last = rows[rows.length - 1] || paint('▌', 'yellow', color)
  const bottom = `${paint('╰─', 'cyan', color)} ${padVisible(last, inner - 4)} ${paint('─╯', 'cyan', color)}`
  const hint = mode === 'confirm'
    ? 'Press y to approve, n/esc/ctrl-c to deny.'
    : 'Enter sends · Ctrl-J newline · arrows/Home/End edit · ↑/↓ history · Ctrl-C exits'
  return [top, ...body, bottom, paint(` ${hint}`, 'dim', color)]
}

export function renderTerminalFrame({ cwd, sessionPath, writeStatus, commandStatus, model = 'default', messages = [], inputText = '', cursorIndex = 0, mode = 'input', busy = false, columns = 100, rows = 30, color = false }) {
  const width = Math.max(Math.min(columns, 120), 50)
  const header = brandHeader({ width, model, cwd, color })
  const prompt = compactPrompt({ width, model, cwd, writeStatus, commandStatus, inputText, cursorIndex, mode, busy, color })
  const slashHints = mode === 'input' ? slashHintPanel({ inputText, width, color }) : []
  const reserved = 1 + prompt.length + slashHints.length + 1
  const availableRows = Math.max(rows - reserved, 4)
  const messageLines = messages.flatMap((message) => formatMessage(message, width, color))
  const mainLines = messageLines.length > 0
    ? messageLines.slice(-availableRows)
    : welcomePanel({ width, model, cwd, writeStatus, commandStatus, color }).slice(0, availableRows)
  return [
    '\x1b[2J\x1b[H\x1b[?25l',
    header,
    ...mainLines,
    ...Array.from({ length: Math.max(availableRows - mainLines.length, 0) }, () => ''),
    ...slashHints,
    ...prompt,
    '\x1b[?25h',
  ].join('\n')
}

export class TerminalTui {
  constructor({ input, output, cwd, sessionPath, writeStatus, commandStatus, model }) {
    this.input = input
    this.output = output
    this.cwd = cwd
    this.sessionPath = sessionPath
    this.writeStatus = writeStatus
    this.commandStatus = commandStatus
    this.model = model || process.env.JEDEN_MODEL || process.env.MODEL || 'default'
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
      model: this.model,
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

  setModel(value) {
    this.model = value || 'default'
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
    const pending = this.pending
    if (!pending) {
      this.clearInput()
      this.render()
      return
    }
    const value = this.inputText.trim()
    if (value) this.history.push(value)
    this.clearInput()
    this.pending = null
    if (value) this.messages.push({ role: 'user', text: value })
    this.render()
    pending.resolve(value)
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
    if (!this.pending) return
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
    if (key.name === 'return' || str === '\r' || (str === '\n' && !(key.ctrl && key.name === 'j'))) {
      this.submitInput()
      return
    }
    if (key.ctrl && key.name === 'j') {
      this.insertText('\n')
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
