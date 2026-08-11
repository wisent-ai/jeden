#!/usr/bin/env bash
# Install the loopback goal model server as a launchd unit before Stado adopts it.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
PLIST="$HOME/Library/LaunchAgents/com.wisent.jeden.goal-model.plist"
LOG_DIR="$HOME/.jeden/logs"
mkdir -p "$(dirname "$PLIST")" "$LOG_DIR"

python3 - "$PLIST" "$HOME" "$ROOT" <<'PY'
import plistlib
import sys
from pathlib import Path

plist = Path(sys.argv[1])
home = Path(sys.argv[2])
root = Path(sys.argv[3])
payload = {
    "Label": "com.wisent.jeden.goal-model",
    "ProgramArguments": [str(root / "jeden-serve-goal-model")],
    "RunAtLoad": True,
    "KeepAlive": True,
    "ThrottleInterval": 30,
    "ProcessType": "Interactive",
    "EnvironmentVariables": {
        "HOME": str(home),
        "PATH": f"{home}/.stado/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
    },
    "StandardOutPath": str(home / ".jeden/logs/goal-model.out.log"),
    "StandardErrorPath": str(home / ".jeden/logs/goal-model.err.log"),
}
with plist.open("wb") as handle:
    plistlib.dump(payload, handle, sort_keys=True)
PY

uid="$(id -u)"
launchctl bootout "gui/$uid/com.wisent.jeden.goal-model" 2>/dev/null || true
launchctl bootstrap "gui/$uid" "$PLIST"
