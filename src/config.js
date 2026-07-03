import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { dirname, join, resolve } from 'node:path'

async function readJson(file) {
  try {
    return JSON.parse(await readFile(file, 'utf8'))
  } catch (error) {
    if (error?.code === 'ENOENT') return {}
    throw error
  }
}

function mergeConfig(base, override) {
  return { ...base, ...override }
}

export function projectJedenConfigPath({ cwd = process.cwd() } = {}) {
  return resolve(cwd, '.jeden', 'config.json')
}

export async function loadProjectJedenConfig({ cwd = process.cwd() } = {}) {
  return readJson(projectJedenConfigPath({ cwd }))
}

export async function saveProjectJedenConfig(config, { cwd = process.cwd() } = {}) {
  const file = projectJedenConfigPath({ cwd })
  await mkdir(dirname(file), { recursive: true })
  await writeFile(file, `${JSON.stringify(config || {}, null, 2)}\n`, 'utf8')
  return file
}

export async function loadJedenConfig({ cwd = process.cwd() } = {}) {
  const user = await readJson(join(homedir(), '.jeden', 'config.json'))
  const project = await readJson(resolve(cwd, '.jeden', 'config.json'))
  return mergeConfig(user, project)
}

export function applyConfigEnv(config, env = process.env) {
  if (config.model && !env.JEDEN_MODEL) env.JEDEN_MODEL = String(config.model)
  if (config.modelRouterUrl && !env.MODEL_ROUTER_URL) env.MODEL_ROUTER_URL = String(config.modelRouterUrl)
  if (config.agentId && !env.WISENT_APP_AGENT_ID) env.WISENT_APP_AGENT_ID = String(config.agentId)
  return env
}
