#!/usr/bin/env sh
# Offline refusal audit — runnable version of
# docs/walkthrough-offline-refusals.md.
#
# Runs Jeden in an isolated throwaway HOME with no model credential and
# shows that every refusal is exact, fail-closed, and still ledgered.
# Nothing leaves the machine. Requires: JEDEN_BIN=<path to jeden>, jq.
# A step that is expected to be refused prints its exit status and goes on.
set -eu

JEDEN="${JEDEN_BIN:?set JEDEN_BIN to the jeden executable this walkthrough runs}"
HOME="$(mktemp -d)"
export HOME
mkdir -p "$HOME/project"
cd "$HOME/project"
# No model route: every model call is refused offline, naming BRAMA_URL.
unset BRAMA_URL BRAMA_TOKEN WISENT_APP_AGENT_AUTH_SECRET JEDEN_MODEL

expect_refusal() {
  if "$@"; then
    echo "(answered)"
  else
    echo "(refused, exit status $?)"
  fi
}

echo "== version"
"$JEDEN" --version

echo "== 1. one-shot refusal (expect: BRAMA_URL is required...; exit 1)"
expect_refusal "$JEDEN" run "Respond exactly: OK"

echo "== the refused run is still a session"
"$JEDEN" sessions
SID="$("$JEDEN" sessions | sed -n '1p')"
jq -r '.payload.type' "$HOME/.jeden/sessions/$SID/transcript.jsonl"

echo "== 2. same refusal over RPC"
# status and shutdown follow the prompt's own answer (id 3), so shutdown never
# cancels the turn whose refusal this step shows. A holder keeps the request
# FIFO open for writing, so jeden rpc does not read end-of-input between
# requests; it ends when the shutdown request is answered.
REQUESTS="$HOME/rpc-requests"
mkfifo "$REQUESTS"
tail -f /dev/null > "$REQUESTS" &
HOLDER=$!
send() { printf '%s\n' "$1" > "$REQUESTS"; }
{
  send '{"id":1,"method":"initialize"}'
  send "{\"id\":2,\"method\":\"session/new\",\"params\":{\"cwd\":\"$HOME/project\"}}"
  send '{"id":3,"method":"session/prompt","params":{"sessionId":"session-1","prompt":"Respond exactly: OK"}}'
} &
"$JEDEN" rpc < "$REQUESTS" | {
  while IFS= read -r frame; do
    printf '%s\n' "$frame"
    case "$frame" in
      '{"id":3,'*)
        send '{"id":4,"method":"session/status","params":{"sessionId":"session-1"}}'
        send '{"id":5,"method":"shutdown"}'
        ;;
      '{"id":5,'*) exit ;;
    esac
  done
  printf '%s\n' "jeden rpc closed before answering the prompt and shutdown" > /dev/stderr
  false
}
kill "$HOLDER"

echo "== RPC error surface"
{ printf '%s\n' '{"id":10,"method":"bogus"}'
  printf '%s\n' '{bad'
  printf '%s\n' '{"id":12,"method":"session/new","params":{"cwd":123}}'
  printf '%s\n' '{"id":13,"method":"session/status","params":{"sessionId":"session-9"}}'
} | "$JEDEN" rpc | sed -n '2,5p'

echo "== 3. every credential-bearing command refuses the same way"
expect_refusal "$JEDEN" token
expect_refusal "$JEDEN" pursue "make a demo"

echo "== 4. doctor names what is missing (expect exit 1: brama unavailable)"
expect_refusal sh -c '"$1" doctor > doctor.json' doctor "$JEDEN"
jq -r '"healthy \(.healthy)", (.probes[] | "\(.subsystem) \(.state) | \(.detail)")' doctor.json

echo "== 5. accounting with zero spend"
"$JEDEN" stats --summary

echo "== done; evidence in $HOME"
