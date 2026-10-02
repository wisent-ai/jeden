#!/usr/bin/env sh
# Drive `jeden rpc` over stdio — companion to docs/rpc.md.
#
# Sends the canonical request sequence (initialize, session/new,
# session/prompt, session/status, shutdown) as newline-delimited JSON and
# prints every frame the server answers, banner included. With no model
# credential in the environment the prompt fails closed with
# `prompt_failed` — the framing is identical either way.
#
# session/status, session/dispose and shutdown are sent only once the prompt
# (id 3) has answered, so the status read sees the finished turn and dispose
# never cancels it. A server that closes before answering is reported with
# the last request it left unanswered.
# Requires: jeden (or JEDEN_BIN=path).
set -eu

JEDEN="${JEDEN_BIN:-jeden}"
CWD="${1:-$PWD}"
PROMPT="${2:-Respond exactly: OK}"

pipes="$(mktemp -d)"
trap 'rm -rf "$pipes"' EXIT
mkfifo "$pipes/requests"
# Held open for reading and writing, so the server sees no end of input
# until shutdown has been sent.
exec 3<>"$pipes/requests"

printf '%s\n' '{"id":1,"method":"initialize"}' >&3
printf '{"id":2,"method":"session/new","params":{"cwd":"%s"}}\n' "$CWD" >&3
printf '{"id":3,"method":"session/prompt","params":{"sessionId":"session-1","prompt":"%s"}}\n' "$PROMPT" >&3

"$JEDEN" rpc <"$pipes/requests" | {
  waiting="the prompt (id 3)"
  while IFS= read -r frame; do
    printf '%s\n' "$frame"
    case "$frame" in
      '{"id":3,'*)
        printf '%s\n' '{"id":4,"method":"session/status","params":{"sessionId":"session-1"}}' >&3
        printf '%s\n' '{"id":5,"method":"session/dispose","params":{"sessionId":"session-1"}}' >&3
        printf '%s\n' '{"id":6,"method":"shutdown"}' >&3
        waiting="shutdown (id 6)"
        ;;
      '{"id":6,'*) exit 0 ;;
    esac
  done
  printf 'rpc-drive: jeden rpc closed before answering %s\n' "$waiting" >&2
  exit 1
}
