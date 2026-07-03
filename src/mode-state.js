const COUNT_RE = /^(\d+)$/
const DURATION_RE = /^(\d+)(ms|s|m|h)$/i

const MODE_COMMANDS = new Set([
  'plan',
  'plan-review',
  'goal',
  'guided-goal',
  'loop',
  'fast',
  'advisor',
  'switch',
  'btw',
  'tan',
  'omfg',
  'retry',
  'force',
])

export function createModeState(overrides = {}) {
  return normalizeModeState(overrides)
}

export function normalizeModeState(state = {}) {
  return {
    plan: {
      enabled: Boolean(state.plan?.enabled),
      latestPlan: typeof state.plan?.latestPlan === 'string' ? state.plan.latestPlan : '',
    },
    goal: {
      enabled: Boolean(state.goal?.enabled),
      paused: Boolean(state.goal?.paused),
      objective: typeof state.goal?.objective === 'string' ? state.goal.objective : '',
      budget: state.goal?.budget ?? null,
    },
    guidedGoal: {
      active: Boolean(state.guidedGoal?.active),
      roughObjective: typeof state.guidedGoal?.roughObjective === 'string' ? state.guidedGoal.roughObjective : '',
    },
    loop: {
      enabled: Boolean(state.loop?.enabled),
      remaining: Number.isInteger(state.loop?.remaining) ? state.loop.remaining : null,
      until: Number.isFinite(state.loop?.until) ? state.loop.until : null,
      prompt: typeof state.loop?.prompt === 'string' ? state.loop.prompt : '',
    },
    fast: {
      enabled: Boolean(state.fast?.enabled),
      serviceTier: typeof state.fast?.serviceTier === 'string' ? state.fast.serviceTier : 'priority',
    },
    advisor: {
      enabled: Boolean(state.advisor?.enabled),
      model: typeof state.advisor?.model === 'string' ? state.advisor.model : '',
      lastReview: normalizeAdvisorReview(state.advisor?.lastReview),
    },
    force: state.force?.tool
      ? { tool: String(state.force.tool), prompt: typeof state.force.prompt === 'string' ? state.force.prompt : '' }
      : null,
    lastFailedTask: typeof state.lastFailedTask === 'string' ? state.lastFailedTask : '',
    lastTask: typeof state.lastTask === 'string' ? state.lastTask : '',
  }
}

function normalizeAdvisorReview(review) {
  if (!review || typeof review !== 'object') return null
  const findings = Array.isArray(review.findings) ? review.findings.map(String) : []
  return {
    backend: typeof review.backend === 'string' ? review.backend : 'local-deterministic',
    model: typeof review.model === 'string' ? review.model : '',
    task: typeof review.task === 'string' ? review.task : '',
    resultPreview: typeof review.resultPreview === 'string' ? review.resultPreview : '',
    findings,
    text: typeof review.text === 'string' ? review.text : formatAdvisorReviewText({ ...review, findings }),
  }
}

function ensureAdvisorState(state) {
  state.advisor = {
    enabled: Boolean(state.advisor?.enabled),
    model: typeof state.advisor?.model === 'string' ? state.advisor.model : '',
    lastReview: normalizeAdvisorReview(state.advisor?.lastReview),
  }
  return state.advisor
}

export function isModeCommand(name) {
  return MODE_COMMANDS.has(String(name || '').toLowerCase())
}

function splitArgs(args) {
  const text = String(args || '').trim()
  if (!text) return { head: '', rest: '' }
  const match = /^(\S+)(?:\s+([\s\S]*))?$/.exec(text)
  return { head: match?.[1] || '', rest: match?.[2] || '' }
}

function parseDurationMs(value) {
  const match = DURATION_RE.exec(String(value || '').trim())
  if (!match) return null
  const amount = Number(match[1])
  const unit = match[2].toLowerCase()
  if (unit === 'ms') return amount
  if (unit === 's') return amount * 1000
  if (unit === 'm') return amount * 60_000
  if (unit === 'h') return amount * 3_600_000
  return null
}

function formatGoalStatus(goal) {
  if (!goal.objective) return 'Goal mode has no objective. Use /goal set <objective>.'
  const state = goal.enabled ? (goal.paused ? 'paused' : 'active') : 'disabled'
  const budget = goal.budget == null ? 'off' : String(goal.budget)
  return [`Goal mode: ${state}`, `Objective: ${goal.objective}`, `Budget: ${budget}`].join('\n')
}

