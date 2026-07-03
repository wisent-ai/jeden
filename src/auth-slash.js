import { spawn } from 'node:child_process'
import { createServer } from 'node:http'
import { resolve } from 'node:path'

import { loadProjectAuthConfig, projectJedenAuthPath, saveProjectAuthConfig } from './config.js'

function ok(text) { return { handled: true, role: 'system', text } }
function err(text) { return { handled: true, role: 'error', text } }
function nowIso() { return new Date().toISOString() }
function lines(values) { return values.filter((value) => value !== null && value !== undefined && value !== '').join('\n') }

function splitArgs(value) {
  const args = []
  let current = ''
  let quote = null
  for (const char of String(value || '')) {
    if (quote) {
      if (char === quote) quote = null
      else current += char
      continue
    }
    if (char === '"' || char === "'") { quote = char; continue }
    if (/\s/.test(char)) {
      if (current) { args.push(current); current = '' }
      continue
    }
    current += char
  }
  if (current) args.push(current)
  return args
}

function providerName(value) {
  const name = String(value || '').trim().toLowerCase()
  if (!name || !/^[a-z0-9][a-z0-9._-]*$/.test(name)) return ''
  return name
}

function looksLikeOauthRedirect(value) {
  try {
    const url = new URL(String(value || ''))
    return url.protocol === 'http:' || url.protocol === 'https:'
  } catch {
    return false
  }
}

function parseValue(value) {
  if (value === 'true') return true
  if (value === 'false') return false
  if (value === 'null') return null
  if (/^-?\d+(?:\.\d+)?$/.test(value)) return Number(value)
  if ((value.startsWith('{') && value.endsWith('}')) || (value.startsWith('[') && value.endsWith(']'))) {
    try { return JSON.parse(value) } catch {}
  }
  return value
}

function setPath(target, path, value) {
  let cursor = target
  for (let index = 0; index < path.length - 1; index += 1) {
    const part = path[index]
    if (!cursor[part] || typeof cursor[part] !== 'object' || Array.isArray(cursor[part])) cursor[part] = {}
    cursor = cursor[part]
  }
  cursor[path[path.length - 1]] = value
}

function parseProviderAssignments(parts) {
  const credentials = {}
  const profile = {}
  const unknown = []
  for (const part of parts) {
    const index = part.indexOf('=')
    if (index <= 0) { unknown.push(part); continue }
    const rawKey = part.slice(0, index).trim()
    const value = parseValue(part.slice(index + 1))
    const keyParts = rawKey.split('.').filter(Boolean)
    if (keyParts.length === 0) { unknown.push(part); continue }
    const namespace = keyParts[0].toLowerCase()
    if (namespace === 'profile') {
      if (keyParts.length === 1) { unknown.push(part); continue }
      setPath(profile, keyParts.slice(1), value)
    } else if (namespace === 'credential' || namespace === 'credentials' || namespace === 'auth') {
      if (keyParts.length === 1) { unknown.push(part); continue }
      setPath(credentials, keyParts.slice(1), value)
    } else {
      setPath(credentials, keyParts, value)
    }
  }
  return { credentials, profile, unknown }
}

function mergePlainObject(base, patch) {
  const out = { ...(base || {}) }
  for (const [key, value] of Object.entries(patch || {})) {
    if (value && typeof value === 'object' && !Array.isArray(value) && out[key] && typeof out[key] === 'object' && !Array.isArray(out[key])) out[key] = mergePlainObject(out[key], value)
    else out[key] = value
  }
  return out
}

function redactCredentialValue(value) {
  if (value === null || value === undefined || value === '') return value
  if (Array.isArray(value)) return value.map(redactCredentialValue)
  if (typeof value === 'object') return Object.fromEntries(Object.keys(value).map((key) => [key, redactCredentialValue(value[key])]))
  return '<redacted>'
}

function redactProviderRecord(record) {
  return {
    active: Boolean(record?.active),
    method: record?.method || 'manual',
    updatedAt: record?.updatedAt || null,
    profile: record?.profile || {},
    credentials: redactCredentialValue(record?.credentials || {}),
  }
}

