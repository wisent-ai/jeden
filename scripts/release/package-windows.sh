#!/bin/sh
set -eu

cat >&2 <<'EOF'
Jeden still supports the x86_64-pc-windows-msvc product output, but it is not a
Stado v1 promoted platform. Stado currently exposes only Darwin ARM64 and Linux
AMD64 runner_platform coordinates; neither can produce the existing MSVC binary.
Refusing to substitute a GNU cross-build or silently omit Windows. Add a Windows
fleet runner coordinate before wiring this target into .wisent-release.json.
EOF
exit 78