function availableToolNames(context) {
  if (Array.isArray(context.availableTools)) return context.availableTools.map((tool) => typeof tool === 'string' ? tool : tool?.name).filter(Boolean)
  const registry = context.createToolRegistry?.({
    cwd: context.args?.cwd,
    allowWrite: context.args?.allowWrite,
    allowCommand: context.args?.allowCommand,
    artifactDir: context.recorder?.artifactDir?.(),
  })
  return registry?.list?.().map((tool) => tool.name) || []
}

export function dispatchModeSlashCommand(parsed, state = createModeState(), context = {}) {
  const canonical = String(parsed?.canonical || parsed?.name || '').toLowerCase()
  if (!isModeCommand(canonical)) return null

  const args = String(parsed?.args || '').trim()
  if (canonical === 'switch') return dispatchSwitch(args, context)
  if (canonical === 'plan') return dispatchPlan(args, state)
  if (canonical === 'plan-review') return dispatchPlanReview(state)
  if (canonical === 'goal') return dispatchGoal(args, state)
  if (canonical === 'guided-goal') return dispatchGuidedGoal(args, state)
  if (canonical === 'loop') return dispatchLoop(args, state)
  if (canonical === 'fast') return dispatchFast(args, state)
  if (canonical === 'advisor') return dispatchAdvisor(args, state, context)
  if (canonical === 'btw') return dispatchBtw(args)
  if (canonical === 'tan' || canonical === 'omfg') return null
  if (canonical === 'retry') return dispatchRetry(state)
  if (canonical === 'force') return dispatchForce(args, state, context)
  return null
}

function dispatchSwitch(args, context) {
  const nextModel = args.trim()
  if (nextModel) {
    if (context.args) context.args.model = nextModel
    context.setModel?.(nextModel)
    return { handled: true, role: 'system', text: `Model route set to ${nextModel}.` }
  }
  return { handled: true, role: 'system', text: `Current model route: ${context.args?.model || process.env.JEDEN_MODEL || process.env.MODEL || 'default'}.` }
}

function dispatchPlan(args, state) {
  const { head, rest } = splitArgs(args)
  const verb = head.toLowerCase()
  if (!args) {
    state.plan.enabled = !state.plan.enabled
    return { handled: true, role: 'system', text: `Plan mode ${state.plan.enabled ? 'enabled' : 'disabled'}.` }
  }
  if (verb === 'on') {
    state.plan.enabled = true
    return { handled: true, role: 'system', text: 'Plan mode enabled.' }
  }
  if (verb === 'off') {
    state.plan.enabled = false
    return { handled: true, role: 'system', text: 'Plan mode disabled.' }
  }
  if (verb === 'status') {
    return { handled: true, role: 'system', text: `Plan mode is ${state.plan.enabled ? 'enabled' : 'disabled'}.${state.plan.latestPlan ? '\nLatest plan is available for /plan-review.' : ''}` }
  }
  const prompt = rest && verb === 'run' ? rest : args
  state.plan.enabled = true
  return { handled: true, role: 'system', text: 'Plan mode enabled for this prompt.', runTask: prompt }
}

function dispatchPlanReview(state) {
  if (!state.plan.latestPlan) {
    return { handled: true, role: 'error', text: 'No plan is available to review yet. Run a prompt while /plan is enabled, then use /plan-review.' }
  }
  return {
    handled: true,
    role: 'system',
    text: 'Reopening the latest plan for review.',
    runTask: [
      'Review the latest plan from this session before any implementation.',
      'Identify missing steps, risks, and concrete changes needed. Do not execute the plan unless the user explicitly asks after the review.',
      '',
      state.plan.latestPlan,
    ].join('\n'),
  }
}

