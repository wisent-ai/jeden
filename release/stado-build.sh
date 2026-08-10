#!/bin/bash
set -euo pipefail
: "${WISENT_SOURCE_DIR:?WISENT_SOURCE_DIR is required}"
: "${WISENT_OUTPUT_DIR:?WISENT_OUTPUT_DIR is required}"
mkdir -p "$WISENT_OUTPUT_DIR/bin" "$WISENT_OUTPUT_DIR/evidence"
cd "$WISENT_SOURCE_DIR"
cargo build --release --locked "$@"
shift_count=0
for argument in "$@"; do
  if [ "$shift_count" -eq 1 ]; then
    install -m 0755 "target/release/$argument" "$WISENT_OUTPUT_DIR/bin/$argument"
    shift_count=0
  elif [ "$argument" = "--bin" ]; then
    shift_count=1
  fi
done
for binary in "$WISENT_OUTPUT_DIR"/bin/*; do shasum -a 256 "$binary"; done > "$WISENT_OUTPUT_DIR/evidence/DIGESTS"
