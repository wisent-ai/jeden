#!/usr/bin/env bash
set -euo pipefail

agent_secret_item="${JEDEN_AGENT_SECRET_NAME:-agent:wisent-app}"
skarbiec_consumer="${JEDEN_SKARBIEC_CONSUMER:-local-operator}"
skarbiec_token_file="${JEDEN_SKARBIEC_TOKEN_FILE:-$HOME/.stado/local-operator-skarbiec-token}"
stado_bin="${JEDEN_STADO_BIN:-stado}"
jeden_bin="${JEDEN_BIN:-jeden}"

if [[ -z "${WISENT_APP_AGENT_AUTH_SECRET:-}" ]]; then
  if ! WISENT_APP_AGENT_AUTH_SECRET="$(
    WC_SKARBIEC_CONSUMER="$skarbiec_consumer" \
    WC_SKARBIEC_TOKEN_FILE="$skarbiec_token_file" \
      "$stado_bin" credentials get --field value "$agent_secret_item"
  )"; then
    printf '%s\n' \
      "Jeden launcher could not read $agent_secret_item for Skarbiec consumer $skarbiec_consumer" \
      >/dev/stderr
    exit 1
  fi
  if [[ -z "$WISENT_APP_AGENT_AUTH_SECRET" ]]; then
    printf '%s\n' "Jeden agent signing credential is empty" >/dev/stderr
    exit 1
  fi
  export WISENT_APP_AGENT_AUTH_SECRET
fi

# BRAMA_TOKEN is optional. When the configured Brama deployment requires a
# bearer, its trusted launcher must inject BRAMA_TOKEN directly; this launcher
# must not use the agent's Skarbiec capability to read an unrelated bearer.
if [[ -z "${BRAMA_URL:-}" && -n "${JEDEN_BRAMA_URL:-}" ]]; then
  export BRAMA_URL="$JEDEN_BRAMA_URL"
fi
export WISENT_APP_AGENT_ID="${WISENT_APP_AGENT_ID:-wisent-app}"
exec "$jeden_bin" "$@"