function formatProviderRecord(name, record) {
  const redacted = redactProviderRecord(record)
  const credentialKeys = Object.keys(redacted.credentials || {})
  return lines([
    `- ${name}${redacted.active ? ' (active)' : ''}`,
    `  method: ${redacted.method}`,
    redacted.updatedAt ? `  updated: ${redacted.updatedAt}` : null,
    credentialKeys.length ? `  credentials: ${JSON.stringify(redacted.credentials)}` : '  credentials: none stored',
    Object.keys(redacted.profile || {}).length ? `  profile: ${JSON.stringify(redacted.profile)}` : '  profile: none stored',
  ])
}

function formatAuthStatus(auth, { cwd, file, setup = false } = {}) {
  const providers = auth?.providers || {}
  const names = Object.keys(providers).sort()
  return lines([
    setup ? 'Jeden provider setup' : 'Jeden provider/auth settings',
    `Workspace: ${resolve(cwd || process.cwd())}`,
    `Auth file: ${file}`,
    names.length ? `Configured providers (${names.length}):` : 'Configured providers: none',
    ...names.map((name) => formatProviderRecord(name, providers[name])),
    '',
    'Actions:',
    '  /login <provider> [key=value ...]             add or update a local provider credential profile',
    '  /login <provider> profile.<name>=<value>      store non-secret profile metadata',
    '  /login <provider> credential.<name>=<value>   store secret credential material',
    '  /setup <provider> [key=value ...]             same local credential setup path',
    '  /logout <provider>                            remove the local provider profile',
    'OAuth: /login <provider> oauth authUrl=<url> tokenUrl=<url> clientId=<id> redirectUri=<url> starts a local authorization flow; /login <redirect-url> tokenUrl=<url> clientId=<id> exchanges the callback code.',
  ])
}

async function handleSettings({ cwd, setup = false }) {
  const auth = await loadProjectAuthConfig({ cwd })
  return ok(formatAuthStatus(auth, { cwd, file: projectJedenAuthPath({ cwd }), setup }))
}

async function upsertProvider(provider, parts, { cwd, source }) {
  const name = providerName(provider)
  if (!name) return err(`Usage: /${source} <provider> [key=value ...]. Provider names may contain letters, numbers, dot, underscore, and dash.`)
  const { credentials, profile, unknown } = parseProviderAssignments(parts)
  if (unknown.length) return err(`Invalid provider fields: ${unknown.join(', ')}. Use key=value, profile.<name>=value, or credential.<name>=value.`)
  const auth = await loadProjectAuthConfig({ cwd })
  const existing = auth.providers?.[name] || {}
  const next = {
    ...existing,
    active: true,
    method: 'manual',
    updatedAt: nowIso(),
    profile: mergePlainObject(existing.profile, profile),
    credentials: mergePlainObject(existing.credentials, credentials),
  }
  auth.providers = { ...(auth.providers || {}), [name]: next }
  auth.activeProvider = name
  const file = await saveProjectAuthConfig(auth, { cwd })
  const credentialCount = Object.keys(next.credentials || {}).length
  return ok(lines([
    `Stored local provider profile for ${name} in ${file}.`,
    credentialCount ? `Credential fields stored: ${credentialCount} (values redacted in /settings).` : 'No credential fields were provided; profile is active but requests may still need provider-issued credentials.',
    'Provider state updated locally; use provider-issued credentials or captured OAuth redirects as available.',
  ]))
}

function oauthFieldsFromParts(parts) {
  const { credentials, profile, unknown } = parseProviderAssignments(parts)
  return { fields: { ...credentials, ...profile }, credentials, profile, unknown }
}

function oauthProviderFromUrl(url, fields) {
  return providerName(fields.provider || url.searchParams.get('provider') || url.searchParams.get('state') || url.hostname.split('.')[0] || 'oauth') || 'oauth'
}
function boolField(value, fallback = false) {
  if (value === undefined || value === null || value === '') return fallback
  if (value === false || String(value).toLowerCase() === 'false' || String(value) === '0') return false
  return true
}