function dispatchGoal(args, state) {
  const { head, rest } = splitArgs(args)
  const verb = head.toLowerCase()
  if (!args || verb === 'show' || verb === 'status') return { handled: true, role: 'system', text: formatGoalStatus(state.goal) }
  if (verb === 'set') {
    if (!rest.trim()) return { handled: true, role: 'error', text: 'Usage: /goal set <objective>' }
    state.goal.objective = rest.trim()
    state.goal.enabled = true
    state.goal.paused = false
    return { handled: true, role: 'system', text: `Goal mode enabled.\nObjective: ${state.goal.objective}` }
  }
  if (verb === 'pause') {
    state.goal.paused = true
    return { handled: true, role: 'system', text: 'Goal mode paused.' }
  }
  if (verb === 'resume') {
    if (!state.goal.objective) return { handled: true, role: 'error', text: 'No goal objective is set. Use /goal set <objective>.' }
    state.goal.enabled = true
    state.goal.paused = false
    return { handled: true, role: 'system', text: 'Goal mode resumed.' }
  }
  if (verb === 'drop' || verb === 'off') {
    state.goal.enabled = false
    state.goal.paused = false
    state.goal.objective = ''
    state.goal.budget = null
    return { handled: true, role: 'system', text: 'Goal mode dropped.' }
  }
  if (verb === 'budget') {
    const budget = rest.trim().toLowerCase()
    if (!budget || budget === 'off') {
      state.goal.budget = null
      return { handled: true, role: 'system', text: 'Goal budget disabled.' }
    }
    const parsed = Number(budget)
    if (!Number.isFinite(parsed) || parsed <= 0) return { handled: true, role: 'error', text: 'Usage: /goal budget <positive-number|off>' }
    state.goal.budget = parsed
    return { handled: true, role: 'system', text: `Goal budget set to ${parsed}.` }
  }
  state.goal.objective = args
  state.goal.enabled = true
  state.goal.paused = false
  return { handled: true, role: 'system', text: `Goal mode enabled.\nObjective: ${state.goal.objective}` }
}

function dispatchGuidedGoal(args, state) {
  if (!args) return { handled: true, role: 'error', text: 'Usage: /guided-goal <rough objective>' }
  state.guidedGoal.active = true
  state.guidedGoal.roughObjective = args
  return {
    handled: true,
    role: 'system',
    text: 'Guided goal drafting started. Jeden will use the next turn to refine the objective instead of pretending to open an overlay.',
    runTask: [
      'Help refine this rough autonomous goal before enabling goal mode.',
      'Ask concise clarifying questions if required; otherwise propose a precise objective and acceptance criteria.',
      `Rough objective: ${args}`,
    ].join('\n'),
  }
}

function dispatchLoop(args, state) {
  const { head, rest } = splitArgs(args)
  const verb = head.toLowerCase()
  if (verb === 'off' || verb === 'stop') {
    state.loop.enabled = false
    state.loop.remaining = null
    state.loop.until = null
    state.loop.prompt = ''
    return { handled: true, role: 'system', text: 'Loop mode disabled.' }
  }
  if (verb === 'status') return { handled: true, role: 'system', text: loopStatus(state.loop) }

  let prompt = args
  state.loop.remaining = null
  state.loop.until = null
  if (COUNT_RE.test(head)) {
    state.loop.remaining = Number(head)
    prompt = rest.trim()
  } else {
    const durationMs = parseDurationMs(head)
    if (durationMs != null) {
      state.loop.until = Date.now() + durationMs
      prompt = rest.trim()
    }
  }
  state.loop.enabled = true
  state.loop.prompt = prompt
  const qualifier = state.loop.remaining != null ? ` for ${state.loop.remaining} resubmission(s)` : state.loop.until != null ? ' until the duration expires' : ''
  return { handled: true, role: 'system', text: `Loop mode enabled${qualifier}.`, ...(prompt ? { runTask: prompt } : {}) }
}

function loopStatus(loop) {
  if (!loop.enabled) return 'Loop mode is disabled.'
  const limits = []
  if (loop.remaining != null) limits.push(`${loop.remaining} resubmission(s) remaining`)
  if (loop.until != null) limits.push(`until ${new Date(loop.until).toISOString()}`)
  if (loop.prompt) limits.push(`prompt: ${loop.prompt}`)
  return `Loop mode is enabled${limits.length ? ` (${limits.join(', ')})` : ''}.`
}

function dispatchFast(args, state) {
  const { head, rest } = splitArgs(args)
  const verb = head.toLowerCase()
  if (!verb) state.fast.enabled = !state.fast.enabled
  else if (verb === 'on') state.fast.enabled = true
  else if (verb === 'off') state.fast.enabled = false
  else if (verb === 'tier') {
    const tier = rest.trim()
    if (!tier) return { handled: true, role: 'error', text: 'Usage: /fast tier <service-tier>' }
    state.fast.serviceTier = tier
    state.fast.enabled = true
  } else if (verb !== 'status') return { handled: true, role: 'error', text: 'Usage: /fast [on|off|status|tier <service-tier>]' }
  const tier = state.fast.serviceTier || 'priority'
  return { handled: true, role: 'system', text: `Fast mode is ${state.fast.enabled ? 'enabled' : 'disabled'}. Model-router service_tier for future requests: ${state.fast.enabled ? tier : '(default)'}.` }
}

