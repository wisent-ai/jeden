#!/bin/sh
set -eu

fail() {
  printf '%s\n' "jeden release quality: $*" >&2
  exit 1
}

SOURCE_DIR=${WISENT_SOURCE_DIR:?WISENT_SOURCE_DIR is required}
OUTPUT_DIR=${WISENT_OUTPUT_DIR:?WISENT_OUTPUT_DIR is required}
VERSION=${WISENT_VERSION:?WISENT_VERSION is required}
PLATFORM=${WISENT_PLATFORM:?WISENT_PLATFORM is required}

case "$PLATFORM" in
  darwin-arm64) TARGET=aarch64-apple-darwin ;;
  linux-amd64) TARGET=x86_64-unknown-linux-gnu ;;
  windows-amd64|x86_64-pc-windows-msvc)
    fail "Stado cannot qualify Windows until a Windows runner_platform exists"
    ;;
  *) fail "unsupported WISENT_PLATFORM: $PLATFORM" ;;
esac

cd "$SOURCE_DIR"
# Same reason as scripts/release/build.sh: tomllib needs Python 3.11+ and the
# python3 first on PATH here is 3.10.
CARGO_VERSION=$(awk -F'"' '/^\[package\]/{p=1;next} /^\[/{p=0} p && /^version[[:space:]]*=/{print $2; exit}' Cargo.toml)
[ "$CARGO_VERSION" = "$VERSION" ] || fail "WISENT_VERSION $VERSION does not match Cargo.toml $CARGO_VERSION"
mkdir -p "$OUTPUT_DIR/quality-$PLATFORM"
CARGO_TARGET_DIR="$OUTPUT_DIR/quality-$PLATFORM" JEDEN_BUILD_VERSION="$VERSION" \
  cargo build --locked --release --target "$TARGET" --bin jeden-quality-report
