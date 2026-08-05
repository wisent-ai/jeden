#!/usr/bin/env bash
set -euo pipefail

runtime_item="${JEDEN_RUNTIME_SECRET_NAME:-jeden-runtime}"
model_router_item="${JEDEN_MODEL_ROUTER_SECRET_NAME:-jeden-model-router}"
model_router_consumer="${JEDEN_MODEL_ROUTER_SKARBIEC_CONSUMER:-jeden-model-router-client}"
model_router_token_file="${JEDEN_MODEL_ROUTER_SKARBIEC_TOKEN_FILE:-$HOME/.stado/jeden-model-router-skarbiec-token}"
runtime_consumer="${JEDEN_RUNTIME_SKARBIEC_CONSUMER:-jeden-runtime-client}"
runtime_token_file="${JEDEN_RUNTIME_SKARBIEC_TOKEN_FILE:-$HOME/.stado/jeden-runtime-skarbiec-token}"
jeden_bin="${JEDEN_BIN:-jeden}"

model_router_token="$(WC_SKARBIEC_CONSUMER="$model_router_consumer" \
  WC_SKARBIEC_TOKEN_FILE="$model_router_token_file" \
  stado secrets get "$model_router_item" --field token)"
if [[ -z "$model_router_token" ]]; then
  printf '%s\n' "missing scoped Brama model-router credential" >/dev/stderr
  false
fi
export BRAMA_TOKEN="$model_router_token"
unset model_router_token

WC_SKARBIEC_CONSUMER="$runtime_consumer" \
WC_SKARBIEC_TOKEN_FILE="$runtime_token_file" \
stado secrets get "$runtime_item" --field value | python3 -c '
import os
import sys

allowed = {
    "WELES_URL",
    "WELES_TOKEN",
    "BRAMA_URL",
    "STADO_MEDIA_ROUTER_URL",
    "WISENT_APP_AGENT_ID",
    "WISENT_APP_AGENT_AUTH_SECRET",
}
text = sys.stdin.read()
if not text:
    raise SystemExit("Jeden runtime item has an empty value field")
for raw in text.splitlines():
    line = raw.strip()
    if not line or line.startswith("#"):
        continue
    line = line.removeprefix("export ").lstrip()
    key, separator, value = line.partition("=")
    if not separator:
        raise SystemExit("invalid line in Jeden runtime item")
    key = key.strip()
    value = value.strip()
    if key not in allowed:
        raise SystemExit(f"unexpected key in Jeden runtime item: {key}")
    if (value.startswith("\"") and value.endswith("\"")) or (value.startswith("\047") and value.endswith("\047")):
        value = value.removeprefix(value[:1]).removesuffix(value[-1:])
    os.environ[key] = value.replace("\\n", "\n")
_, program, *arguments = sys.argv
os.execvp(program, [program, *arguments])
' "$jeden_bin" "$@"