function currentModelRoute(context) {
  return context.args?.model || process.env.JEDEN_MODEL || process.env.MODEL || 'default'
}

function advisorModelLabel(advisor, context) {
  return advisor.model || currentModelRoute(context)
}

function formatAdvisorReviewText(review) {
  return String(review?.text || '').trim() || 'Advisor review is empty.'
}

function formatAdvisorStatus(advisor, context) {
  return [
    `Advisor reviewer is ${advisor.enabled ? 'enabled' : 'disabled'}.`,
    'Review backend: second model-router call after each successful agent result.',
    `Configured reviewer route: ${advisorModelLabel(advisor, context)}.`,
    advisor.lastReview ? 'Last advisor notes are available with /advisor dump.' : 'No advisor notes have been recorded yet.',
  ].join('\n')
}

function dispatchAdvisor(args, state, context = {}) {
  const advisor = ensureAdvisorState(state)
  const { head, rest } = splitArgs(args)
  const verb = head.toLowerCase() || 'status'
  if (verb === 'on') {
    advisor.enabled = true
    return { handled: true, role: 'system', text: `Advisor reviewer enabled.\n${formatAdvisorStatus(advisor, context)}` }
  }
  if (verb === 'off') {
    advisor.enabled = false
    return { handled: true, role: 'system', text: 'Advisor reviewer disabled.' }
  }
  if (verb === 'status') return { handled: true, role: 'system', text: formatAdvisorStatus(advisor, context) }
  if (verb === 'dump') {
    if (!advisor.lastReview) return { handled: true, role: 'error', text: 'No advisor notes are available yet. Enable /advisor and complete an agent turn first.' }
    if (rest.trim().toLowerCase() === 'raw') return { handled: true, role: 'system', text: JSON.stringify(advisor.lastReview, null, 2) }
    return { handled: true, role: 'system', text: advisor.lastReview.text }
  }
  if (verb === 'configure') {
    const configText = rest.trim()
    if (!configText) return { handled: true, role: 'system', text: formatAdvisorStatus(advisor, context) }
    const { head: key, rest: valueRest } = splitArgs(configText)
    const lowerKey = key.toLowerCase()
    let model = configText
    if (lowerKey === 'model') model = valueRest.trim()
    else {
      const eq = key.indexOf('=')
      if (eq > 0 && key.slice(0, eq).toLowerCase() === 'model') model = key.slice(eq + 1)
    }
    if (!model) return { handled: true, role: 'error', text: 'Usage: /advisor configure [model <route>|model=<route>|<route>]' }
    advisor.model = model
    return { handled: true, role: 'system', text: `Advisor reviewer route set to ${advisor.model}.\n${formatAdvisorStatus(advisor, context)}` }
  }
  return { handled: true, role: 'error', text: 'Usage: /advisor [on|off|status|dump [raw]|configure [model <route>|model=<route>|<route>]]' }
}

function dispatchBtw(args) {
  if (!args) return { handled: true, role: 'error', text: 'Usage: /btw <side question>' }
  return {
    handled: true,
    role: 'system',
    text: 'Running an ephemeral side question against the current session context.',
    runTask: [
      'Answer this side question using the current session context.',
      'Keep it separate from the main task: do not change files unless the side question explicitly asks for file changes.',
      `Question: ${args}`,
    ].join('\n'),
  }
}

function dispatchRetry(state) {
  if (!state.lastFailedTask) return { handled: true, role: 'error', text: 'There is no failed agent turn to retry in this interactive session.' }
  const task = state.lastFailedTask
  state.lastFailedTask = ''
  return { handled: true, role: 'system', text: 'Retrying the last failed agent turn.', runTask: task }
}

function dispatchForce(args, state, context) {
  const { head, rest } = splitArgs(args)
  if (!head) return { handled: true, role: 'error', text: 'Usage: /force <tool-name> [prompt]' }
  const names = availableToolNames(context)
  if (names.length > 0 && !names.includes(head)) {
    return { handled: true, role: 'error', text: `Unknown or unavailable tool: ${head}. Visible tools: ${names.slice(0, 20).join(', ')}${names.length > 20 ? ', …' : ''}` }
  }
  state.force = { tool: head, prompt: rest.trim() }
  return { handled: true, role: 'system', text: `The next agent turn will be instructed to use ${head} first.`, ...(rest.trim() ? { runTask: rest.trim() } : {}) }
}


