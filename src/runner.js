import { chatCompletion, modelRouterConfig } from './model-router.js'
import { parseAction, formatToolResult } from './protocol.js'
import { systemPrompt } from './policy.js'
import { createToolRegistry, toolCapability } from './tools.js'
import { loadCustomTools } from './custom-tools.js'
import { formatProjectContext, loadProjectContext } from './context.js'
import { toolHookEvent, postToolHookEvent } from './hooks.js'
import { closeMcpClients, loadMcpToolAdapters } from './mcp.js'
import { recordUsageEvent } from './usage.js'
import { buildMemoryContext, learnFromCompletedRun } from './memory.js'
import { errorMessage } from './self-repair.js'

const MAX_TOOL_RESULT_BYTES = 64_000

function safeArtifactName(step, tool) {
  const safeTool = String(tool || 'tool').replace(/[^a-zA-Z0-9._-]/g, '_')
  return `tool-result-${step}-${safeTool}.json`
}

async function compactToolResult({ result, recorder, step, tool }) {
  const payload = JSON.stringify(result)
  if (Buffer.byteLength(payload, 'utf8') <= MAX_TOOL_RESULT_BYTES) return result
  const preview = payload.slice(0, 4_000)
  const compacted = {
    ok: result?.ok ?? true,
    truncated: true,
    bytes: Buffer.byteLength(payload, 'utf8'),
    preview,
  }
  if (recorder?.writeArtifact) {
    const path = await recorder.writeArtifact(safeArtifactName(step, tool), JSON.stringify(result, null, 2))
    return { ...compacted, artifact: path }
  }
  return compacted
}

function inputFieldSchema(spec) {
  const text = String(spec || '').toLowerCase()
  const schema = {}
  if (text.indexOf('number') !== -1) schema.type = 'number'
  else if (text.indexOf('boolean') !== -1) schema.type = 'boolean'
  else if (text.indexOf('array') !== -1) schema.type = 'array'
  else if (text.indexOf('object') !== -1) schema.type = 'object'
  else schema.type = 'string'
  if (text.indexOf('optional') !== -1) schema.description = String(spec)
  else schema.description = String(spec)
  return schema
}

function inputObjectSchema(input = {}) {
  if (input?.type === 'object' && input.properties && typeof input.properties === 'object') {
    return {
      ...input,
      additionalProperties: input.additionalProperties ?? false,
    }
  }
  const properties = {}
  const required = []
  for (const [name, spec] of Object.entries(input || {})) {
    properties[name] = inputFieldSchema(spec)
    if (String(spec || '').toLowerCase().indexOf('required') !== -1) required.push(name)
  }
  return {
    type: 'object',
    properties,
    required,
    additionalProperties: false,
  }
}

function toOpenAIToolSpecs(list) {
  return list.map((tool) => ({
    type: 'function',
    function: {
      name: tool.name,
      description: tool.description,
      parameters: inputObjectSchema(tool.input),
    },
  }))
}


