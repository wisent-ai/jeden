import { chatCompletion, modelRouterConfig } from './model-router.js'
import { parseAction, formatToolResult } from './protocol.js'
import { systemPrompt } from './policy.js'
import { createToolRegistry } from './tools.js'

function approvalKind(tool) {
  if (tool === 'write_file' || tool === 'apply_patch') return 'write'
  if (tool === 'run_command' || tool === 'run_package_script') return 'command'
  return null
}

async function maybeApprove({ action, allowWrite, allowCommand, approveTool, recorder }) {
  const kind = approvalKind(action.tool)
  if (!kind) return { approved: true }
  if (kind === 'write' && allowWrite) return { approved: true }
  if (kind === 'command' && allowCommand) return { approved: true }
  if (!approveTool) return { approved: true }
  const approved = await approveTool({ kind, tool: action.tool, input: action.input })
  await recorder?.record('approval', { tool: action.tool, kind, approved })
  if (!approved) return { approved: false, result: { ok: false, error: `${action.tool} denied by user approval` } }
  return { approved: true }
}


export async function runJeden({
  task,
  cwd = process.cwd(),
  allowWrite = false,
  allowCommand = false,
  maxSteps = 8,
  config = modelRouterConfig(),
  chat = chatCompletion,
  recorder = null,
  approveTool = null,
} = {}) {
  if (!task || typeof task !== 'string') throw new Error('task is required')
  const tools = createToolRegistry({ cwd, allowWrite: allowWrite || Boolean(approveTool), allowCommand: allowCommand || Boolean(approveTool) })
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

    const approval = await maybeApprove({ action, allowWrite, allowCommand, approveTool, recorder })
    const result = approval.approved ? await tools.execute(action.tool, action.input) : approval.result
    await recorder?.record('tool_result', { step, tool: action.tool, result })
    messages.push({ role: 'user', content: formatToolResult(result) })
  }

  throw new Error(`max steps exceeded: ${maxSteps}`)
}
