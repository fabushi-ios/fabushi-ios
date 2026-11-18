#!/usr/bin/env bash
# scripts/build-mahayana-ios.sh
# Production iOS Rust cross-compilation & prebuilt staticlib reuse pipeline.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Capture Xcode platform/arch/config arguments BEFORE sanitizing Xcode SDK env vars
PLATFORM="${1:-${PLATFORM_NAME:-iphonesimulator}}"
ARCH="${2:-${CURRENT_ARCH:-arm64}}"
CONFIG="${CONFIGURATION:-Release}"

REQUIRED_SYMBOLS=(
  "_telegram_app_host_create"
  "_telegram_app_host_create_test"
  "_telegram_app_host_dispatch"
  "_telegram_app_host_free_string"
  "_telegram_app_host_destroy"
  "_telegram_bridge_register_event_callback"
  "_telegram_replay_window_create"
  "_mahayana_app_host_create"
)

verify_staticlib_symbols() {
  local lib_path="$1"
  if [[ ! -s "$lib_path" ]]; then
    echo "[build-mahayana-ios] Error: Static library missing or empty at $lib_path" >&2
    exit 1
  fi

  local nm_bin=""
  if command -v xcrun >/dev/null 2>&1 && xcrun --find nm >/dev/null 2>&1; then
    nm_bin="$(xcrun --find nm)"
  elif command -v llvm-nm >/dev/null 2>&1; then
    nm_bin="$(command -v llvm-nm)"
  elif command -v nm >/dev/null 2>&1; then
    nm_bin="$(command -v nm)"
  fi

  if [[ -n "$nm_bin" ]]; then
    local exported
    exported="$("$nm_bin" -g "$lib_path" 2>/dev/null || true)"
    for sym in "${REQUIRED_SYMBOLS[@]}"; do
      if ! grep -Fq "$sym" <<<"$exported"; then
        echo "[build-mahayana-ios] Error: Required C-ABI symbol '$sym' missing from $lib_path" >&2
        exit 1
      fi
    done
    echo "[build-mahayana-ios] Verified all ${#REQUIRED_SYMBOLS[@]} required C-ABI symbols in $lib_path"
  fi
}

check_prebuilt() {
  local output_dir="$1"
  local prebuilt="$ROOT/build/rust/$output_dir/libmahayana_app_host.a"
  verify_staticlib_symbols "$prebuilt"
  echo "[build-mahayana-ios] Reusing prebuilt staticlib: $prebuilt"
}

# Fast-path for staged CI jobs (Simulator tests / Device archive / Store delivery)
if [[ "${FABUSHI_USE_PREBUILT_RUST:-0}" == "1" || "${FABUSHI_PREBUILT_RUST_HOST:-0}" == "1" ]]; then
  case "$PLATFORM" in
    iphoneos)
      check_prebuilt iphoneos
      ;;
    iphonesimulator)
      check_prebuilt iphonesimulator
      ;;
    all)
      check_prebuilt iphoneos
      check_prebuilt iphonesimulator
      ;;
    *)
      echo "[build-mahayana-ios] Unsupported Apple platform in prebuilt mode: $PLATFORM" >&2
      exit 2
      ;;
  esac
  exit 0
fi

MANIFEST="${MANIFEST:-$ROOT/source/host/Cargo.toml}"
if [[ ! -f "$MANIFEST" ]]; then
  echo "[build-mahayana-ios] Error: Missing iOS-owned host manifest at $MANIFEST" >&2
  exit 2
fi

# Sanitize Xcode build environment variables so host proc-macros & build.rs compile cleanly
unset SDKROOT DEVELOPER_DIR PLATFORM_DISPLAY_NAME PLATFORM_NAME CURRENT_ARCH ARCHS
unset CC CXX CFLAGS CXXFLAGS LDFLAGS AR RANLIB SDK_NAME

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_DEV_INCREMENTAL=false
export CARGO_PROFILE_DEV_STRIP="debuginfo"
export CARGO_PROFILE_TEST_DEBUG=0
export CARGO_PROFILE_TEST_INCREMENTAL=false
export CARGO_PROFILE_TEST_STRIP="debuginfo"
export CARGO_PROFILE_RELEASE_INCREMENTAL=false
export CARGO_PROFILE_RELEASE_STRIP="debuginfo"
export CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS:--Cembed-bitcode=no}"

export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"
export CFLAGS_aarch64_apple_ios="-miphoneos-version-min=${IPHONEOS_DEPLOYMENT_TARGET}"
export CFLAGS_aarch64_apple_ios_sim="-mios-simulator-version-min=${IPHONEOS_DEPLOYMENT_TARGET}"
export CFLAGS_x86_64_apple_ios="-mios-simulator-version-min=${IPHONEOS_DEPLOYMENT_TARGET}"

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

  echo "[build-mahayana-ios] Target: $target | Platform: $output_dir | Profile: $PROFILE_SUBDIR"

  if ! rustup target list --installed | grep -q "^${target}$"; then
    echo "[build-mahayana-ios] Installing rustup target $target..."
    rustup target add "$target"
  fi

  if [[ -n "$CARGO_PROFILE_FLAG" ]]; then
    cargo build --manifest-path "$MANIFEST" "$CARGO_PROFILE_FLAG" --target "$target"
  else
    cargo build --manifest-path "$MANIFEST" --target "$target"
  fi

  local artifact="$CARGO_TARGET_DIR/$target/$PROFILE_SUBDIR/libmahayana_app_host.a"
  if [[ ! -f "$artifact" ]]; then
    echo "[build-mahayana-ios] Error: Build artifact not found at $artifact" >&2
    exit 1
  fi

  mkdir -p "$ROOT/build/rust/$output_dir"
  cp "$artifact" "$ROOT/build/rust/$output_dir/libmahayana_app_host.a"
  verify_staticlib_symbols "$ROOT/build/rust/$output_dir/libmahayana_app_host.a"
  echo "[build-mahayana-ios] Staged static library -> $ROOT/build/rust/$output_dir/libmahayana_app_host.a"
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

if [[ "${SKIP_CLEAN_BUILD_GARBAGE:-0}" != "1" && -f "$ROOT/scripts/clean_build_garbage.py" ]]; then
  echo "[build-mahayana-ios] Running build hygiene garbage collection..."
  python3 "$ROOT/scripts/clean_build_garbage.py"
fi

echo "[build-mahayana-ios] Pipeline completed successfully."