function approvalKind(tool) {
  return toolCapability(tool).permission || null
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

function actionWithHookInput(action, hookResult) {
  if (!hookResult?.toolInput) return action
  return { ...action, input: hookResult.toolInput }
}

function canParallelizeToolActions(actions, hookRunner, approveTool) {
  if (hookRunner || approveTool || actions.length < 2) return false
  return actions.every((action) => {
    const capability = toolCapability(action.tool)
    return !capability.permission && !capability.postHook
  })
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
  maxTokens = 2048,
  config = modelRouterConfig(),
  chat = chatCompletion,
  recorder = null,
  approveTool = null,
  hookRunner = null,
  askUser = null,
  priorContext = '',
  priorMessages = [],
  memory = true,
} = {}) {
  if (!task || typeof task !== 'string') throw new Error('task is required')
  await recorder?.ensure?.()
  const builtInToolNames = createToolRegistry({ cwd, allowWrite, allowCommand, artifactDir: recorder?.artifactDir?.() || null }).list().map((tool) => tool.name)
  const custom = await loadCustomTools({ cwd, builtInToolNames, allowCommand: allowCommand || Boolean(approveTool) })
  const mcp = await loadMcpToolAdapters({ cwd })
  const allCustomTools = [...custom.tools, ...mcp.tools]
  const tools = createToolRegistry({ cwd, allowWrite: allowWrite || Boolean(approveTool), allowCommand: allowCommand || Boolean(approveTool), artifactDir: recorder?.artifactDir?.() || null, customTools: allCustomTools, askUser })
  if (custom.errors.length > 0 || mcp.errors.length > 0) await recorder?.record('custom_tool_errors', { errors: [...custom.errors, ...mcp.errors] })
  const contextFiles = await loadProjectContext({ cwd })
  const contextText = formatProjectContext(contextFiles)
  if (contextFiles.length > 0) await recorder?.record('project_context', { files: contextFiles.map((file) => file.path) })
  let memoryContext = { records: [], text: '' }
  if (memory) {
    try {
      memoryContext = await buildMemoryContext({ cwd, task })
      if (memoryContext.records.length > 0) await recorder?.record('memory_recall', { ids: memoryContext.records.map((record) => record.id), count: memoryContext.records.length })
    } catch (error) {
      await recorder?.record('memory_error', { stage: 'recall', error: error.message })
    }
  }
  const systemParts = [systemPrompt(tools.list()), contextText, memoryContext.text].filter(Boolean)
  const messages = [
    { role: 'system', content: systemParts.join('\n\n') },
  ]
  if (Array.isArray(priorMessages) && priorMessages.length > 0) messages.push(...priorMessages)
  else if (priorContext) messages.push({ role: 'user', content: `Previous session context:\n\n${priorContext}` })
  messages.push({ role: 'user', content: task })
  await recorder?.record('user', { task, cwd, allowWrite, allowCommand, maxSteps, maxTokens })

  try {
    const toolSpecs = toOpenAIToolSpecs(tools.list())
    for (let step = 1; step <= maxSteps; step += 1) {
    const content = await chat({ messages, config, tools: toolSpecs, maxTokens })
    try {
      await recordUsageEvent({ cwd, model: config.model, serviceTier: config.serviceTier, messages, output: content, step })
    } catch (usageError) {
      await recorder?.record('usage_error', { message: usageError instanceof Error ? usageError.message : String(usageError) })
    }
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
      if (memory) {
        try {
          const learnedMemory = await learnFromCompletedRun({ cwd, task, finalText: action.text, runId: recorder?.id || null, sessionPath: recorder?.path?.() || null })
          if (learnedMemory) await recorder?.record('memory_learned', { id: learnedMemory.id, kind: learnedMemory.kind, confidence: learnedMemory.confidence })
        } catch (error) {
          await recorder?.record('memory_error', { stage: 'learn', error: error.message })
        }
      }
      return { text: action.text, steps: step, sessionPath: recorder?.path?.() || null }
    }

    const toolActions = action.action === 'tools' ? action.tools : [action]
    let results = []
    if (canParallelizeToolActions(toolActions, hookRunner, approveTool)) {
      const parallel = await Promise.all(toolActions.map((toolAction) => tools.execute(toolAction.tool, toolAction.input)))
      for (let i = 0; i < toolActions.length; i += 1) {
        const toolAction = toolActions[i]
        const result = parallel[i]
        const compactedResult = await compactToolResult({ result, recorder, step, tool: toolAction.tool })
        results.push({ tool: toolAction.tool, result: compactedResult })
        await recorder?.record('tool_result', { step, tool: toolAction.tool, requested: toolAction, effective: toolAction, parallel: true, result })
        if (compactedResult !== result) await recorder?.record('tool_result_compacted', { step, tool: toolAction.tool, result: compactedResult })
      }
    } else {
      for (const toolAction of toolActions) {
        const preHook = await runHook({
          hookRunner,
          event: toolHookEvent(toolAction.tool),
          payload: hookPayload({ task, cwd, action: toolAction }),
          recorder,
        })
        const effectiveAction = actionWithHookInput(toolAction, preHook)
        const approval = preHook.decision === 'block'
          ? { approved: false, result: { ok: false, error: `hook blocked ${toolAction.tool}: ${preHook.reason}` } }
          : await maybeApprove({ action: effectiveAction, allowWrite, allowCommand, approveTool, recorder })
        const result = approval.approved ? await tools.execute(effectiveAction.tool, effectiveAction.input) : approval.result
        const compactedResult = await compactToolResult({ result, recorder, step, tool: effectiveAction.tool })
        results.push({ tool: effectiveAction.tool, result: compactedResult })
        await recorder?.record('tool_result', { step, tool: effectiveAction.tool, requested: toolAction, effective: effectiveAction, result })
        if (compactedResult !== result) await recorder?.record('tool_result_compacted', { step, tool: effectiveAction.tool, result: compactedResult })
        const postEvent = postToolHookEvent(effectiveAction.tool)
        if (postEvent) {
          await runHook({ hookRunner, event: postEvent, payload: hookPayload({ task, cwd, action: effectiveAction, result }), recorder })
        }
      }
    }
    messages.push({ role: 'user', content: formatToolResult(action.action === 'tools' ? results : results[0].result) })
    }

    throw new Error(`max steps exceeded: ${maxSteps}`)
  } catch (error) {
    await recorder?.record('run_error', { message: errorMessage(error) })
    throw error
  } finally {
    await closeMcpClients({ cwd })
  }
}
