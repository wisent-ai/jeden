#!/bin/sh
set -eu

[ "$(uname -s)" = "Darwin" ] || exit 0
helper=${1:-target/release/jeden-sandbox-helper}
identity=${JEDEN_CODESIGN_IDENTITY:--}

[ -x "$helper" ] || {
  printf '%s\n' "sandbox helper is missing or not executable: $helper" >&2
  exit 1
}
/usr/bin/codesign --force --sign "$identity" "$helper"
/usr/bin/codesign --verify --strict "$helper"
