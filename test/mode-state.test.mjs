import test from 'node:test'
import assert from 'node:assert/strict'

import {
  consumeLoopPrompt,
  createModeState,
  dispatchModeSlashCommand,
  modelConfigForModes,
  noteModeRunResult,
  prepareTaskForModes,
  storeAdvisorModelReview,
} from '../src/mode-state.js'

function fakeContext() {
  const tools = [{ name: 'read_file' }, { name: 'edit' }]
  return {
    args: { cwd: '/tmp/work', model: 'default', allowWrite: false, allowCommand: false },
    createToolRegistry() { return { list: () => tools } },
  }
}

function parsed(name, args = '') {
  return { name, canonical: name, args }
}

test('plan mode mutates state and annotates subsequent prompts', () => {
  const state = createModeState()
  const enabled = dispatchModeSlashCommand(parsed('plan', 'implement a parser'), state, fakeContext())

  assert.equal(enabled.handled, true)
  assert.equal(enabled.runTask, 'implement a parser')
  assert.equal(state.plan.enabled, true)

  const prepared = prepareTaskForModes('implement a parser', state)
  assert.match(prepared, /Plan mode is enabled/)
  assert.match(prepared, /User prompt:\nimplement a parser/)

  noteModeRunResult(state, { task: 'implement a parser', text: 'Plan:\n1. Parse\n2. Test' })
  const review = dispatchModeSlashCommand(parsed('plan-review'), state, fakeContext())
  assert.equal(review.handled, true)
  assert.match(review.runTask, /Review the latest plan/)
  assert.match(review.runTask, /1\. Parse/)
})

test('goal and guided goal maintain session-local prompt state', () => {
  const state = createModeState()

  const goal = dispatchModeSlashCommand(parsed('goal', 'set ship slash parity'), state, fakeContext())
  assert.equal(goal.handled, true)
  assert.equal(state.goal.enabled, true)
  assert.equal(state.goal.objective, 'ship slash parity')

  dispatchModeSlashCommand(parsed('goal', 'budget 3'), state, fakeContext())
  const prepared = prepareTaskForModes('next step', state)
  assert.match(prepared, /Goal mode is active/)
  assert.match(prepared, /ship slash parity/)
  assert.match(prepared, /Budget: 3/)

  const guided = dispatchModeSlashCommand(parsed('guided-goal', 'make the migration safer'), state, fakeContext())
  assert.equal(state.guidedGoal.active, true)
  assert.match(guided.runTask, /Help refine this rough autonomous goal/)
  assert.match(prepareTaskForModes('refine', state), /Guided goal drafting is active/)
})

test('loop state returns bounded resubmissions without pretending to run background workers', () => {
  const state = createModeState()
  const started = dispatchModeSlashCommand(parsed('loop', '2 check status'), state, fakeContext())

  assert.equal(started.handled, true)
  assert.equal(started.runTask, 'check status')
  assert.equal(state.loop.enabled, true)

  assert.equal(consumeLoopPrompt(state, 'fallback'), 'check status')
  assert.equal(state.loop.remaining, 1)
  assert.equal(state.loop.enabled, true)
  assert.equal(consumeLoopPrompt(state, 'fallback'), 'check status')
  assert.equal(state.loop.remaining, 0)
  assert.equal(state.loop.enabled, false)
  assert.equal(consumeLoopPrompt(state, 'fallback'), null)
})

test('force validates tools, consumes the next-turn instruction, and retry records failures', () => {
  const state = createModeState()

  const missing = dispatchModeSlashCommand(parsed('force', 'missing_tool inspect'), state, fakeContext())
  assert.equal(missing.role, 'error')
  assert.match(missing.text, /Visible tools: read_file, edit/)

  const forced = dispatchModeSlashCommand(parsed('force', 'read_file inspect package'), state, fakeContext())
  assert.equal(forced.runTask, 'inspect package')
  assert.equal(state.force.tool, 'read_file')

  const prepared = prepareTaskForModes('inspect package', state)
  assert.match(prepared, /Forced tool request/)
  assert.match(prepared, /read_file/)
  assert.equal(state.force, null)
  assert.doesNotMatch(prepareTaskForModes('later prompt', state), /Forced tool request/)

  noteModeRunResult(state, { task: 'inspect package', error: new Error('boom') })
  const retry = dispatchModeSlashCommand(parsed('retry'), state, fakeContext())
  assert.equal(retry.runTask, 'inspect package')
  assert.equal(state.lastFailedTask, '')
})

test('advisor mode stores model reviewer notes', () => {
  const state = createModeState()

  const enabled = dispatchModeSlashCommand(parsed('advisor', 'on'), state, fakeContext())
  assert.equal(enabled.handled, true)
  assert.equal(state.advisor.enabled, true)

  const configured = dispatchModeSlashCommand(parsed('advisor', 'configure model reviewer-local'), state, fakeContext())
  assert.equal(configured.role, 'system')
  assert.equal(state.advisor.model, 'reviewer-local')

  const status = dispatchModeSlashCommand(parsed('advisor', 'status'), state, fakeContext())
  assert.match(status.text, /enabled/)
  assert.match(status.text, /reviewer-local/)
  assert.match(status.text, /second model-router call/)
  assert.match(prepareTaskForModes('ship change', state), /request a second model-router review/)

  noteModeRunResult(state, { task: 'ship change', text: 'Implemented the change.' })
  assert.equal(state.advisor.lastReview, null)
  storeAdvisorModelReview(state, { task: 'ship change', text: 'Verdict: pass', model: 'reviewer-local' })
  const dump = dispatchModeSlashCommand(parsed('advisor', 'dump'), state, fakeContext())
  assert.equal(dump.role, 'system')
  assert.match(dump.text, /Verdict: pass/)

  const raw = dispatchModeSlashCommand(parsed('advisor', 'dump raw'), state, fakeContext())
  assert.match(raw.text, /"backend": "model-router"/)
})

test('fast mode maps to model-router service tier', () => {
  const state = createModeState()

  const fast = dispatchModeSlashCommand(parsed('fast', 'tier flex'), state, fakeContext())
  assert.equal(fast.role, 'system')
  assert.equal(state.fast.enabled, true)
  assert.equal(state.fast.serviceTier, 'flex')
  assert.equal(modelConfigForModes(state, { model: 'm' }).serviceTier, 'flex')
  assert.match(prepareTaskForModes('run', state), /service_tier=flex/)
})
