#!/bin/bash
set -euo pipefail
: "${WISENT_SOURCE_DIR:?WISENT_SOURCE_DIR is required}"
: "${WISENT_OUTPUT_DIR:?WISENT_OUTPUT_DIR is required}"
# The release worker and product installer sign the declared native stage
# before packaging or installation; a PATH-selected signer is not required.
mkdir -p "$WISENT_OUTPUT_DIR/bin"
cd "$WISENT_SOURCE_DIR"
python3 release/cargo.py cargo build --release --locked "$@"
shift_count=0
for argument in "$@"; do
  if [ "$shift_count" -eq 1 ]; then
    install -m 0755 "target/release/$argument" "$WISENT_OUTPUT_DIR/bin/$argument"
    shift_count=0
  elif [ "$argument" = "--bin" ]; then
    shift_count=1
  fi
done
