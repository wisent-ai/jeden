#!/usr/bin/env python3
"""Say whether the model-router bearer reads back the same way twice.

`run-with-stado.sh` injects BRAMA_TOKEN by reading one field of one Skarbiec
item. When that read succeeds intermittently, the symptom moves: an empty read
makes the binary report the credential as missing, and a differing read makes
Brama answer 401 -- two messages for one unstable dependency, neither naming it.

Reads the field several times and prints only lengths and digests, never the
value, so a mismatch is visible without exposing the bearer.
"""
from __future__ import annotations

import hashlib
import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
ITEM = os.environ.get("JEDEN_MODEL_ROUTER_ITEM", "jeden-model-router")
CONSUMER = os.environ.get("JEDEN_SKARBIEC_CONSUMER", "local-operator")
TOKEN_FILE = os.environ.get(
    "JEDEN_SKARBIEC_TOKEN_FILE", str(HOME / ".stado" / "local-operator-skarbiec-token")
)
STADO = os.environ.get("JEDEN_STADO_BIN", str(HOME / ".local" / "bin" / "stado"))
ATTEMPTS = len("....")

environment = dict(os.environ)
environment["WC_SKARBIEC_CONSUMER"] = CONSUMER
environment["WC_SKARBIEC_TOKEN_FILE"] = TOKEN_FILE

print("item:", ITEM, "consumer:", CONSUMER)
for attempt in range(ATTEMPTS):
    done = subprocess.run(
        [STADO, "credentials", "get", "--field", "token", ITEM],
        capture_output=True,
        text=True,
        env=environment,
    )
    value = done.stdout.strip()
    digest = hashlib.sha256(value.encode()).hexdigest()[: len("abcdefgh")] if value else "-"
    detail = "" if value else " detail=" + " ".join((done.stderr or "").split())
    print(
        f"attempt {attempt + len('x')}: exit={done.returncode} "
        f"length={len(value)} digest={digest}{detail}"
    )
