#!/usr/bin/env sh
# Drive `jeden rpc` over stdio — companion to docs/rpc.md.
#
# Sends the canonical request sequence (initialize, session/new,
# session/prompt, session/status, shutdown) as newline-delimited JSON and
# prints every frame the server answers, banner included. With no model
# credential in the environment the prompt fails closed with
# `prompt_failed` — the framing is identical either way.
# Requires: jeden (or JEDEN_BIN=path).
set -eu

JEDEN="${JEDEN_BIN:-jeden}"
CWD="${1:-$PWD}"
PROMPT="${2:-Respond exactly: OK}"

{ printf '%s\n' '{"id":1,"method":"initialize"}'
  printf '{"id":2,"method":"session/new","params":{"cwd":"%s"}}\n' "$CWD"
  printf '{"id":3,"method":"session/prompt","params":{"sessionId":"session-1","prompt":"%s"}}\n' "$PROMPT"
  sleep 2
  printf '%s\n' '{"id":4,"method":"session/status","params":{"sessionId":"session-1"}}'
  printf '%s\n' '{"id":5,"method":"session/dispose","params":{"sessionId":"session-1"}}'
  printf '%s\n' '{"id":6,"method":"shutdown"}'
} | "$JEDEN" rpc
