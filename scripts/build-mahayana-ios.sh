#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="$ROOT/source/packages/mahayana-rs/Cargo.toml"
PLATFORM="${1:-all}"
ARCH="${2:-arm64}"

if [[ ! -f "$MANIFEST" ]]; then
  echo "Missing iOS-owned Mahayana sources at $MANIFEST" >&2
  echo "fabushi-ios must contain its own runtime sources; external source import is not a supported build fallback." >&2
  exit 2
fi

export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"
export CARGO_TARGET_DIR="$ROOT/build/cargo"

build_target() {
  local target="$1"
  local output="$2"
  rustup target add "$target" >/dev/null
  cargo build     --manifest-path "$MANIFEST"     -p mahayana-app-host-mobile     --release     --target "$target"
  mkdir -p "$ROOT/build/rust/$output"
  cp "$CARGO_TARGET_DIR/$target/release/libmahayana_app_host.a"      "$ROOT/build/rust/$output/libmahayana_app_host.a"
}

case "$PLATFORM" in
  iphoneos)
    build_target aarch64-apple-ios iphoneos
    ;;
  iphonesimulator)
    if [[ "$ARCH" == "x86_64" ]]; then
      build_target x86_64-apple-ios iphonesimulator
    else
      build_target aarch64-apple-ios-sim iphonesimulator
    fi
    ;;
  all)
    build_target aarch64-apple-ios iphoneos
    build_target aarch64-apple-ios-sim iphonesimulator
    ;;
  *)
    echo "Unsupported Apple platform: $PLATFORM" >&2
    exit 2
    ;;
esac
