#!/bin/sh
# Real test of the operator ask register through the jeden binary, in an
# isolated HOME and session root under target/real-tests/asks/<run>:
#
# - asks already recorded on tasks of several sessions, with the same words
#   (apart from case, spacing and closing punctuation), become one register
#   ask with every place it was asked, and an answered one keeps its answer;
# - `jeden todo list` shows that ask once per task with its id, how often it
#   was asked and the `jeden asks answer` command;
# - `jeden asks show` lists every place;
# - `jeden asks answer` records the answer once and hands it to every waiting
#   task in every session: each completion.json on disk is read back;
# - every refusal of `jeden asks` and the todo answer refusal for an ask
#   already answered elsewhere, with their exit status and exact sentence.
#
# Every command, its exit status and output go to the run's report.txt.
# Usage: tests/asks/register.sh   (JEDEN selects the binary, default
#   target/debug/jeden)
set -eu
cd "$(dirname "$0")/../.."
BIN=${JEDEN:-target/debug/jeden}
case "$BIN" in /*) ;; *) BIN="$PWD/$BIN" ;; esac
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/asks/$RUN"
REPORT="$ROOT/report.txt"
HOME="$ROOT/home"
JEDEN_SESSION_ROOT="$HOME/.jeden/sessions"
WORKSPACE="$ROOT/workspace"
export HOME JEDEN_SESSION_ROOT
mkdir -p "$JEDEN_SESSION_ROOT" "$WORKSPACE" "$ROOT/sessions"
CWD=$WORKSPACE
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "binary: $BIN" >>"$REPORT"

run() {
  expected=$1
  shift
  set +e
  out=$(cd "$CWD" && "$BIN" "$@" 2>"$ROOT/stderr" </dev/null)
  status=$?
  set -e
  err=$(cat "$ROOT/stderr")
  printf '$ jeden %s\nexit: %s\nstdout: %s\nstderr: %s\n\n' "$*" "$status" "$out" "$err" >>"$REPORT"
  if [ "$status" -ne "$expected" ]; then
    echo "FAIL: jeden $* exited $status, expected $expected: $err" | tee -a "$REPORT" >&2
    exit 1
  fi
}
contains() {
  case "$1" in
    *"$2"*) echo "ok: $3" >>"$REPORT" ;;
    *) echo "FAIL: $3: expected '$2' in: $1" | tee -a "$REPORT" >&2; exit 1 ;;
  esac
}
equals() {
  if [ "$1" = "$2" ]; then
    echo "ok: $3 = $2" >>"$REPORT"
  else
    echo "FAIL: $3: expected '$2', got '$1'" | tee -a "$REPORT" >&2
    exit 1
  fi
}

ASK="Store the deploy token for the example-host release in the vault."
OTHER="Choose the billing plan for example-account."

# session <name> <task-id> <ask> [<task-id> <ask>]...
# records a real session with `jeden todo add` in its own workspace, then
# seeds its completion.json with blocked tasks that asked the operator, as an
# older Jeden recorded them before the register existed.
session() {
  name=$1
  shift
  workspace="$ROOT/workspaces/$name"
  mkdir -p "$workspace"
  CWD=$workspace
  run 0 todo add "Release example-host" --json
  CWD=$WORKSPACE
  dir=$(jq -r '.lastSessionPath' "$workspace/.jeden/mode-state.json")
  echo "$dir" >"$ROOT/sessions/$name"
  tasks=""
  while [ $# -ge 2 ]; do
    tasks="$tasks${tasks:+,}{\"id\":\"$1\",\"requestId\":\"request-1\",\"phase\":\"Work\",\"text\":\"Release example-host\",\"criteria\":[\"The release is published\"],\"kind\":\"work\",\"origin\":\"user\",\"status\":\"blocked\",\"reason\":\"Needs the operator\",\"verification\":null,\"operatorRequest\":{\"askedAt\":\"1700000000\",\"ask\":\"$2\",\"answer\":null}}"
    shift 2
  done
  printf '{"schemaVersion":3,"revision":3,"requests":[{"id":"request-1","prompt":"Release example-host","cwd":"%s","capturedAt":"1700000000","planned":true,"coverageVerified":false}],"tasks":[%s],"blocker":null}\n' "$workspace" "$tasks" >"$dir/completion.json"
}
path() {
  cat "$ROOT/sessions/$1"
}

# 1. An empty machine has nothing in the register.
run 0 asks list
contains "$out" "Nothing has been asked of you." "empty register"
run 0 asks list --json
equals "$(echo "$out" | jq '.asks | length')" "0" "asks in an empty register"
rm -f "$HOME/.jeden/operator-asks.json"

# 2. Three tasks in two sessions asked the same thing; adoption makes one ask.
session a task-a1 "$ASK" task-a2 "$ASK"
session b task-b1 "store the deploy token  for the example-host release in the vault"
session c task-c1 "$OTHER"
run 0 asks list --json
equals "$(echo "$out" | jq '.asks | length')" "2" "register asks after adoption"
# The two wordings of the deploy-token ask are one ask; whichever session was
# adopted first gives it its words.
id=$(echo "$out" | jq -r --arg ask "$OTHER" '.asks[] | select(.ask != $ask) | .id')
words=$(echo "$out" | jq -r --arg id "$id" '.asks[] | select(.id == $id) | .ask')
other=$(echo "$out" | jq -r --arg ask "$OTHER" '.asks[] | select(.ask == $ask) | .id')
equals "$(echo "$out" | jq -r --arg id "$id" '.asks[] | select(.id == $id) | .timesAsked')" "3" "places the deploy token was asked"
equals "$(echo "$out" | jq -r --arg id "$id" '.asks[] | select(.id == $id) | .status')" "waiting" "deploy token status"
equals "$(jq -r --arg id "$id" '.asks[] | select(.id == $id) | .occurrences | length' "$HOME/.jeden/operator-asks.json")" "3" "occurrences on disk"

# 3. The task list names the register ask and its answer command.
run 0 todo list --session "$(path a)"
contains "$out" "Waiting on you: $words (ask $id; asked 3 times since 1700000000)" "todo list shows the register ask"
contains "$out" "Answer with: jeden asks answer $id --text <answer>" "todo list names jeden asks answer"

# 4. show lists every place it was asked.
run 0 asks show "$id"
contains "$out" "for task task-a1 (task blocked)" "show lists task-a1"
contains "$out" "for task task-b1 (task blocked)" "show lists task-b1"

# 5. One answer reaches every waiting task in every session.
run 0 asks answer "$id" --text "It is in the vault as example-deploy-token" --json
equals "$(echo "$out" | jq '[.deliveries[] | select(.outcome == "delivered")] | length')" "3" "tasks the answer reached"
for place in a:task-a1 a:task-a2 b:task-b1; do
  file="$(path "${place%%:*}")/completion.json"
  task=${place#*:}
  equals "$(jq -r --arg task "$task" '.tasks[] | select(.id == $task) | .status' "$file")" "pending" "$place status on disk"
  equals "$(jq -r --arg task "$task" '.tasks[] | select(.id == $task) | .operatorRequest.answer.text' "$file")" "It is in the vault as example-deploy-token" "$place answer on disk"
done
equals "$(jq -r --arg id "$id" '.asks[] | select(.id == $id) | .answer.text' "$HOME/.jeden/operator-asks.json")" "It is in the vault as example-deploy-token" "answer in the register on disk"
run 0 asks list
contains "$out" "Your answer (" "list shows the answer"

# 6. Refusals.
run 1 asks answer "$id" --text "again"
contains "$err" "ask $id already holds an answer, given at" "second answer refused"
run 1 asks show no-such-ask
contains "$err" "unknown ask: no-such-ask; jeden asks list shows every recorded ask" "unknown ask refused"
run 1 asks answer no-such-ask --text "x"
contains "$err" "unknown ask: no-such-ask" "answer to an unknown ask refused"
run 2 asks answer "$other"
contains "$err" "answer requires --text with what the ask asked for" "missing --text refused"
run 1 asks answer "$other" --text "   "
contains "$err" "an answer requires nonempty text" "blank answer refused"
run 2 asks show
contains "$err" "show requires an ask id" "show without id refused"
run 2 asks forget "$id"
contains "$err" "unknown asks action: forget" "unknown action refused"
run 2 asks list --session "$(path a)"
contains "$err" "unknown asks option: --session" "unknown option refused"
run 2 asks list extra
contains "$err" "too many arguments: list extra" "extra words refused"

# 7. The other ask answered from its task reaches the register too.
revision=$(jq -r '.revision' "$(path c)/completion.json")
run 0 todo answer task-c1 --session "$(path c)" --revision "$revision" --text "The yearly plan"
equals "$(jq -r --arg id "$other" '.asks[] | select(.id == $id) | .answer.text' "$HOME/.jeden/operator-asks.json")" "The yearly plan" "todo answer recorded in the register"
equals "$(jq -r '.tasks[0].status' "$(path c)/completion.json")" "pending" "task-c1 status on disk"

# 8. A session recorded later asks, in the same words, what the register
# already holds an answer to: answering it again from the task is refused,
# and the task is linked to the answered ask.
session d task-d1 "$OTHER"
revision=$(jq -r '.revision' "$(path d)/completion.json")
run 1 todo answer task-d1 --session "$(path d)" --revision "$revision" --text "Monthly"
contains "$err" "ask $other already holds an answer, given at" "todo answer on an answered register ask refused"
run 0 asks show "$other"
contains "$out" "for task task-d1 (task blocked)" "task-d1 linked to the answered ask"

touch "$ROOT/passed"
echo "PASS" >>"$REPORT"
echo "PASS: $REPORT"
