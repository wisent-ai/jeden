#!/usr/bin/env sh
# Session export and recovery — runnable version of
# docs/walkthrough-session-export.md.
#
# Records a session (offline, so it holds a run_error refusal), then lists,
# shows, exports, searches, resumes, and finally tears its ledger tail to
# demonstrate the recovery contract. Requires: jeden (or JEDEN_BIN=path),
# python3.
set -eu

JEDEN="${JEDEN_BIN:-jeden}"
HOME="$(mktemp -d)"
export HOME
mkdir -p "$HOME/project"
cd "$HOME/project"
unset BRAMA_URL BRAMA_TOKEN WISENT_APP_AGENT_AUTH_SECRET \
      STADO_MODEL_ROUTER_URL STADO_MODEL_ROUTER_TOKEN JEDEN_MODEL || true

echo "== produce a session (offline refusal is fine — it is still history)"
"$JEDEN" run "Respond exactly: OK" || true
SID="$("$JEDEN" sessions | head -1)"
echo "session: $SID"

echo "== 1. show (same JSON document export produces)"
"$JEDEN" show "$SID" | python3 -c "
import json, sys
d = json.load(sys.stdin)
print('keys:', sorted(d))
print('state:', d['state'])
print('recoveredTruncatedTail:', d['recoveredTruncatedTail'])
"
echo "== a missing id answers with JSON, not a crash"
"$JEDEN" show no-such-session || true

echo "== 2. export: markdown, file"
"$JEDEN" export "$SID" --markdown | sed -n '1,12p'
"$JEDEN" export "$SID" out.json

echo "== 3. artifacts (empty here)"
"$JEDEN" artifacts "$SID" || true

echo "== 4. search across sessions"
"$JEDEN" search-sessions "BRAMA_URL"

echo "== 5. resume forks a child session (refuses offline at the model boundary)"
"$JEDEN" resume "$SID" "continue" || true
for d in "$HOME/.jeden/sessions"/*/; do
  case "$d" in *"$SID"*) continue ;; esac
  echo "child ledger event types ($d):"
  python3 -c "import sys,json; [print(' ', json.loads(l)['payload']['type']) for l in open(sys.argv[1])]" \
    "$d/transcript.jsonl"
done

echo "== 6. tear the tail, observe the recovery contract"
P="$HOME/.jeden/sessions/$SID/transcript.jsonl"
python3 -c "
import sys
p = sys.argv[1]
data = open(p, 'rb').read()
open(p, 'wb').write(data[:-30])
" "$P"
"$JEDEN" show "$SID" | python3 -c "
import json, sys
d = json.load(sys.stdin)
print('recoveredTruncatedTail =', d['recoveredTruncatedTail'], '| events =', len(d['events']))
"

echo "== done; evidence in $HOME"
