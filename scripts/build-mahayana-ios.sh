#!/usr/bin/env bash
# scripts/build-mahayana-ios.sh
# Production iOS Rust cross-compilation pipeline for source/host -> libmahayana_app_host.a.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Respect Xcode environment variables if passed from XcodeGen preBuildScripts phase
RAW_PLATFORM="${1:-${PLATFORM_NAME:-iphonesimulator}}"
ARCH="${2:-${CURRENT_ARCH:-arm64}}"
CONFIG="${CONFIGURATION:-Release}"

MANIFEST="${MANIFEST:-$ROOT/source/host/Cargo.toml}"
if [[ ! -f "$MANIFEST" ]]; then
  echo "[build-mahayana-ios] Error: Missing canonical iOS Rust host manifest at $MANIFEST" >&2
  exit 2
fi

# Ensure cargo/rustup from ~/.cargo/bin are discoverable when invoked inside Xcode build phases
if [[ -d "$HOME/.cargo/bin" ]]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi

# Prevent Xcode's SDKROOT / PLATFORM_DEVELOPER_BIN_DIR from poisoning host build scripts & proc-macros
unset SDKROOT
unset PLATFORM_DEVELOPER_BIN_DIR
unset DEVELOPER_DIR || true

# Single workspace target/ directory & lean profile flags
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_DEV_INCREMENTAL=false
export CARGO_PROFILE_DEV_STRIP="debuginfo"
export CARGO_PROFILE_TEST_DEBUG=0
export CARGO_PROFILE_TEST_INCREMENTAL=false
export CARGO_PROFILE_TEST_STRIP="debuginfo"
export CARGO_PROFILE_RELEASE_INCREMENTAL=false
export CARGO_PROFILE_RELEASE_STRIP="debuginfo"
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"

CONFIG_LOWER="$(echo "$CONFIG" | tr '[:upper:]' '[:lower:]')"
CARGO_PROFILE_FLAG="--release"
PROFILE_SUBDIR="release"
if [[ "$CONFIG_LOWER" == "debug" && "${FORCE_RELEASE:-1}" != "1" ]]; then
  CARGO_PROFILE_FLAG=""
  PROFILE_SUBDIR="debug"
fi

verify_c_abi_symbols() {
  local lib_path="$1"
  local nm_out
  nm_out="$(nm -gU "$lib_path")"
  for sym in \
    _telegram_app_host_create \
    _telegram_app_host_create_test \
    _telegram_app_host_dispatch \
    _telegram_app_host_free_string \
    _telegram_app_host_destroy \
    _telegram_bridge_register_event_callback \
    _telegram_replay_window_create \
    _mahayana_app_host_create; do
    if ! grep -q "$sym" <<<"$nm_out"; then
      echo "[build-mahayana-ios] Error: Missing required C-ABI symbol $sym in $lib_path" >&2
      exit 1
    fi
  done
}

build_target() {
  local target="$1"
  local output_dir="$2"

  echo "[build-mahayana-ios] Target: $target | Platform: $output_dir | Profile: $PROFILE_SUBDIR"

  if ! rustup target list --installed | grep -q "^${target}$"; then
    echo "[build-mahayana-ios] Installing rustup target $target..."
    rustup target add "$target"
  fi

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

  local artifact="$CARGO_TARGET_DIR/$target/$PROFILE_SUBDIR/libmahayana_app_host.a"
  if [[ ! -f "$artifact" ]]; then
    echo "[build-mahayana-ios] Error: Build artifact not found at $artifact" >&2
    exit 1
  fi

  mkdir -p "$ROOT/build/rust/$output_dir"
  cp "$artifact" "$ROOT/build/rust/$output_dir/libmahayana_app_host.a"
  verify_c_abi_symbols "$ROOT/build/rust/$output_dir/libmahayana_app_host.a"
  echo "[build-mahayana-ios] Verified C-ABI symbols and staged $ROOT/build/rust/$output_dir/libmahayana_app_host.a"
}

case "$RAW_PLATFORM" in
  iphoneos|--device)
    build_target aarch64-apple-ios iphoneos
    ;;
  iphonesimulator|--sim)
    if [[ "$ARCH" == "x86_64" ]]; then
      build_target x86_64-apple-ios iphonesimulator
    else
      build_target aarch64-apple-ios-sim iphonesimulator
    fi
    ;;
  all|--all)
    build_target aarch64-apple-ios iphoneos
    build_target aarch64-apple-ios-sim iphonesimulator
    ;;
  *)
    echo "[build-mahayana-ios] Unsupported Apple platform: $RAW_PLATFORM" >&2
    exit 2
    ;;
esac

if [[ -f "$ROOT/scripts/clean_build_garbage.py" ]]; then
  echo "[build-mahayana-ios] Running build hygiene garbage collection..."
  python3 "$ROOT/scripts/clean_build_garbage.py"
fi

echo "[build-mahayana-ios] Pipeline completed successfully."
