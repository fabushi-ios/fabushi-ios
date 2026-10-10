#!/usr/bin/env bash
set -euo pipefail

platform="${APP_STORE_PLATFORM:-ios}"
package_path="${APP_STORE_PACKAGE:-}"
status_file="${APP_STORE_STATUS_FILE:-}"

if [ "$platform" != "ios" ]; then
  echo "Unsupported App Store platform: $platform" >&2
  exit 2
fi
if [ -z "$package_path" ] || [ -z "$status_file" ]; then
  echo "APP_STORE_PACKAGE and APP_STORE_STATUS_FILE are required." >&2
  exit 2
fi
if [ ! -f "$package_path" ]; then
  echo "App Store package does not exist: $package_path" >&2
  exit 2
fi

mkdir -p "$(dirname "$status_file")"

write_status() {
  local status="$1"
  local reason="$2"
  local uploaded_at="${3:-}"
  {
    echo "status=$status"
    echo "reason=$reason"
    echo "platform=$platform"
    echo "source_sha=${SOURCE_SHA:-${GITHUB_SHA:-}}"
    echo "app_version=${APP_VERSION:-}"
    echo "build_number=${RELEASE_BUILD_NUMBER:-}"
    echo "package=$(basename "$package_path")"
    echo "uploaded_at=$uploaded_at"
  } > "$status_file"
}

for name in APP_STORE_CONNECT_API_KEY_ID APP_STORE_CONNECT_API_ISSUER_ID APP_STORE_CONNECT_API_KEY_BASE64; do
  if [ -z "${!name:-}" ]; then
    write_status failed "missing_${name}"
    echo "$name is required for App Store Connect upload." >&2
    exit 2
  fi
done

api_key_dir="$HOME/.appstoreconnect/private_keys"
api_key_path="$api_key_dir/AuthKey_${APP_STORE_CONNECT_API_KEY_ID}.p8"
mkdir -p "$api_key_dir"
printf '%s' "$APP_STORE_CONNECT_API_KEY_BASE64" | base64 --decode > "$api_key_path"
chmod 600 "$api_key_path"

is_duplicate_upload() {
  grep -Eiq 'ATTRIBUTE.INVALID.DUPLICATE|bundle version must be higher|value that has already been used|previously uploaded version|already been used' "$1"
}

run_altool() {
  local phase="$1"
  shift
  local log_file="$(dirname "$status_file")/ios-${phase}-altool.log"
  set +e
  "$@" 2>&1 | tee "$log_file"
  local code="${PIPESTATUS[0]}"
  set -e
  if [ "$code" -eq 0 ]; then
    return 0
  fi
  if is_duplicate_upload "$log_file"; then
    write_status failed "duplicate_upload_cannot_prove_exact_source"
    echo "App Store Connect reports a duplicate/already-used build; that does not prove this exact source SHA was uploaded." >&2
    return "$code"
  fi
  write_status failed "app_store_connect_${phase}_failed"
  return "$code"
}

run_altool validate \
  xcrun altool --validate-app \
    --type ios \
    --file "$package_path" \
    --apiKey "$APP_STORE_CONNECT_API_KEY_ID" \
    --apiIssuer "$APP_STORE_CONNECT_API_ISSUER_ID"

run_altool upload \
  xcrun altool --upload-app \
    --type ios \
    --file "$package_path" \
    --apiKey "$APP_STORE_CONNECT_API_KEY_ID" \
    --apiIssuer "$APP_STORE_CONNECT_API_ISSUER_ID"

write_status uploaded accepted_by_app_store_connect "$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
