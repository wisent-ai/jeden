import { chatCompletion, modelRouterConfig } from './model-router.js'
import { parseAction, formatToolResult } from './protocol.js'
import { systemPrompt } from './policy.js'
import { createToolRegistry } from './tools.js'

export async function runJeden({ task, cwd = process.cwd(), allowWrite = false, maxSteps = 8, config = modelRouterConfig(), chat = chatCompletion } = {}) {
  if (!task || typeof task !== 'string') throw new Error('task is required')
  const tools = createToolRegistry({ cwd, allowWrite })
  const messages = [
    { role: 'system', content: systemPrompt(tools.list()) },
    { role: 'user', content: task },
  ]

  for (let step = 1; step <= maxSteps; step += 1) {
    const content = await chat({ messages, config })
    const action = parseAction(content)
    messages.push({ role: 'assistant', content })

    if (action.action === 'final') {
      return { text: action.text, steps: step }
    }

    const result = await tools.execute(action.tool, action.input)
    messages.push({ role: 'user', content: formatToolResult(result) })
  }

  throw new Error(`max steps exceeded: ${maxSteps}`)
}
