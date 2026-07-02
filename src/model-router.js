import { createHash, createHmac } from 'node:crypto'

export function modelRouterConfig(env = process.env) {
  const url = env.MODEL_ROUTER_URL || 'https://model-router-1080673333190.us-central1.run.app'
  const agentId = env.WISENT_APP_AGENT_ID || 'wisent-app'
  const secret = env.WISENT_APP_AGENT_AUTH_SECRET || ''
  const model = env.JEDEN_MODEL || 'claude-code-subscription'
  return { url, agentId, secret, model }
}

export function hmacHeaders({ body, agentId, secret }) {
  if (!secret) throw new Error('WISENT_APP_AGENT_AUTH_SECRET is required')
  const ts = String(Math.floor(Date.now() / 1000))
  const bodyHash = createHash('sha256').update(body).digest('hex')
  const signature = createHmac('sha256', secret).update(`${agentId}:${ts}:${bodyHash}`).digest('hex')
  return {
    'x-agent-id': agentId,
    'x-agent-timestamp': ts,
    'x-agent-signature': signature,
    'Content-Type': 'application/json',
  }
}

export async function chatCompletion({ messages, maxTokens = 2048, config = modelRouterConfig(), fetchImpl = fetch, tools = [] }) {
  const body = JSON.stringify({
    model: config.model,
    max_tokens: maxTokens,
    messages,
    ...(tools.length > 0 ? { tools, tool_choice: 'auto' } : {}),
  })
  const endpoint = `${config.url.replace(/\/$/, '')}/v1/chat/completions`
  const response = await fetchImpl(endpoint, {
    method: 'POST',
    headers: hmacHeaders({ body, agentId: config.agentId, secret: config.secret }),
    body,
  })
  const text = await response.text()
  if (!response.ok) throw new Error(`model router ${response.status}: ${text.slice(0, 800)}`)
  const data = JSON.parse(text)
  const message = data?.choices?.[0]?.message
  const toolCall = Array.isArray(message?.tool_calls) ? message.tool_calls[0] : null
  const name = toolCall?.function?.name
  if (name) {
    const rawArgs = toolCall?.function?.arguments || '{}'
    const input = rawArgs ? JSON.parse(rawArgs) : {}
    return JSON.stringify({ action: 'tool', tool: name, input })
  }
  const content = message?.content
  if (typeof content !== 'string' || !content.trim()) throw new Error('model router returned no message content')
  return content
}
