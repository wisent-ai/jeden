import { dirname, isAbsolute, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

export function defaultJedenPackageRoot() {
  return resolve(dirname(fileURLToPath(import.meta.url)), '..')
}

export function isInsidePath(child, parent) {
  const resolvedChild = resolve(child)
  const resolvedParent = resolve(parent)
  const pathFromParent = relative(resolvedParent, resolvedChild)
  if (!pathFromParent) return true
  const [firstPart] = pathFromParent.split(/[\\/]/)
  return firstPart !== '..' && !isAbsolute(pathFromParent)
}

export function selfRepairPermissions({ cwd = process.cwd(), allowWrite = false, allowCommand = false, allowOwnCode = false, packageRoot = defaultJedenPackageRoot() } = {}) {
  const ownCode = isInsidePath(cwd, packageRoot)
  return {
    allowWrite: Boolean(allowWrite && (!ownCode || allowOwnCode)),
    allowCommand: Boolean(allowCommand && (!ownCode || allowOwnCode)),
    ownCodeProtected: ownCode && !allowOwnCode,
    packageRoot,
  }
}

export function errorMessage(error) {
  return error instanceof Error ? error.message : String(error)
}

export function buildSelfRepairTask({ task, cwd = process.cwd(), error, sessionPath = null, apply = false, ownCodeProtected = false } = {}) {
  const mode = apply ? 'apply a minimal repair if the cause is inside the current project' : 'produce a concrete repair plan only; do not edit files'
  const ownCodeRule = ownCodeProtected
    ? '\n- The current cwd is inside the Jeden runtime package. Do not edit Jeden runtime/source files in this self-repair attempt; explain the needed patch instead.'
    : ''
  const sessionRule = sessionPath ? `\n- Inspect the failed session transcript at ${sessionPath}/transcript.jsonl before deciding.` : ''
  return `A previous Jeden run failed. Self-repair mode is enabled.\n\nOriginal task:\n${task}\n\nFailure:\n${errorMessage(error)}\n\nCurrent project cwd:\n${cwd}\n\nRepair contract:\n- First identify the root cause from current evidence, not guesses.${sessionRule}\n- ${mode}.\n- Keep the repair minimal and scoped to the original failure.\n- Do not mask the error, weaken tests, delete unrelated code, or introduce placeholders.${ownCodeRule}\n- If edits are made, run the narrowest verification that proves the original failure path is fixed.\n- Final answer must state: root cause, files changed or proposed, verification run, and any blocker.`
}