export function prepareTaskForModes(task, state = createModeState()) {
  const normalized = normalizeModeState(state)
  const parts = []
  if (normalized.fast.enabled) parts.push(`Fast mode is enabled: model-router requests use service_tier=${normalized.fast.serviceTier || 'priority'}; prefer the fastest correct path and keep responses concise without skipping required verification.`)
  if (normalized.plan.enabled) parts.push('Plan mode is enabled: start by presenting a concrete plan and wait for approval before making irreversible changes unless the user explicitly asked for immediate execution.')
  if (normalized.goal.enabled && !normalized.goal.paused && normalized.goal.objective) {
    const budget = normalized.goal.budget == null ? '' : ` Budget: ${normalized.goal.budget}.`
    parts.push(`Goal mode is active for this session. Persistent objective: ${normalized.goal.objective}.${budget} Keep this objective in view across turns and report blockers explicitly.`)
  }
  if (normalized.guidedGoal.active) parts.push(`Guided goal drafting is active. Rough objective: ${normalized.guidedGoal.roughObjective}. Refine it into a precise goal before enabling autonomous work.`)
  if (normalized.advisor.enabled) {
    const route = normalized.advisor.model || 'current Jeden model route'
    parts.push(`Advisor reviewer is enabled. After this agent result, Jeden will request a second model-router review using reviewer route: ${route}.`)
  }
  if (state.force?.tool) {
    parts.push(`Forced tool request for this turn: use tool "${state.force.tool}" first if it is applicable and available. If it is unsafe or inapplicable, explain why before using another tool.`)
    state.force = null
  }
  state.lastTask = task
  if (parts.length === 0) return task
  return [`Session mode instructions:`, ...parts.map((part) => `- ${part}`), '', 'User prompt:', task].join('\n')
}

export function modelConfigForModes(state = createModeState(), baseConfig = {}) {
  const normalized = normalizeModeState(state)
  if (!normalized.fast.enabled) return baseConfig
  return { ...baseConfig, serviceTier: normalized.fast.serviceTier || 'priority' }
}

export function advisorReviewConfigForModes(state = createModeState(), baseConfig = {}) {
  const advisor = ensureAdvisorState(state)
  if (!advisor.enabled) return null
  return { ...baseConfig, ...(advisor.model ? { model: advisor.model } : {}) }
}

export function advisorReviewPrompt({ task = '', result = '' } = {}) {
  return [
    'Review the preceding Jeden agent result as an advisor reviewer.',
    'Check correctness, completeness against the user request, missed verification, hidden blockers, and unsafe claims.',
    'Do not edit files or call tools unless the review itself requires reading explicitly supplied context.',
    'Return concise reviewer notes with: Verdict, Risks, Required fixes.',
    '',
    'Original user task:',
    String(task || ''),
    '',
    'Agent result to review:',
    String(result || ''),
  ].join('\n')
}

export function storeAdvisorModelReview(state = createModeState(), { task = '', text = '', model = '' } = {}) {
  const advisor = ensureAdvisorState(state)
  advisor.lastReview = {
    backend: 'model-router',
    model: model || advisor.model || '',
    task: String(task || '').trim().split(/\r?\n/, 1)[0] || '',
    resultPreview: '',
    findings: [],
    text: String(text || '').trim(),
  }
  return advisor.lastReview
}

export function noteModeRunResult(state = createModeState(), { task = '', text = '', error = null } = {}) {
  if (error) {
    if (task) state.lastFailedTask = task
    return state
  }
  if (task) state.lastTask = task
  state.lastFailedTask = ''
  if (state.plan?.enabled && typeof text === 'string' && text.trim()) state.plan.latestPlan = text.trim()
  return state
}

export function consumeLoopPrompt(state = createModeState(), fallbackTask = '') {
  const loop = state.loop
  if (!loop?.enabled) return null
  if (loop.until != null && Date.now() > loop.until) {
    loop.enabled = false
    loop.until = null
    return null
  }
  if (loop.remaining != null) {
    if (loop.remaining <= 0) {
      loop.enabled = false
      return null
    }
    loop.remaining -= 1
    if (loop.remaining === 0) loop.enabled = false
  }
  return loop.prompt || fallbackTask || state.lastTask || null
}