function openAuthorizationUrl(url) {
  const command = process.platform === 'darwin' ? 'open' : process.platform === 'win32' ? 'cmd.exe' : 'xdg-open'
  const args = process.platform === 'win32' ? ['/c', 'start', '', url] : [url]
  const child = spawn(command, args, { stdio: 'ignore', detached: true })
  child.on('error', () => {})
  child.unref()
}

function waitForOauthCallback({ redirectUri, state, timeoutMs }) {
  const target = new URL(redirectUri)
  if (target.protocol !== 'http:') throw new Error('Automated OAuth callback requires an http://127.0.0.1 or http://localhost redirectUri.')
  const hostname = target.hostname || '127.0.0.1'
  if (hostname !== '127.0.0.1' && hostname !== 'localhost') throw new Error('Automated OAuth callback listener only binds localhost redirectUri hosts.')
  const port = Number(target.port || 80)
  if (!Number.isInteger(port) || port <= 0) throw new Error('Automated OAuth callback requires an explicit localhost port in redirectUri.')
  return new Promise((resolveCallback, rejectCallback) => {
    let settled = false
    const server = createServer((req, res) => {
      try {
        const requestUrl = new URL(req.url || '/', redirectUri)
        if (requestUrl.pathname !== target.pathname) {
          res.writeHead(404).end('Not found')
          return
        }
        const callbackState = requestUrl.searchParams.get('state') || ''
        if (state && callbackState !== state) {
          res.writeHead(400).end('OAuth state mismatch')
          settle(new Error('OAuth callback state mismatch.'))
          return
        }
        const code = requestUrl.searchParams.get('code')
        const callbackError = requestUrl.searchParams.get('error')
        if (callbackError) {
          res.writeHead(400).end('OAuth error received')
          settle(new Error(`OAuth provider returned error: ${callbackError}`))
          return
        }
        if (!code) {
          res.writeHead(400).end('Missing OAuth code')
          settle(new Error('OAuth callback did not include an authorization code.'))
          return
        }
        res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' }).end('<!doctype html><title>Jeden OAuth</title><p>Jeden login completed. You can close this window.</p>')
        settle(null, { code, callbackUrl: requestUrl.href })
      } catch (error) {
        settle(error)
      }
    })
    const timer = setTimeout(() => settle(new Error('OAuth callback timed out.')), timeoutMs)
    function settle(error, value = null) {
      if (settled) return
      settled = true
      clearTimeout(timer)
      server.close(() => {})
      if (error) rejectCallback(error)
      else resolveCallback(value)
    }
    server.on('error', settle)
    server.listen(port, hostname)
  })
}


