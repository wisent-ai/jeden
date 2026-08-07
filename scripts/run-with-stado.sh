#!/usr/bin/env bash
set -euo pipefail

agent_secret_item="${JEDEN_AGENT_SECRET_NAME:-agent:wisent-app}"
model_router_item="${JEDEN_MODEL_ROUTER_ITEM:-jeden-model-router}"
skarbiec_consumer="${JEDEN_SKARBIEC_CONSUMER:-local-operator}"
skarbiec_token_file="${JEDEN_SKARBIEC_TOKEN_FILE:-$HOME/.stado/local-operator-skarbiec-token}"
stado_bin="${JEDEN_STADO_BIN:-stado}"
jeden_bin="${JEDEN_BIN:-jeden}"

# The gateway verifies this signature by reading the item out of the Skarbiec
# vault itself, so the value that authenticates is the item's current revision
# and nothing else. On this workstation the local Skarbiec service answers the
# same coordinate with an older revision -- both CLI builds read the vault's
# current value while the service returns a superseded one -- and a caller that
# signs with the service's answer is refused by a verifier behaving correctly,
# with a 401 that names neither side.
#
# So the vault is asked first and the managed path stays as the fallback: one
# authority for the value, the same one the far end checks against, and hosts
# where the vault is not readable directly keep working.
skarbiec_bin="${JEDEN_SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}"
vault_file="${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}"

if [[ -z "${WISENT_APP_AGENT_AUTH_SECRET:-}" && -x "$skarbiec_bin" && -f "$vault_file" ]]; then
  WISENT_APP_AGENT_AUTH_SECRET="$(
    SKARBIEC_VAULT_FILE="$vault_file" "$skarbiec_bin" get "$agent_secret_item" 2>/dev/null |
      "${JEDEN_PYTHON_BIN:-python3}" -c 'import json,sys; print((json.load(sys.stdin).get("fields") or {}).get("value",""))' 2>/dev/null
  )" || WISENT_APP_AGENT_AUTH_SECRET=""
  if [[ -n "$WISENT_APP_AGENT_AUTH_SECRET" ]]; then
    export WISENT_APP_AGENT_AUTH_SECRET
  fi
fi

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

# BRAMA_TOKEN is optional to this launcher and required by the binary whenever
# the configured Brama demands a bearer, which the fleet gateway does: an
# unauthenticated /v1/models there answers 401. This is the trusted launcher, so
# it injects the bearer rather than leaving the binary to fail with a message
# naming a credential and no way to obtain it.
#
# The prohibition this replaces still holds: the agent capability read above is
# for `agent:wisent-app` and is never reused. The bearer is a separate item read
# under the operator's own consumer, which is the identity the vault grants
# `read jeden-model-router#token` to -- `jeden-model-router-client`, the
# consumer an earlier launcher used, holds no such grant and answers 403.
if [[ -z "${BRAMA_URL:-}" && -n "${JEDEN_BRAMA_URL:-}" ]]; then
  export BRAMA_URL="$JEDEN_BRAMA_URL"
fi

if [[ -z "${BRAMA_TOKEN:-}" ]]; then
  if BRAMA_TOKEN="$(
    WC_SKARBIEC_CONSUMER="$skarbiec_consumer" \
    WC_SKARBIEC_TOKEN_FILE="$skarbiec_token_file" \
      "$stado_bin" credentials get --field token "$model_router_item"
  )" && [[ -n "$BRAMA_TOKEN" ]]; then
    export BRAMA_TOKEN
  else
    unset BRAMA_TOKEN
    printf '%s\n' \
      "Jeden launcher could not read $model_router_item for Skarbiec consumer $skarbiec_consumer; a Brama that requires a bearer will refuse this run" \
      >/dev/stderr
  fi
fi

# Onboarding telemetry is optional. The first-use journey always runs from the
# definition compiled into the binary; a configured endpoint only adds bundle
# reads and event collection at the integration boundary. The client has its
# own consumer and grant file, so the agent capability read above is never
# reused for it.
#
# The endpoint itself is read by the binary from ~/.jeden/.env, which this shell
# never parses, so the grant file — not an exported URL — is what decides
# whether the credential is fetched. Without the endpoint the binary stays on
# OfflineTransport and the injected value is simply unused.
onboarding_endpoint="${STADO_INTEGRATION_API_URL:-${JEDEN_STADO_INTEGRATION_API_URL:-}}"
onboarding_item="${JEDEN_ONBOARDING_INTEGRATION_ITEM:-jeden-integration-api}"
onboarding_consumer="${JEDEN_ONBOARDING_SKARBIEC_CONSUMER:-jeden-onboarding-client}"
onboarding_token_file="${JEDEN_ONBOARDING_SKARBIEC_TOKEN_FILE:-$HOME/.stado/jeden-onboarding-client-skarbiec-token}"

if [[ -n "$onboarding_endpoint" ]]; then
  export STADO_INTEGRATION_API_URL="$onboarding_endpoint"
fi

if [[ -z "${JEDEN_STADO_INTEGRATION_TOKEN:-}" && -f "$onboarding_token_file" ]]; then
  if ! JEDEN_STADO_INTEGRATION_TOKEN="$(
    WC_SKARBIEC_CONSUMER="$onboarding_consumer" \
    WC_SKARBIEC_TOKEN_FILE="$onboarding_token_file" \
      "$stado_bin" credentials get --field token "$onboarding_item"
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
export WISENT_APP_AGENT_ID="${WISENT_APP_AGENT_ID:-wisent-app}"
exec "$jeden_bin" "$@"
