#!/usr/bin/env sh
# Offline refusal audit — runnable version of
# docs/walkthrough-offline-refusals.md.
#
# Runs Jeden in an isolated throwaway HOME with no model credential and
# shows that every refusal is exact, fail-closed, and still ledgered.
# Nothing leaves the machine. Requires: jeden (or JEDEN_BIN=path), python3.
set -eu

JEDEN="${JEDEN_BIN:-jeden}"
HOME="$(mktemp -d)"
export HOME
mkdir -p "$HOME/project"
cd "$HOME/project"
unset BRAMA_URL BRAMA_TOKEN WISENT_APP_AGENT_AUTH_SECRET \
      STADO_MODEL_ROUTER_URL STADO_MODEL_ROUTER_TOKEN JEDEN_MODEL || true

echo "== version"
"$JEDEN" --version

echo "== 1. one-shot refusal (expect: BRAMA_URL is required...; exit 1)"
"$JEDEN" run "Respond exactly: OK" || echo "exit=$?"

echo "== the refused run is still a session"
"$JEDEN" sessions
SID="$("$JEDEN" sessions | head -1)"
python3 -c "import sys,json; [print(json.loads(l)['payload']['type']) for l in open(sys.argv[1])]" \
  "$HOME/.jeden/sessions/$SID/transcript.jsonl"

echo "== 2. same refusal over RPC"
{ printf '%s\n' '{"id":1,"method":"initialize"}'
  printf '%s\n' "{\"id\":2,\"method\":\"session/new\",\"params\":{\"cwd\":\"$HOME/project\"}}"
  printf '%s\n' '{"id":3,"method":"session/prompt","params":{"sessionId":"session-1","prompt":"Respond exactly: OK"}}'
  sleep 2
  printf '%s\n' '{"id":4,"method":"session/status","params":{"sessionId":"session-1"}}'
  printf '%s\n' '{"id":5,"method":"shutdown"}'
} | "$JEDEN" rpc

echo "== RPC error surface"
{ printf '%s\n' '{"id":10,"method":"bogus"}'
  printf '%s\n' '{bad'
  printf '%s\n' '{"id":12,"method":"session/new","params":{"cwd":123}}'
  printf '%s\n' '{"id":13,"method":"session/status","params":{"sessionId":"session-9"}}'
} | "$JEDEN" rpc | sed -n '2,5p'

echo "== 3. every credential-bearing command refuses the same way"
"$JEDEN" token || echo "exit=$?"
"$JEDEN" pursue "make a demo" || echo "exit=$?"

echo "== 4. doctor names what is missing (expect exit 1: brama unavailable)"
"$JEDEN" doctor > doctor.json || echo "exit=$?"
python3 -c "
import json
d = json.load(open('doctor.json'))
print('healthy', d['healthy'])
for p in d['probes']:
    print(p['subsystem'], p['state'], '|', p['detail'])
"

echo "== 5. accounting with zero spend"
"$JEDEN" stats --summary

echo "== done; evidence in $HOME"