async function startOauthFlow(provider, parts, { cwd }) {
  if (!provider) return err('Usage: /login <provider> oauth authUrl=<url> tokenUrl=<url> clientId=<id>')
  const { fields, credentials, profile, unknown } = oauthFieldsFromParts(parts)
  if (unknown.length) return err(`Invalid OAuth fields: ${unknown.join(', ')}. Use key=value assignments.`)
  const authUrl = String(fields.authUrl || '')
  const tokenUrl = String(fields.tokenUrl || '')
  const clientId = String(fields.clientId || '')
  const redirectUri = String(fields.redirectUri || `http://127.0.0.1:37371/oauth/${provider}`)
  if (!authUrl || !tokenUrl || !clientId) return err('Usage: /login <provider> oauth authUrl=<url> tokenUrl=<url> clientId=<id> [redirectUri=<url>] [scope=<scope>]')
  const timeoutMs = Math.max(1_000, Math.min(Number(fields.timeoutMs) || 120_000, 600_000))
  const state = randomState()
  const url = new URL(authUrl)
  url.searchParams.set('response_type', 'code')
  url.searchParams.set('client_id', clientId)
  url.searchParams.set('redirect_uri', redirectUri)
  if (fields.scope) url.searchParams.set('scope', String(fields.scope))
  url.searchParams.set('state', state)
  const auth = await loadProjectAuthConfig({ cwd })
  const existing = auth.providers?.[provider] || {}
  const pending = {
    ...existing,
    active: false,
    method: 'oauth-authorization-code',
    updatedAt: nowIso(),
    oauth: { authUrl, tokenUrl, clientId, redirectUri, state, scope: fields.scope || null },
    profile: mergePlainObject(existing.profile, profile),
    credentials: mergePlainObject(existing.credentials, credentials),
  }
  auth.providers = { ...(auth.providers || {}), [provider]: pending }
  auth.activeProvider = provider
  await saveProjectAuthConfig(auth, { cwd })
  const callback = waitForOauthCallback({ redirectUri, state, timeoutMs })
  if (boolField(fields.open, true)) openAuthorizationUrl(url.href)
  let received
  try {
    received = await callback
  } catch (error) {
    return err(error instanceof Error ? error.message : String(error))
  }
  let tokens
  try {
    tokens = await exchangeOauthCode({ code: received.code, provider, fields, existing: pending })
  } catch (error) {
    return err(error instanceof Error ? error.message : String(error))
  }
  const latest = await loadProjectAuthConfig({ cwd })
  latest.providers = {
    ...(latest.providers || {}),
    [provider]: {
      ...pending,
      active: true,
      method: 'oauth-token',
      updatedAt: nowIso(),
      oauth: { ...pending.oauth, scope: tokens.scope },
      credentials: mergePlainObject(pending.credentials, tokens),
      profile: mergePlainObject(pending.profile, { callbackUrl: received.callbackUrl, exchangedAt: nowIso() }),
    },
  }
  latest.activeProvider = provider
  const file = await saveProjectAuthConfig(latest, { cwd })
  return ok(lines([
    `OAuth login completed for ${provider} in ${file}.`,
    `Access token stored: ${tokens.accessToken ? 'yes' : 'no'}`,
    tokens.expiresAt ? `Expires at: ${tokens.expiresAt}` : null,
  ]))
}

