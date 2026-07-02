import { chatCompletion, modelRouterConfig } from './model-router.js'
import { parseAction, formatToolResult } from './protocol.js'
import { systemPrompt } from './policy.js'
import { createToolRegistry } from './tools.js'
import { loadCustomTools } from './custom-tools.js'
import { toolHookEvent, postToolHookEvent } from './hooks.js'


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
async function runHook({ hookRunner, event, payload, recorder }) {
  if (!hookRunner) return { decision: 'pass' }
  const result = await hookRunner.run(event, payload)
  await recorder?.record('hook', { event, result })
  return result
}

function hookPayload({ task, cwd, action = null, result = null, finalText = null }) {
  return {
    runtime: 'jeden',
    project_dir: cwd,
    user_message: task,
    last_assistant_message: finalText || null,
    tool_name: action?.tool || null,
    tool_input: action?.input || null,
    tool: action ? { name: action.tool, input: action.input } : null,
    tool_result: result,
  }
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
  hookRunner = null,
} = {}) {
  if (!task || typeof task !== 'string') throw new Error('task is required')
  await recorder?.ensure?.()
  const builtInToolNames = createToolRegistry({ cwd, allowWrite, allowCommand, artifactDir: recorder?.artifactDir?.() || null }).list().map((tool) => tool.name)
  const custom = await loadCustomTools({ cwd, builtInToolNames })
  const tools = createToolRegistry({ cwd, allowWrite: allowWrite || Boolean(approveTool), allowCommand: allowCommand || Boolean(approveTool), artifactDir: recorder?.artifactDir?.() || null, customTools: custom.tools })
  if (custom.errors.length > 0) await recorder?.record('custom_tool_errors', { errors: custom.errors })
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
      const stop = await runHook({
        hookRunner,
        event: 'stop',
        payload: hookPayload({ task, cwd, finalText: action.text }),
        recorder,
      })
      if (stop.decision === 'block') {
        messages.push({
          role: 'user',
          content: `Stop hook blocked the final answer: ${stop.reason}. Continue executing available work in this same turn, or finish only if there is a concrete external blocker.`,
        })
        continue
      }
      await recorder?.record('final', { step, text: action.text })
      return { text: action.text, steps: step, sessionPath: recorder?.path?.() || null }
    }

    const preHook = await runHook({
      hookRunner,
      event: toolHookEvent(action.tool),
      payload: hookPayload({ task, cwd, action }),
      recorder,
    })
    const approval = preHook.decision === 'block'
      ? { approved: false, result: { ok: false, error: `hook blocked ${action.tool}: ${preHook.reason}` } }
      : await maybeApprove({ action, allowWrite, allowCommand, approveTool, recorder })
    const result = approval.approved ? await tools.execute(action.tool, action.input) : approval.result
    await recorder?.record('tool_result', { step, tool: action.tool, result })
    const postEvent = postToolHookEvent(action.tool)
    if (postEvent) {
      await runHook({ hookRunner, event: postEvent, payload: hookPayload({ task, cwd, action, result }), recorder })
    }
    messages.push({ role: 'user', content: formatToolResult(result) })
  }

  throw new Error(`max steps exceeded: ${maxSteps}`)
}
