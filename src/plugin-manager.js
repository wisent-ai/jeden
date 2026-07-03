import { mkdir, readFile, readdir, stat, writeFile } from 'node:fs/promises'
import { basename, dirname, extname, join, resolve } from 'node:path'

import { discoverCustomToolFiles, loadCustomTools } from './custom-tools.js'

const REGISTRY_VERSION = 1
const MANIFEST_FILES = ['jeden-plugin.json', '.jeden-plugin.json', 'plugin.json', 'package.json']

function nowIso() { return new Date().toISOString() }

function emptyRegistry() {
  return { version: REGISTRY_VERSION, sources: {}, installed: {}, reload: null }
}

export function pluginRegistryPath({ cwd = process.cwd() } = {}) {
  return join(resolve(cwd), '.jeden', 'plugins.json')
}

function sanitizeName(value, fallback = 'source') {
  const text = String(value || fallback).trim().replace(/[@\\/]+/g, '-').replace(/[^a-zA-Z0-9._-]+/g, '-').replace(/^-+|-+$/g, '')
  return text || fallback
}

function sourceName(source) {
  const text = String(source || '').trim()
  if (!text) return ''
  try {
    const url = new URL(text)
    const tail = basename(url.pathname.replace(/\/+$/, ''))
    return sanitizeName(tail || url.hostname || text, 'source')
  } catch {}
  return sanitizeName(basename(text.replace(/\/+$/, '')) || text, 'source')
}

