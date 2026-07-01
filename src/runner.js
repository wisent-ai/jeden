import { chatCompletion, modelRouterConfig } from './model-router.js'
import { parseAction, formatToolResult } from './protocol.js'
import { systemPrompt } from './policy.js'
import { createToolRegistry } from './tools.js'

export async function runJeden({
  task,
  cwd = process.cwd(),
  allowWrite = false,
  allowCommand = false,
  maxSteps = 8,
  config = modelRouterConfig(),
  chat = chatCompletion,
  recorder = null,
} = {}) {
  if (!task || typeof task !== 'string') throw new Error('task is required')
  const tools = createToolRegistry({ cwd, allowWrite, allowCommand })
  const messages = [
    { role: 'system', content: systemPrompt(tools.list()) },
    { role: 'user', content: task },
  ]
  await recorder?.record('user', { task, cwd, allowWrite, allowCommand, maxSteps })

  for (let step = 1; step <= maxSteps; step += 1) {
    const content = await chat({ messages, config })
    await recorder?.record('assistant_raw', { step, content })
    const action = parseAction(content)
    await recorder?.record('action', { step, action })
    messages.push({ role: 'assistant', content })

    if (action.action === 'final') {
      await recorder?.record('final', { step, text: action.text })
      return { text: action.text, steps: step, sessionPath: recorder?.path?.() || null }
    }

    const result = await tools.execute(action.tool, action.input)
    await recorder?.record('tool_result', { step, tool: action.tool, result })
    messages.push({ role: 'user', content: formatToolResult(result) })
  }

  throw new Error(`max steps exceeded: ${maxSteps}`)
}
