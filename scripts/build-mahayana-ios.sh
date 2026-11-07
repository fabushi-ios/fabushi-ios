#!/usr/bin/env bash
# scripts/build-mahayana-ios.sh
# Production iOS Rust cross-compilation pipeline adhering to AGENTS.md build hygiene.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Respect Xcode environment variables if passed from XcodeGen Run Script phase
PLATFORM="${1:-${PLATFORM_NAME:-iphonesimulator}}"
ARCH="${2:-${CURRENT_ARCH:-arm64}}"
CONFIG="${CONFIGURATION:-Release}"

# Choose manifest: Prefer source/host/Cargo.toml (standalone mobile bridge crate), fallback to monolithic workspace
if [[ -f "$ROOT/source/host/Cargo.toml" ]]; then
  MANIFEST="${MANIFEST:-$ROOT/source/host/Cargo.toml}"
  PACKAGE_NAME=""
else
  MANIFEST="${MANIFEST:-$ROOT/source/packages/mahayana-rs/Cargo.toml}"
  PACKAGE_NAME="mahayana-app-host-mobile"
fi

if [[ ! -f "$MANIFEST" ]]; then
  echo "[build-mahayana-ios] Error: Missing iOS-owned sources at $MANIFEST" >&2
  exit 2
fi

# Mandatory AGENTS.md Build Hygiene Rule 1: Single workspace target/ directory
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

# Mandatory AGENTS.md Build Hygiene Rule 1: Lean profile flags (debug = 0, strip = "debuginfo", incremental = false)
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_DEV_INCREMENTAL=false
export CARGO_PROFILE_DEV_STRIP="debuginfo"
export CARGO_PROFILE_TEST_DEBUG=0
export CARGO_PROFILE_TEST_INCREMENTAL=false
export CARGO_PROFILE_TEST_STRIP="debuginfo"
export CARGO_PROFILE_RELEASE_INCREMENTAL=false
export CARGO_PROFILE_RELEASE_STRIP="debuginfo"

export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"

# Determine profile directory name and cargo build flag
CONFIG_LOWER="$(echo "$CONFIG" | tr '[:upper:]' '[:lower:]')"
CARGO_PROFILE_FLAG="--release"
PROFILE_SUBDIR="release"
if [[ "$CONFIG_LOWER" == "debug" && "${FORCE_RELEASE:-1}" != "1" ]]; then
  CARGO_PROFILE_FLAG=""
  PROFILE_SUBDIR="debug"
fi

build_target() {
  local target="$1"
  local output_dir="$2"

  echo "[build-mahayana-ios] Target: $target | Platform: $output_dir | Config: $PROFILE_SUBDIR"

  # Ensure rustup target is installed
  if ! rustup target list --installed | grep -q "^${target}$"; then
    echo "[build-mahayana-ios] Target $target not found in rustup, installing..."
    rustup target add "$target"
  fi

  echo "[build-mahayana-ios] Executing cargo build (manifest: $MANIFEST, target dir: $CARGO_TARGET_DIR)..."
  if [[ -n "$PACKAGE_NAME" ]]; then
    if [[ -n "$CARGO_PROFILE_FLAG" ]]; then
      cargo build \
        --manifest-path "$MANIFEST" \
        -p "$PACKAGE_NAME" \
        "$CARGO_PROFILE_FLAG" \
        --target "$target"
    else
      cargo build \
        --manifest-path "$MANIFEST" \
        -p "$PACKAGE_NAME" \
        --target "$target"
    fi
  else
    if [[ -n "$CARGO_PROFILE_FLAG" ]]; then
      cargo build \
        --manifest-path "$MANIFEST" \
        "$CARGO_PROFILE_FLAG" \
        --target "$target"
    else
      cargo build \
        --manifest-path "$MANIFEST" \
        --target "$target"
    fi
  fi

  local artifact="$CARGO_TARGET_DIR/$target/$PROFILE_SUBDIR/libmahayana_app_host.a"
  if [[ ! -f "$artifact" ]]; then
    echo "[build-mahayana-ios] Error: Build artifact not found at $artifact" >&2
    exit 1
  fi

  # Route output library to build/rust/$(PLATFORM_NAME)/libmahayana_app_host.a
  mkdir -p "$ROOT/build/rust/$output_dir"
  cp "$artifact" "$ROOT/build/rust/$output_dir/libmahayana_app_host.a"
  echo "[build-mahayana-ios] Successfully copied static library to $ROOT/build/rust/$output_dir/libmahayana_app_host.a"
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
    echo "[build-mahayana-ios] Unsupported Apple platform: $PLATFORM" >&2
    exit 2
    ;;
esac

# Mandatory AGENTS.md Build Hygiene Rule 2: Post-build automated garbage collection
if [[ -f "$ROOT/scripts/clean_build_garbage.py" ]]; then
  echo "[build-mahayana-ios] Running AGENTS.md build hygiene garbage collection..."
  python3 "$ROOT/scripts/clean_build_garbage.py"
fi

echo "[build-mahayana-ios] Pipeline completed successfully."