function sourceType(source) {
  const text = String(source || '')
  if (/^https?:\/\//i.test(text)) return 'url'
  if (/^(git\+)?ssh:\/\//i.test(text) || /^git@/i.test(text)) return 'git'
  return 'local'
}

function normalizeRegistry(raw) {
  const registry = raw && typeof raw === 'object' && !Array.isArray(raw) ? raw : emptyRegistry()
  return {
    version: REGISTRY_VERSION,
    sources: registry.sources && typeof registry.sources === 'object' && !Array.isArray(registry.sources) ? registry.sources : {},
    installed: registry.installed && typeof registry.installed === 'object' && !Array.isArray(registry.installed) ? registry.installed : {},
    reload: registry.reload && typeof registry.reload === 'object' && !Array.isArray(registry.reload) ? registry.reload : null,
  }
}

export async function loadPluginRegistry({ cwd = process.cwd() } = {}) {
  const file = pluginRegistryPath({ cwd })
  try {
    return normalizeRegistry(JSON.parse(await readFile(file, 'utf8')))
  } catch (error) {
    if (error?.code === 'ENOENT') return emptyRegistry()
    throw error
  }
}

export async function savePluginRegistry(registry, { cwd = process.cwd() } = {}) {
  const file = pluginRegistryPath({ cwd })
  const normalized = normalizeRegistry(registry)
  normalized.updatedAt = nowIso()
  await mkdir(dirname(file), { recursive: true })
  await writeFile(file, `${JSON.stringify(normalized, null, 2)}\n`, 'utf8')
  return file
}

function parsePluginTarget(target) {
  const text = String(target || '').trim()
  if (!text) return { name: '', marketplace: '' }
  const at = text.lastIndexOf('@')
  if (at > 0 && at < text.length - 1) return { name: text.slice(0, at), marketplace: text.slice(at + 1) }
  return { name: text, marketplace: '' }
}

function pluginId(name, marketplace) {
  return marketplace ? `${name}@${marketplace}` : name
}

function formatSource(source) {
  return `${source.name}\t${source.type}\t${source.source}\t${source.enabled === false ? 'disabled' : 'enabled'}`
}

function formatPlugin(plugin) {
  return `${plugin.id}\t${plugin.version || '-'}\t${plugin.enabled === false ? 'disabled' : 'enabled'}\t${plugin.source || '-'}`
}

async function readJsonIfExists(file) {
  try {
    return JSON.parse(await readFile(file, 'utf8'))
  } catch (error) {
    if (error?.code === 'ENOENT') return null
    if (error instanceof SyntaxError) return null
    throw error
  }
}

async function pathStatus(path) {
  try {
    return await stat(path)
  } catch (error) {
    if (error?.code === 'ENOENT' || error?.code === 'ENOTDIR') return null
    throw error
  }
}

async function localManifest(root) {
  for (const name of MANIFEST_FILES) {
    const manifest = await readJsonIfExists(join(root, name))
    if (!manifest) continue
    return { file: join(root, name), manifest }
  }
  return null
}

async function listModuleFiles(root) {
  try {
    const entries = await readdir(root, { withFileTypes: true })
    return entries
      .filter((entry) => entry.isFile() && (entry.name.endsWith('.js') || entry.name.endsWith('.mjs')))
      .map((entry) => join(root, entry.name))
      .sort()
  } catch (error) {
    if (error?.code === 'ENOENT' || error?.code === 'ENOTDIR') return []
    throw error
  }
}

function pluginFromManifest(source, manifestInfo) {
  const manifest = manifestInfo?.manifest || {}
  const name = sanitizeName(manifest.name || manifest.displayName || source.name, source.name)
  return {
    id: pluginId(name, source.name),
    name,
    marketplace: source.name,
    version: String(manifest.version || '0.0.0'),
    description: String(manifest.description || ''),
    source: source.source,
    manifestPath: manifestInfo?.file || null,
  }
}

function pluginFromFile(source, file) {
  const name = sanitizeName(basename(file, extname(file)), source.name)
  return {
    id: pluginId(name, source.name),
    name,
    marketplace: source.name,
    version: '0.0.0',
    description: 'Local custom tool module',
    source: file,
    manifestPath: null,
  }
}

export async function discoverMarketplacePlugins(source, { cwd = process.cwd() } = {}) {
  if (!source) return []
  if (source.type !== 'local') return [pluginFromManifest(source, { manifest: { name: source.name, version: source.version || '0.0.0', description: `Remote source metadata for ${source.source}` }, file: null })]

  const root = resolve(cwd, source.source)
  const status = await pathStatus(root)
  if (!status) return [pluginFromManifest(source, { manifest: { name: source.name, version: source.version || '0.0.0', description: `Configured local source not found yet: ${source.source}` }, file: null })]
  if (status.isFile()) return [pluginFromFile(source, root)]

  const manifest = await localManifest(root)
  const plugins = []
  if (manifest) plugins.push(pluginFromManifest(source, manifest))
  for (const dir of [root, join(root, 'tools')]) {
    for (const file of await listModuleFiles(dir)) plugins.push(pluginFromFile(source, file))
  }
  if (plugins.length === 0) plugins.push(pluginFromManifest(source, { manifest: { name: source.name, version: source.version || '0.0.0', description: `Local plugin source ${source.source}` }, file: null }))
  const byId = new Map()
  for (const plugin of plugins) byId.set(plugin.id, plugin)
  return [...byId.values()].sort((a, b) => a.id.localeCompare(b.id))
}

async function discoverSources(registry, names, options) {
  const sources = Object.values(registry.sources).filter((source) => names.length === 0 || names.includes(source.name))
  const discovered = []
  for (const source of sources) {
    const plugins = await discoverMarketplacePlugins(source, options)
    source.discoveredAt = nowIso()
    source.plugins = plugins.map(({ execute, ...plugin }) => plugin)
    discovered.push(...plugins)
  }
  return discovered
}

async function upsertInstalled(registry, plugin, status = {}) {
  const id = plugin.id
  const previous = registry.installed[id] || {}
  registry.installed[id] = {
    ...previous,
    ...plugin,
    id,
    enabled: status.enabled ?? previous.enabled ?? true,
    installedAt: previous.installedAt || nowIso(),
    updatedAt: nowIso(),
  }
  return registry.installed[id]
}

export async function handleMarketplaceCommand(args, { cwd = process.cwd() } = {}) {
  const [verb = 'help', first, ...rest] = args
  const registry = await loadPluginRegistry({ cwd })

  if (verb === 'help') {
    return { text: 'Usage: /marketplace add <source> | remove <name> | list | discover [marketplace] | install <name@marketplace> | installed | uninstall <name@marketplace> | update [marketplace] | upgrade [name@marketplace]' }
  }

  if (verb === 'add') {
    const source = [first, ...rest].filter(Boolean).join(' ').trim()
    if (!source) return { error: 'Usage: /marketplace add <source>' }
    const name = sourceName(source)
    const existing = registry.sources[name]
    registry.sources[name] = {
      name,
      source,
      type: sourceType(source),
      enabled: true,
      addedAt: existing?.addedAt || nowIso(),
      updatedAt: nowIso(),
      plugins: existing?.plugins || [],
    }
    const file = await savePluginRegistry(registry, { cwd })
    return { text: `Added marketplace source ${name} (${source}) in ${file}.` }
  }

  if (verb === 'remove') {
    if (!first) return { error: 'Usage: /marketplace remove <name>' }
    if (!registry.sources[first]) return { error: `Marketplace source not found: ${first}` }
    delete registry.sources[first]
    const file = await savePluginRegistry(registry, { cwd })
    return { text: `Removed marketplace source ${first} from ${file}. Installed plugin records were kept; uninstall them explicitly if desired.` }
  }

  if (verb === 'list') {
    const sources = Object.values(registry.sources).sort((a, b) => a.name.localeCompare(b.name))
    return { text: sources.length ? ['Marketplace sources:', ...sources.map(formatSource)].join('\n') : 'No marketplace sources configured. Add one with /marketplace add <source>.' }
  }

  if (verb === 'discover' || verb === 'update') {
    const names = first ? [first] : []
    const missing = names.filter((name) => !registry.sources[name])
    if (missing.length) return { error: `Marketplace source not found: ${missing.join(', ')}` }
    const discovered = await discoverSources(registry, names, { cwd })
    const file = await savePluginRegistry(registry, { cwd })
    const label = verb === 'update' ? 'Updated marketplace metadata' : 'Discovered marketplace plugins'
    return { text: discovered.length ? [`${label} in ${file}:`, ...discovered.map(formatPlugin)].join('\n') : `${label} in ${file}: no sources configured.` }
  }

  if (verb === 'install') {
    if (!first) return { error: 'Usage: /marketplace install <name@marketplace>' }
    const target = parsePluginTarget(first)
    const discovered = await discoverSources(registry, target.marketplace ? [target.marketplace] : [], { cwd })
    const plugin = discovered.find((candidate) => candidate.name === target.name && (!target.marketplace || candidate.marketplace === target.marketplace))
    if (!plugin) return { error: `Plugin not found in configured sources: ${first}` }
    const installed = await upsertInstalled(registry, plugin, { enabled: true })
    const file = await savePluginRegistry(registry, { cwd })
    return { text: `Installed plugin ${installed.id} in ${file}.` }
  }

  if (verb === 'installed') {
    const installed = Object.values(registry.installed).sort((a, b) => a.id.localeCompare(b.id))
    return { text: installed.length ? ['Installed plugins:', ...installed.map(formatPlugin)].join('\n') : 'No plugins installed.' }
  }

  if (verb === 'uninstall') {
    if (!first) return { error: 'Usage: /marketplace uninstall <name@marketplace>' }
    const id = first
    if (!registry.installed[id]) return { error: `Installed plugin not found: ${id}` }
    delete registry.installed[id]
    const file = await savePluginRegistry(registry, { cwd })
    return { text: `Uninstalled plugin ${id} from ${file}.` }
  }

  if (verb === 'upgrade') {
    if (!first) return { error: 'Usage: /marketplace upgrade <name@marketplace>' }
    const current = registry.installed[first]
    if (!current) return { error: `Installed plugin not found: ${first}` }
    const discovered = await discoverSources(registry, current.marketplace ? [current.marketplace] : [], { cwd })
    const plugin = discovered.find((candidate) => candidate.id === first)
    if (!plugin) return { error: `Plugin not found in configured sources: ${first}` }
    const installed = await upsertInstalled(registry, plugin, { enabled: current.enabled !== false })
    const file = await savePluginRegistry(registry, { cwd })
    return { text: `Upgraded plugin ${installed.id} metadata in ${file}.` }
  }

  return { error: 'Usage: /marketplace add <source> | remove <name> | list | discover [marketplace] | install <name@marketplace> | installed | uninstall <name@marketplace> | update [marketplace] | upgrade <name@marketplace> | help' }
}

export async function handlePluginsCommand(args, { cwd = process.cwd() } = {}) {
  const [verb = 'list', target] = args
  const registry = await loadPluginRegistry({ cwd })
  if (verb === 'list') {
    const installed = Object.values(registry.installed).sort((a, b) => a.id.localeCompare(b.id))
    return { text: installed.length ? ['Installed plugins:', ...installed.map(formatPlugin)].join('\n') : 'No plugins installed. Use /marketplace discover and /marketplace install <name@marketplace>.' }
  }
  if (verb === 'enable' || verb === 'disable') {
    if (!target) return { error: `Usage: /plugins ${verb} <name@marketplace>` }
    const plugin = registry.installed[target]
    if (!plugin) return { error: `Installed plugin not found: ${target}` }
    plugin.enabled = verb === 'enable'
    plugin.updatedAt = nowIso()
    const file = await savePluginRegistry(registry, { cwd })
    return { text: `${verb === 'enable' ? 'Enabled' : 'Disabled'} plugin ${target} in ${file}.` }
  }
  return { error: 'Usage: /plugins list | enable <name@marketplace> | disable <name@marketplace>' }
}

export async function handleExtensionsCommand({ cwd = process.cwd() } = {}) {
  const registry = await loadPluginRegistry({ cwd })
  const sources = Object.values(registry.sources).sort((a, b) => a.name.localeCompare(b.name))
  const installed = Object.values(registry.installed).sort((a, b) => a.id.localeCompare(b.id))
  return {
    text: [
      `Extension registry: ${pluginRegistryPath({ cwd })}`,
      `Sources: ${sources.length}`,
      ...(sources.length ? sources.map((source) => `- ${formatSource(source)}`) : ['- none']),
      `Installed plugins: ${installed.length}`,
      ...(installed.length ? installed.map((plugin) => `- ${formatPlugin(plugin)}`) : ['- none']),
    ].join('\n'),
  }
}

export async function reloadPlugins({ cwd = process.cwd(), builtInToolNames = [], allowCommand = false } = {}) {
  const registry = await loadPluginRegistry({ cwd })
  const files = await discoverCustomToolFiles({ cwd })
  const loaded = await loadCustomTools({ cwd, builtInToolNames, allowCommand })
  registry.reload = {
    requestedAt: nowIso(),
    customToolFiles: files,
    loadedTools: loaded.tools.map((tool) => ({ name: tool.name, source: tool.source })),
    errors: loaded.errors,
    status: 'loaded-for-verification',
  }
  const file = await savePluginRegistry(registry, { cwd })
  return {
    text: [
      `Plugin reload scanned ${files.length} custom tool file(s).`,
      `Loaded ${loaded.tools.length} custom tool definition(s) for verification.`,
      loaded.errors.length ? `Errors: ${loaded.errors.map((error) => `${error.path}: ${error.error}`).join('; ')}` : 'Errors: none.',
      `Reload marker: ${file}`,
      'The active tool registry is rebuilt on the next Jeden run; this command verifies local files and records the reload request durably.',
    ].join('\n'),
  }
}
