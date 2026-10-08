#!/bin/sh
# Real test that a context walk rooted at the home folder reads the home's own
# files and enters none of the folders under it (on macOS those are the
# consent-guarded Music, Pictures, Desktop, Downloads and Library, and
# entering them raised privacy prompts in the name of whoever started Jeden),
# while a folder under the home that is itself the working directory is read.
# The home is a fresh directory under this run's report folder; the real home
# is never walked. Every command and its output go to the run's report.txt.
#
# Usage: JEDEN=<path to the jeden binary under test> tests/context/home_walk.sh
set -eu
cd "$(dirname "$0")/../.."
BIN=${JEDEN:?JEDEN names the jeden binary under test, such as target/debug/jeden}
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/context/$RUN"
REPORT="$ROOT/report.txt"
HOME_DIR="$ROOT/home"
mkdir -p "$HOME_DIR/Music"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "binary: $BIN" >>"$REPORT"

printf '# Home notes\n\nquokkahomeword lives directly in the home.\n' >"$HOME_DIR/NOTES.md"
printf '# Music notes\n\nquokkamusicword lives in a folder under the home.\n' >"$HOME_DIR/Music/notes.md"

fail() {
  echo "FAIL: $1" | tee -a "$REPORT"
  false
}
run() {
  directory=$1
  shift
  printf '$ (cd %s) jeden %s\n' "$directory" "$*" >>"$REPORT"
  if ! out=$(cd "$directory" && HOME="$HOME_DIR" "$BIN" "$@" </dev/null); then
    fail "jeden $* failed in $directory"
  fi
  printf 'stdout: %s\n\n' "$out" >>"$REPORT"
}
holds() {
  case "$out" in
    *"$1"*) echo "ok: answer names $1" >>"$REPORT" ;;
    *) fail "expected the answer to name $1" ;;
  esac
}
lacks() {
  case "$out" in
    *"$1"*) fail "the answer names $1, which a home-rooted walk must not open" ;;
    *) echo "ok: answer does not name $1" >>"$REPORT" ;;
  esac
}

run "$HOME_DIR" context recommend quokkahomeword --source files --json
holds "NOTES.md"

if [ "$(uname -s)" = Darwin ]; then
  run "$HOME_DIR" context recommend quokkamusicword --source files --json
  lacks "Music/notes.md"
fi

run "$HOME_DIR/Music" context recommend quokkamusicword --source files --json
holds "notes.md"

touch "$ROOT/passed"
echo "PASS" >>"$REPORT"
echo "PASS: $REPORT"
