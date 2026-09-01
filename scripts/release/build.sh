#!/bin/sh
set -eu

fail() {
  printf '%s\n' "jeden release: $*" >&2
  exit 1
}

SOURCE_DIR=${WISENT_SOURCE_DIR:?WISENT_SOURCE_DIR is required}
OUTPUT_DIR=${WISENT_OUTPUT_DIR:?WISENT_OUTPUT_DIR is required}
VERSION=${WISENT_VERSION:?WISENT_VERSION is required}
PLATFORM=${WISENT_PLATFORM:?WISENT_PLATFORM is required}

case "$SOURCE_DIR" in /*) ;; *) fail "WISENT_SOURCE_DIR must be absolute" ;; esac
case "$OUTPUT_DIR" in /*) ;; *) fail "WISENT_OUTPUT_DIR must be absolute" ;; esac
case "$VERSION" in *[!0-9A-Za-z.-]*|'') fail "WISENT_VERSION is not a canonical coordinate" ;; esac

case "$PLATFORM" in
  darwin-arm64)
    TARGET=aarch64-apple-darwin
    EXECUTABLE=jeden
    ;;
  linux-amd64)
    TARGET=x86_64-unknown-linux-gnu
    EXECUTABLE=jeden
    ;;
  windows-amd64|x86_64-pc-windows-msvc)
    fail "Windows packaging remains supported by scripts/release/package-windows.sh, but Stado has no Windows runner_platform; refusing to publish unbuilt Windows bytes"
    ;;
  *) fail "unsupported WISENT_PLATFORM: $PLATFORM" ;;
esac

cd "$SOURCE_DIR"
# Read it with awk, not python3 -c 'import tomllib'. tomllib is Python 3.11+ and
# this machine's python3 is 3.10, so the release build of a Rust crate failed on
# the interpreter that happened to be first on PATH. Restricted to [package] so a
# dependency's own version line cannot answer instead.
CARGO_VERSION=$(awk -F'"' '/^\[package\]/{p=1;next} /^\[/{p=0} p && /^version[[:space:]]*=/{print $2; exit}' Cargo.toml)
[ "$CARGO_VERSION" = "$VERSION" ] || fail "WISENT_VERSION $VERSION does not match Cargo.toml $CARGO_VERSION"

TARGET_DIR="$OUTPUT_DIR/cargo-$PLATFORM"
STAGE_DIR="$OUTPUT_DIR/stage-$PLATFORM"
rm -rf "$STAGE_DIR"
mkdir -p "$TARGET_DIR" "$STAGE_DIR/bin" "$STAGE_DIR/receipts"

CARGO_TARGET_DIR="$TARGET_DIR" JEDEN_BUILD_VERSION="$VERSION" \
  cargo build --locked --release --target "$TARGET" --bin jeden
cp "$TARGET_DIR/$TARGET/release/$EXECUTABLE" "$STAGE_DIR/bin/jeden"
chmod 0755 "$STAGE_DIR/bin/jeden"

python3 scripts/release/write-evidence.py \
  --binary "$STAGE_DIR/bin/jeden" \
  --receipts "$STAGE_DIR/receipts" \
  --version "$VERSION" \
  --platform "$PLATFORM"