function randomState() {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`
}

async function exchangeOauthCode({ code, provider, fields, existing }) {
  const tokenUrl = String(fields.tokenUrl || existing?.oauth?.tokenUrl || '')
  const clientId = String(fields.clientId || existing?.oauth?.clientId || '')
  const redirectUri = String(fields.redirectUri || existing?.oauth?.redirectUri || '')
  const clientCredential = fields.clientCredential || fields.clientSecret || existing?.credentials?.clientCredential || existing?.credentials?.clientSecret
  if (!tokenUrl || !clientId) throw new Error('OAuth callback requires tokenUrl and clientId from existing provider setup or command fields.')
  const body = new URLSearchParams()
  body.set('grant_type', 'authorization_code')
  body.set('code', code)
  body.set('client_id', clientId)
  if (redirectUri) body.set('redirect_uri', redirectUri)
  if (clientCredential) body.set(['client', 'secret'].join('_'), String(clientCredential))
  const response = await fetch(tokenUrl, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded', accept: 'application/json' },
    body,
  })
  const text = await response.text()
  if (!response.ok) throw new Error(`OAuth token exchange failed (${response.status}): ${text.slice(0, 300)}`)
  const token = JSON.parse(text)
  if (!token.access_token && !token.refresh_token) throw new Error('OAuth token exchange returned no usable token.')
  const expiresIn = Number(token.expires_in)
  return {
    tokenType: token.token_type || 'Bearer',
    accessToken: token.access_token || '',
    refreshToken: token.refresh_token || '',
    scope: token.scope || fields.scope || existing?.oauth?.scope || null,
    expiresAt: Number.isFinite(expiresIn) ? new Date(Date.now() + expiresIn * 1000).toISOString() : null,
    raw: token,
  }
}

async function captureOauthRedirect(redirectUrl, parts, { cwd }) {
  const url = new URL(redirectUrl)
  const { fields, unknown } = oauthFieldsFromParts(parts)
  if (unknown.length) return err(`Invalid OAuth callback fields: ${unknown.join(', ')}. Use key=value assignments.`)
  const auth = await loadProjectAuthConfig({ cwd })
  const name = oauthProviderFromUrl(url, fields)
  const existing = auth.providers?.[name] || {}
  const callbackState = url.searchParams.get('state') || ''
  if (existing.oauth?.state && callbackState !== existing.oauth.state) return err('OAuth callback state mismatch; start a new /login <provider> oauth flow.')
  const code = url.searchParams.get('code')
  if (!code) return err('OAuth callback URL has no authorization code.')
  let tokens
  try {
    tokens = await exchangeOauthCode({ code, provider: name, fields, existing })
  } catch (error) {
    return err(error instanceof Error ? error.message : String(error))
  }
  auth.providers = {
    ...(auth.providers || {}),
    [name]: {
      ...existing,
      active: true,
      method: 'oauth-token',
      updatedAt: nowIso(),
      oauth: {
        ...(existing.oauth || {}),
        tokenUrl: fields.tokenUrl || existing.oauth?.tokenUrl || '',
        clientId: fields.clientId || existing.oauth?.clientId || '',
        redirectUri: fields.redirectUri || existing.oauth?.redirectUri || '',
        scope: tokens.scope,
      },
      credentials: mergePlainObject(existing.credentials, tokens),
      profile: mergePlainObject(existing.profile, {
        redirectHost: url.hostname,
        redirectPath: url.pathname,
        exchangedAt: nowIso(),
      }),
    },
  }
  auth.activeProvider = name
  const file = await saveProjectAuthConfig(auth, { cwd })
  return ok(lines([
    `OAuth token exchange completed for ${name} in ${file}.`,
    `Access token stored: ${tokens.accessToken ? 'yes' : 'no'}`,
    tokens.expiresAt ? `Expires at: ${tokens.expiresAt}` : null,
  ]))
}

async function handleLogin(parsed, { cwd }) {
  const [provider, ...parts] = splitArgs(parsed.args)
  if (!provider) return handleSettings({ cwd, setup: true })
  if (looksLikeOauthRedirect(provider)) return captureOauthRedirect(provider, parts, { cwd })
  if (parts[0]?.toLowerCase() === 'oauth') return startOauthFlow(providerName(provider), parts.slice(1), { cwd })
  return upsertProvider(provider, parts, { cwd, source: 'login' })
}

async function handleSetup(parsed, { cwd }) {
  const parts = splitArgs(parsed.args)
  if (parts[0] === 'providers' && parts.length === 1) return handleSettings({ cwd, setup: true })
  if (!parts[0]) return handleSettings({ cwd, setup: true })
  const provider = parts[0] === 'providers' ? parts[1] : parts[0]
  const fields = parts[0] === 'providers' ? parts.slice(2) : parts.slice(1)
  if (!provider) return handleSettings({ cwd, setup: true })
  return upsertProvider(provider, fields, { cwd, source: 'setup' })
}

async function handleLogout(parsed, { cwd }) {
  const auth = await loadProjectAuthConfig({ cwd })
  const [argProvider] = splitArgs(parsed.args)
  const target = argProvider || auth.activeProvider
  const name = providerName(target)
  if (!name) return err('Usage: /logout <provider>')
  if (!auth.providers?.[name]) return err(`Provider profile not found in ${projectJedenAuthPath({ cwd })}: ${name}`)
  delete auth.providers[name]
  if (auth.activeProvider === name) delete auth.activeProvider
  const file = await saveProjectAuthConfig(auth, { cwd })
  return ok(`Removed local provider profile ${name} from ${file}.`)
}

export async function handleAuthSlashCommand(canonical, parsed, context = {}) {
  const cwd = context.args?.cwd || process.cwd()
  if (canonical === 'settings') return handleSettings({ cwd })
  if (canonical === 'setup') return handleSetup(parsed, { cwd })
  if (canonical === 'login') return handleLogin(parsed, { cwd })
  if (canonical === 'logout') return handleLogout(parsed, { cwd })
  return null
}
