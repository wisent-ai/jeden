#!/usr/bin/env bash
set -euo pipefail

model_router_item="${JEDEN_MODEL_ROUTER_SECRET_NAME:-jeden-model-router}"
agent_secret_item="${JEDEN_AGENT_SECRET_NAME:-agent:wisent-app}"
skarbiec_consumer="${JEDEN_SKARBIEC_CONSUMER:-local-operator}"
skarbiec_token_file="${JEDEN_SKARBIEC_TOKEN_FILE:-$HOME/.stado/local-operator-skarbiec-token}"
jeden_bin="${JEDEN_BIN:-jeden}"

get_secret_field() {
  WC_SKARBIEC_CONSUMER="$skarbiec_consumer" \
  WC_SKARBIEC_TOKEN_FILE="$skarbiec_token_file" \
    stado secrets get "$1" --field "$2"
}

if [[ -z "${BRAMA_TOKEN:-}" ]]; then
  BRAMA_TOKEN="$(get_secret_field "$model_router_item" token)"
  if [[ -z "$BRAMA_TOKEN" ]]; then
    printf '%s\n' "missing Brama model-router credential" >/dev/stderr
    false
  fi
  export BRAMA_TOKEN
fi

if [[ -z "${WISENT_APP_AGENT_AUTH_SECRET:-}" ]]; then
  WISENT_APP_AGENT_AUTH_SECRET="$(get_secret_field "$agent_secret_item" value)"
  if [[ -z "$WISENT_APP_AGENT_AUTH_SECRET" ]]; then
    printf '%s\n' "missing Jeden agent signing credential" >/dev/stderr
    false
  fi
  export WISENT_APP_AGENT_AUTH_SECRET
fi

export BRAMA_URL="${BRAMA_URL:-${JEDEN_BRAMA_URL:-http://127.0.0.1:8080}}"
export WISENT_APP_AGENT_ID="${WISENT_APP_AGENT_ID:-wisent-app}"
exec "$jeden_bin" "$@"
