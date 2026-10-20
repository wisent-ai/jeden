#!/usr/bin/env sh
# Session export and recovery — runnable version of
# docs/walkthrough-session-export.md.
#
# Records a session (offline, so it holds a run_error refusal), then lists,
# shows, exports, searches, resumes, and finally tears its ledger tail to
# demonstrate the recovery contract. Requires: JEDEN_BIN=<path to jeden>, jq.
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

echo "== produce a session (offline refusal is fine — it is still history)"
expect_refusal "$JEDEN" run "Respond exactly: OK"
SID="$("$JEDEN" sessions | sed -n '1p')"
echo "session: $SID"

echo "== 1. show (same JSON document export produces)"
"$JEDEN" show "$SID" | jq -r '"keys: \(keys)", "state: \(.state)", "recoveredTruncatedTail: \(.recoveredTruncatedTail)"'
echo "== a missing id answers with JSON, not a crash"
expect_refusal "$JEDEN" show no-such-session

echo "== 2. export: markdown, file"
"$JEDEN" export "$SID" --markdown | sed -n '1,12p'
"$JEDEN" export "$SID" out.json

echo "== 3. artifacts (empty here)"
expect_refusal "$JEDEN" artifacts "$SID"

echo "== 4. search across sessions"
"$JEDEN" sessions search "BRAMA_URL"

echo "== 5. resume forks a child session (refuses offline at the model boundary)"
expect_refusal "$JEDEN" resume "$SID" "continue"
for d in "$HOME/.jeden/sessions"/*/; do
  case "$d" in *"$SID"*) continue ;; esac
  echo "child ledger event types ($d):"
  jq -r '"  " + .payload.type' "$d/transcript.jsonl"
done

echo "== 6. tear the tail, observe the recovery contract"
P="$HOME/.jeden/sessions/$SID/transcript.jsonl"
# Cut the closing brace off the last record, as an interrupted write leaves it.
sed '$ s/}$//' "$P" > "$P.torn"
mv "$P.torn" "$P"
"$JEDEN" show "$SID" | jq -r '"recoveredTruncatedTail = \(.recoveredTruncatedTail) | events = \(.events | length)"'

echo "== done; evidence in $HOME"
