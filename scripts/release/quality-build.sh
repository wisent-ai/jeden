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
CARGO_VERSION=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])')
[ "$CARGO_VERSION" = "$VERSION" ] || fail "WISENT_VERSION $VERSION does not match Cargo.toml $CARGO_VERSION"
mkdir -p "$OUTPUT_DIR/quality-$PLATFORM"
CARGO_TARGET_DIR="$OUTPUT_DIR/quality-$PLATFORM" JEDEN_BUILD_VERSION="$VERSION" \
  cargo build --locked --release --target "$TARGET" --bin jeden-quality-report
