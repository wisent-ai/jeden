#!/bin/sh
# Real test that reading a recorded session has one command: `jeden
# recall_conversation`, which printed what `jeden show` prints, is refused as
# an unknown command, and `jeden show` still reads the newest recorded
# session of this machine. Every command, its exit status and output go to
# the run's report.txt.
#
# Usage: tests/cli/removed_recall.sh   (JEDEN selects the binary, default
#   target/debug/jeden)
set -eu
cd "$(dirname "$0")/../.."
BIN=${JEDEN:-target/debug/jeden}
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/cli/$RUN"
REPORT="$ROOT/report.txt"
mkdir -p "$ROOT"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "binary: $BIN" >>"$REPORT"

run() {
  expected=$1
  shift
  set +e
  out=$("$BIN" "$@" 2>"$ROOT/stderr" </dev/null)
  status=$?
  set -e
  err=$(cat "$ROOT/stderr")
  printf '$ jeden %s\nexit: %s\nstdout: %.2000s\nstderr: %.2000s\n\n' "$*" "$status" "$out" "$err" >>"$REPORT"
  if [ "$status" -ne "$expected" ]; then
    echo "FAIL: jeden $* exited $status, expected $expected: $err" | tee -a "$REPORT" >&2
    exit 1
  fi
}
refused() {
  case "$err" in
    *"$1"*) echo "ok: refused with: $1" >>"$REPORT" ;;
    *) echo "FAIL: expected a refusal containing '$1', got: $err" | tee -a "$REPORT" >&2; exit 1 ;;
  esac
}

run 2 recall_conversation
refused "unknown command: recall_conversation"
run 2 recall-conversation
refused "unknown command: recall-conversation"

run 0 sessions 1 --json
session=$(echo "$out" | jq -r '.[0] // empty')
if [ -n "$session" ]; then
  run 0 show "$session" --json
  echo "ok: show reads session $session" >>"$REPORT"
else
  echo "ok: no recorded session on this machine; show has nothing to read" >>"$REPORT"
fi

touch "$ROOT/passed"
echo "PASS" >>"$REPORT"
echo "PASS: $REPORT"
