#!/usr/bin/env bash
# Real test of `jeden sessions search`, the one way to search recorded
# sessions: it reads this machine's own session store read-only and answers as
# JSON, the retired `search-sessions` is an unknown command, a missing store is
# an empty answer, a store that is not a directory is refused naming it, and a
# blank query or a limit that is not a whole number is refused. Every command,
# whether it succeeded and its output go to the run's report.txt.
#
# Usage: JEDEN=target/debug/jeden tests/sessions/search.sh
set -eu
cd "$(dirname "$0")/../.."
BIN=${JEDEN:?set JEDEN to the jeden binary under test, e.g. JEDEN=target/debug/jeden}
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/sessions/$RUN"
REPORT="$ROOT/report.txt"
mkdir -p "$ROOT"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "binary: $BIN" >>"$REPORT"

fail() {
  echo "FAIL: $*" | tee -a "$REPORT" >/dev/stderr
  false
}
# WANT=ok|refused run ARGS...: standard output and error together in $out.
run() {
  if out=$(set -o pipefail; "$BIN" "$@" </dev/null |& cat); then got=ok; else got=refused; fi
  printf '$ jeden %s\nresult: %s\noutput: %s\n\n' "$*" "$got" "$out" >>"$REPORT"
  [ "$got" = "$WANT" ] || fail "jeden $* was $got, expected $WANT: $out"
}
says() {
  case "$out" in
    *"$1"*) echo "ok: $1" >>"$REPORT" ;;
    *) fail "expected '$1' in: $out" ;;
  esac
}

# This machine's own store, read only: whatever it holds, the answer is a JSON
# array of {session, ts, type, snippet}.
WANT=ok run sessions search session --json
echo "$out" | jq -e 'type == "array" and all(.[]; has("session") and has("ts") and has("type") and has("snippet"))' >/dev/null \
  || fail "sessions search --json did not answer an array of hits"
echo "ok: sessions search --json answers an array of hits" >>"$REPORT"

WANT=refused run search-sessions session
says "unknown command: search-sessions"
WANT=refused run sessions search "   "
says "sessions search requires a non-empty query"
WANT=refused run sessions search session newest
says "sessions search takes a whole number of sessions to scan after the query"

export JEDEN_SESSION_ROOT="$ROOT/no-store-here"
WANT=ok run sessions search session --json
[ "$out" = "[]" ] || fail "a missing store answered $out"
echo "ok: a missing store is an empty answer" >>"$REPORT"

: >"$ROOT/not-a-directory"
export JEDEN_SESSION_ROOT="$ROOT/not-a-directory"
WANT=refused run sessions search session
says "cannot list sessions in $ROOT/not-a-directory"

touch "$ROOT/passed"
echo "PASS" >>"$REPORT"
echo "PASS: $REPORT"
