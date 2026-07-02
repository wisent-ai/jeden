export function extractJsonObject(text) {
  const raw = String(text || '').trim()
  if (!raw) throw new Error('model returned empty content')
  if (raw.slice(0, 1) === '{' && raw.slice(-1) === '}') return raw
  const start = raw.indexOf('{')
  const end = raw.lastIndexOf('}')
  if (start === -1 || end === -1 || end <= start) {
    throw new Error(`model returned non-json content: ${raw.slice(0, 200)}`)
  }
  return raw.slice(start, end + 1)
}


function parseToolAction(json) {
  if (typeof json.tool !== 'string') throw new Error('tool action requires tool')
  const input = json.input && typeof json.input === 'object' && !Array.isArray(json.input) ? json.input : {}
  return { action: 'tool', tool: json.tool, input }
}
export function parseAction(text) {
  const json = JSON.parse(extractJsonObject(text))
  if (!json || typeof json !== 'object' || Array.isArray(json)) {
    throw new Error('action must be a JSON object')
  }
  if (json.action === 'final') {
    if (typeof json.text !== 'string') throw new Error('final action requires text')
    return { action: 'final', text: json.text }
  }
  if (json.action === 'tool') {
    return parseToolAction(json)
  }
  if (json.action === 'tools') {
    if (!Array.isArray(json.tools) || json.tools.length === 0) throw new Error('tools action requires tools')
    return { action: 'tools', tools: json.tools.map(parseToolAction) }
  }
  throw new Error(`unknown action: ${String(json.action)}`)
}

export function formatToolResult(result) {
  return JSON.stringify({ type: 'tool_result', result })
}
