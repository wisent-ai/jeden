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

# Onboarding telemetry is optional. The first-use journey always runs from the
# definition compiled into the binary; a configured endpoint only adds bundle
# reads and event collection at the integration boundary. The client has its
# own consumer and grant file, so the agent capability read above is never
# reused for it.
onboarding_endpoint="${STADO_INTEGRATION_API_URL:-${JEDEN_STADO_INTEGRATION_API_URL:-}}"
onboarding_item="${JEDEN_ONBOARDING_INTEGRATION_ITEM:-jeden-integration-api}"
onboarding_consumer="${JEDEN_ONBOARDING_SKARBIEC_CONSUMER:-jeden-onboarding-client}"
onboarding_token_file="${JEDEN_ONBOARDING_SKARBIEC_TOKEN_FILE:-$HOME/.stado/jeden-onboarding-client-skarbiec-token}"

if [[ -n "$onboarding_endpoint" ]]; then
  export STADO_INTEGRATION_API_URL="$onboarding_endpoint"
  if [[ -z "${JEDEN_STADO_INTEGRATION_TOKEN:-}" ]]; then
    if [[ ! -f "$onboarding_token_file" ]]; then
      printf '%s\n' \
        "Jeden launcher found no onboarding grant at $onboarding_token_file; the first-use journey stays offline" \
        >/dev/stderr
    elif ! JEDEN_STADO_INTEGRATION_TOKEN="$(
      WC_SKARBIEC_CONSUMER="$onboarding_consumer" \
      WC_SKARBIEC_TOKEN_FILE="$onboarding_token_file" \
        "$stado_bin" credentials get --field value "$onboarding_item"
    )"; then
      unset JEDEN_STADO_INTEGRATION_TOKEN
      printf '%s\n' \
        "Jeden launcher could not read $onboarding_item for Skarbiec consumer $onboarding_consumer; the first-use journey stays offline" \
        >/dev/stderr
    elif [[ -z "$JEDEN_STADO_INTEGRATION_TOKEN" ]]; then
      unset JEDEN_STADO_INTEGRATION_TOKEN
      printf '%s\n' \
        "Jeden onboarding integration credential is empty; the first-use journey stays offline" \
        >/dev/stderr
    else
      export JEDEN_STADO_INTEGRATION_TOKEN
    fi
  fi
fi
export WISENT_APP_AGENT_ID="${WISENT_APP_AGENT_ID:-wisent-app}"
exec "$jeden_bin" "$@"
