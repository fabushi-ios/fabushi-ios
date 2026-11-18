#!/usr/bin/env bash
# Unified Opaque-Box E2E Suite Runner for Fabushi iOS (`TEST_INFRA.md § 3`)
# Supports:
#   --tier 1|2|3|4|all
#   --domain R1|R2|R3|R4|R5|R6|R7|R8
#   --list
#   --compile-only
#   --self-test
#   --require-xcodebuild

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

UPSTREAM_AUTHORITY_REPO="https://github.com/bhrum/gpui-telegram.git"
resolve_upstream_authority_sha() {
  if [[ -n "${FABUSHI_UPSTREAM_SERVER_SHA:-}" ]]; then
    echo "$FABUSHI_UPSTREAM_SERVER_SHA"
    return 0
  fi
  python3 - "$REPO_ROOT/manifests/desktop-main-authority.json" <<'PY'
import json, pathlib, sys
fallback = "b436dcdca2c05584dd522301a3201441fb3b4e0e"
p = pathlib.Path(sys.argv[1])
if p.is_file():
    try:
        data = json.loads(p.read_text(encoding="utf-8"))
        blob = json.dumps(data)
        if "gpui-telegram" in blob:
            for k in ("commitSha", "desktopMainSha", "upstreamSha", "sha", "headSha"):
                v = data.get(k)
                if isinstance(v, str) and len(v.strip()) == 40:
                    print(v.strip())
                    sys.exit(0)
    except Exception:
        pass
print(fallback)
PY
}
UPSTREAM_AUTHORITY_SHA="$(resolve_upstream_authority_sha)"

# Enforce strict disk hygiene (never touch /tmp, never place Cargo target inside $REPO_ROOT
# where scripts/clean_build_garbage.py deletes nested target/ directories).
E2E_TMP_ROOT="${RUNNER_TEMP:-${FABUSHI_E2E_TMP_ROOT:-/home/antigravity/.gemini/jetski/scratch/teamwork_tmp/e2e_runner}}"
mkdir -p \
  "$E2E_TMP_ROOT/tmp" \
  "$E2E_TMP_ROOT/cargo-target" \
  "$E2E_TMP_ROOT/bin" \
  "$E2E_TMP_ROOT/sandboxes" \
  "$E2E_TMP_ROOT/reports"

export TMPDIR="$E2E_TMP_ROOT/tmp"
export CARGO_TARGET_DIR="$E2E_TMP_ROOT/cargo-target"

TIER="all"
DOMAIN_FILTER=""
MODE_LIST=0
MODE_COMPILE_ONLY=0
MODE_SELF_TEST=0
REQUIRE_XCODEBUILD=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tier)
      [[ $# -ge 2 ]] || { echo "ERROR: --tier requires an argument (1|2|3|4|all)" >&2; exit 2; }
      TIER="$2"
      shift 2
      ;;
    --tier=*)
      TIER="${1#*=}"
      shift
      ;;
    --domain)
      [[ $# -ge 2 ]] || { echo "ERROR: --domain requires an argument (R1..R8)" >&2; exit 2; }
      DOMAIN_FILTER="$2"
      shift 2
      ;;
    --domain=*)
      DOMAIN_FILTER="${1#*=}"
      shift
      ;;
    --list)
      MODE_LIST=1
      shift
      ;;
    --compile-only)
      MODE_COMPILE_ONLY=1
      shift
      ;;
    --self-test)
      MODE_SELF_TEST=1
      shift
      ;;
    --require-xcodebuild)
      REQUIRE_XCODEBUILD=1
      shift
      ;;
    -h|--help)
      cat <<'EOF'
Usage: scripts/run_ios_e2e_suite.sh [OPTIONS]

Options:
  --tier 1|2|3|4|all       Select test tier (default: all)
  --domain R1..R8          Optional requirement-domain filter
  --list                   Enumerate and emit JSON + Markdown test counts from source declarations
  --compile-only           Compile Rust parity suite and (on macOS) XcodeGen + Swift 6 strict concurrency check
  --self-test              Build/boot loopback gpui_telegram_server, run Rust parity suite, and Swift 6 check
  --require-xcodebuild     Require full xcodebuild build-for-testing on macOS
EOF
      exit 0
      ;;
    *)
      echo "ERROR: Unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

case "$TIER" in
  1|2|3|4|all) ;;
  *)
    echo "ERROR: Invalid --tier '$TIER' (expected 1|2|3|4|all)" >&2
    exit 2
    ;;
esac

if [[ -n "$DOMAIN_FILTER" ]]; then
  case "$DOMAIN_FILTER" in
    R1|R2|R3|R4|R5|R6|R7|R8) ;;
    *)
      echo "ERROR: Invalid --domain '$DOMAIN_FILTER' (expected R1..R8)" >&2
      exit 2
      ;;
  esac
fi

HAD_CARGO_LOCK=0
[[ -f "$REPO_ROOT/tests/e2e_ios_parity/Cargo.lock" ]] && HAD_CARGO_LOCK=1
HAD_APP_PLIST=0
[[ -f "$REPO_ROOT/mobile/ios/Fabushi/Info.plist" ]] && HAD_APP_PLIST=1
HAD_BROADCAST_PLIST=0
[[ -f "$REPO_ROOT/mobile/ios/FabushiBroadcastUpload/Info.plist" ]] && HAD_BROADCAST_PLIST=1

SERVER_PID=""
cleanup() {
  local exit_code=$?
  if [[ -n "$SERVER_PID" ]]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
    SERVER_PID=""
  fi
  if [[ -d "$REPO_ROOT/mobile/ios/Fabushi.xcodeproj" ]]; then
    rm -rf "$REPO_ROOT/mobile/ios/Fabushi.xcodeproj"
  fi
  if [[ "$HAD_CARGO_LOCK" -eq 0 && -f "$REPO_ROOT/tests/e2e_ios_parity/Cargo.lock" ]]; then
    rm -f "$REPO_ROOT/tests/e2e_ios_parity/Cargo.lock"
  fi
  if [[ "$HAD_APP_PLIST" -eq 0 && -f "$REPO_ROOT/mobile/ios/Fabushi/Info.plist" ]]; then
    rm -f "$REPO_ROOT/mobile/ios/Fabushi/Info.plist"
  fi
  if [[ "$HAD_BROADCAST_PLIST" -eq 0 && -f "$REPO_ROOT/mobile/ios/FabushiBroadcastUpload/Info.plist" ]]; then
    rm -f "$REPO_ROOT/mobile/ios/FabushiBroadcastUpload/Info.plist"
  fi
  return "$exit_code"
}
trap cleanup EXIT INT TERM

# ------------------------------------------------------------------------------
# --list: Compute exact Swift XCUITest + Rust integration test counts from source
# ------------------------------------------------------------------------------
run_list_mode() {
  python3 - "$REPO_ROOT" "$TIER" "$DOMAIN_FILTER" "$E2E_TMP_ROOT/reports/e2e_inventory.json" <<'PY'
import json
import pathlib
import re
import sys

repo_root = pathlib.Path(sys.argv[1])
tier_filter = sys.argv[2]
domain_filter = sys.argv[3]
json_out_path = pathlib.Path(sys.argv[4])

ui_root = repo_root / "mobile" / "ios" / "FabushiUITests"
rust_tests_root = repo_root / "tests" / "e2e_ios_parity" / "tests"

func_re = re.compile(r"^\s*(?:@MainActor\s+)?(?:public\s+|internal\s+)?func\s+(test[A-Za-z0-9_]+)\s*\(")
feature_comment_re = re.compile(r"Feature:\s*(F\d{2})\s*\|\s*Tier:\s*(\d+)")
feature_name_re = re.compile(r"^test_?(F\d{2})_")
domain_file_re = re.compile(r"Tier(\d+)_(R\d+)_")

swift_tests = []
for tier_num in ("1", "2", "3", "4"):
    tier_dir = ui_root / f"Tier{tier_num}"
    if not tier_dir.is_dir():
        continue
    for swift_file in sorted(tier_dir.rglob("*.swift")):
        m_file = domain_file_re.search(swift_file.name)
        file_domain = m_file.group(2) if m_file else "CROSS"
        lines = swift_file.read_text(encoding="utf-8").splitlines()
        pending_feature = None
        for idx, line in enumerate(lines, start=1):
            mc = feature_comment_re.search(line)
            if mc:
                pending_feature = mc.group(1)
            mf = func_re.match(line)
            if mf:
                fn_name = mf.group(1)
                feat = pending_feature
                if not feat:
                    mn = feature_name_re.match(fn_name)
                    if mn:
                        feat = mn.group(1)
                pending_feature = None
                if tier_filter != "all" and tier_filter != tier_num:
                    continue
                if domain_filter and file_domain != domain_filter:
                    continue
                swift_tests.append({
                    "runner": "xcuitest",
                    "tier": int(tier_num),
                    "domain": file_domain,
                    "feature": feat or "MULTI",
                    "file": str(swift_file.relative_to(repo_root)),
                    "line": idx,
                    "name": fn_name,
                })

rust_attr_re = re.compile(r"^\s*#\[(?:tokio::)?test(?:\([^\]]*\))?\]")
rust_fn_re = re.compile(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)\s*\(")
rust_tier_re = re.compile(r"^tier(\d+)_")

rust_tests = []
if rust_tests_root.is_dir():
    for rs_file in sorted(rust_tests_root.glob("*.rs")):
        mt = rust_tier_re.match(rs_file.name)
        rs_tier = mt.group(1) if mt else "1"
        if tier_filter != "all" and tier_filter != rs_tier:
            continue
        lines = rs_file.read_text(encoding="utf-8").splitlines()
        saw_test_attr = False
        pending_domain = None
        pending_feature = None
        for idx, line in enumerate(lines, start=1):
            mc = feature_comment_re.search(line)
            if mc:
                pending_feature = mc.group(1)
            md = re.search(r"Domain:\s*(R\d+)", line)
            if md:
                pending_domain = md.group(1)
            if rust_attr_re.match(line):
                saw_test_attr = True
                continue
            if saw_test_attr:
                mf = rust_fn_re.match(line)
                if mf:
                    fn_name = mf.group(1)
                    dom = pending_domain
                    if not dom:
                        md_fn = re.search(r"_(r[1-8])_", fn_name.lower())
                        dom = md_fn.group(1).upper() if md_fn else "R1_R4"
                    if not domain_filter or dom == domain_filter or dom == "R1_R4":
                        rust_tests.append({
                            "runner": "cargo_test",
                            "tier": int(rs_tier),
                            "domain": dom,
                            "feature": pending_feature or "WIRE",
                            "file": str(rs_file.relative_to(repo_root)),
                            "line": idx,
                            "name": fn_name,
                        })
                    saw_test_attr = False
                    pending_domain = None
                    pending_feature = None
                elif line.strip() and not line.strip().startswith("#[") and not line.strip().startswith("//"):
                    saw_test_attr = False

by_tier_swift = {}
by_domain_swift = {}
by_feature_tier = {}
for item in swift_tests:
    t_key = f"tier{item['tier']}"
    by_tier_swift[t_key] = by_tier_swift.get(t_key, 0) + 1
    d_key = f"{t_key}:{item['domain']}"
    by_domain_swift[d_key] = by_domain_swift.get(d_key, 0) + 1
    f_key = item["feature"]
    if f_key not in by_feature_tier:
        by_feature_tier[f_key] = {"tier1": 0, "tier2": 0, "tier3": 0, "tier4": 0}
    by_feature_tier[f_key][t_key] += 1

by_tier_rust = {}
for item in rust_tests:
    t_key = f"tier{item['tier']}"
    by_tier_rust[t_key] = by_tier_rust.get(t_key, 0) + 1

summary = {
    "filter": {"tier": tier_filter, "domain": domain_filter or "all"},
    "xcuitest_total": len(swift_tests),
    "rust_parity_total": len(rust_tests),
    "combined_total": len(swift_tests) + len(rust_tests),
    "xcuitest_by_tier": by_tier_swift,
    "xcuitest_by_tier_domain": by_domain_swift,
    "rust_parity_by_tier": by_tier_rust,
    "xcuitest_by_feature": dict(sorted(by_feature_tier.items())),
}

json_out_path.write_text(json.dumps({"summary": summary, "xcuitest": swift_tests, "rust_parity": rust_tests}, indent=2), encoding="utf-8")

print("## Fabushi iOS Opaque-Box E2E Test Inventory")
print()
print(f"- Filter: `tier={tier_filter}`, `domain={domain_filter or 'all'}`")
print(f"- Swift `XCUITest` cases: **{len(swift_tests)}**")
print(f"- Rust `e2e_ios_parity` cases: **{len(rust_tests)}**")
print(f"- Combined E2E executable cases: **{len(swift_tests) + len(rust_tests)}**")
print()
print("| Feature | Tier 1 | Tier 2 | Tier 3 | Tier 4 |")
print("|---|---:|---:|---:|---:|")
for feat, counts in sorted(by_feature_tier.items()):
    print(f"| {feat} | {counts['tier1']} | {counts['tier2']} | {counts['tier3']} | {counts['tier4']} |")
print()
print("### Machine-Readable Summary JSON")
print(json.dumps(summary, indent=2))
PY
}

# ------------------------------------------------------------------------------
# Swift 6.0 Strict-Concurrency Compilation & XcodeGen Check (macOS / Darwin)
# ------------------------------------------------------------------------------
resolve_simulator_destination() {
  local udid=""
  udid="$(xcrun simctl list devices available -j | python3 -c '
import json, sys
data = json.load(sys.stdin).get("devices", {})
cands = []
for rt, ds in sorted(data.items(), reverse=True):
    if "iOS" not in rt:
        continue
    for d in ds:
        if not d.get("isAvailable"):
            continue
        name = d.get("name", "")
        if "iPhone 16 Pro" == name:
            cands.append((0, d["udid"]))
        elif "iPhone 16" in name:
            cands.append((1, d["udid"]))
        elif "iPhone" in name:
            cands.append((2, d["udid"]))
cands.sort()
print(cands[0][1] if cands else "")
')"
  if [[ -n "$udid" ]]; then
    echo "platform=iOS Simulator,id=$udid"
  else
    echo "platform=iOS Simulator,name=iPhone 16 Pro"
  fi
}

run_macos_swift_compile_checks() {
  echo "=== [macOS] Verifying unique Swift filenames across mobile/ios/FabushiUITests ==="
  python3 - "$REPO_ROOT/mobile/ios/FabushiUITests" <<'PY'
import pathlib, sys
root = pathlib.Path(sys.argv[1])
seen = {}
dups = []
for p in sorted(root.rglob("*.swift")):
    if p.name in seen:
        dups.append((p.name, str(seen[p.name]), str(p)))
    else:
        seen[p.name] = p
if dups:
    for name, a, b in dups:
        print(f"ERROR: Duplicate Swift basename '{name}': {a} vs {b}", file=sys.stderr)
    sys.exit(1)
print(f"Verified {len(seen)} unique Swift files under {root}")
PY

  echo "=== [macOS] Generating Xcode project via XcodeGen ==="
  xcodegen generate --spec "$REPO_ROOT/mobile/ios/project.yml"
  test -d "$REPO_ROOT/mobile/ios/Fabushi.xcodeproj"

  echo "=== [macOS] Running Swift 6.0 (-strict-concurrency=complete) typecheck on FabushiUITests ==="
  local sdk_path platform_path
  sdk_path="$(xcrun --sdk iphonesimulator --show-sdk-path)"
  platform_path="$(xcrun --sdk iphonesimulator --show-sdk-platform-path)"

  local swift_files=()
  while IFS= read -r line; do
    swift_files+=("$line")
  done < <(find "$REPO_ROOT/mobile/ios/FabushiUITests" -type f -name '*.swift' | sort)

  if [[ "${#swift_files[@]}" -eq 0 ]]; then
    echo "ERROR: No Swift files found under mobile/ios/FabushiUITests" >&2
    exit 1
  fi

  xcrun --sdk iphonesimulator swiftc \
    -typecheck \
    -target arm64-apple-ios17.0-simulator \
    -sdk "$sdk_path" \
    -F "$platform_path/Developer/Library/Frameworks" \
    -F "$platform_path/Developer/Library/PrivateFrameworks" \
    -I "$platform_path/Developer/usr/lib" \
    -L "$platform_path/Developer/usr/lib" \
    -swift-version 6 \
    -strict-concurrency=complete \
    -parse-as-library \
    -module-name FabushiUITests \
    "${swift_files[@]}"

  echo "=== [macOS] Swift 6.0 strict-concurrency typecheck PASSED (${#swift_files[@]} files) ==="

  local m1_routes_present=0
  if [[ -f "$REPO_ROOT/source/desktop_core/crates/gpui_telegram_server/src/routes/mod.rs" ]]; then
    m1_routes_present=1
  fi

  if [[ "$REQUIRE_XCODEBUILD" -eq 1 || "$m1_routes_present" -eq 1 ]]; then
    local sim_dest
    sim_dest="$(resolve_simulator_destination)"
    echo "=== [macOS] Running xcodebuild build-for-testing ($sim_dest) ==="
    xcodebuild build-for-testing \
      -project "$REPO_ROOT/mobile/ios/Fabushi.xcodeproj" \
      -scheme Fabushi \
      -destination "$sim_dest" \
      -only-testing:FabushiUITests \
      CODE_SIGNING_ALLOWED=NO
  fi
}

# ------------------------------------------------------------------------------
# Build or resolve gpui_telegram_server binary & boot loopback oracle
# ------------------------------------------------------------------------------
resolve_or_build_server_binary() {
  if [[ -n "${FABUSHI_SERVER_BIN:-}" && -x "${FABUSHI_SERVER_BIN}" ]]; then
    cp -f "$FABUSHI_SERVER_BIN" "$E2E_TMP_ROOT/bin/gpui_telegram_server"
    chmod +x "$E2E_TMP_ROOT/bin/gpui_telegram_server"
    export FABUSHI_SERVER_BIN="$E2E_TMP_ROOT/bin/gpui_telegram_server"
    return 0
  fi

  if [[ -x "$E2E_TMP_ROOT/bin/gpui_telegram_server" ]]; then
    export FABUSHI_SERVER_BIN="$E2E_TMP_ROOT/bin/gpui_telegram_server"
    return 0
  fi

  local built_bin=""
  if [[ -f "$REPO_ROOT/source/desktop_core/crates/gpui_telegram_server/src/routes/mod.rs" ]] && \
     cargo metadata --no-deps --format-version 1 --manifest-path "$REPO_ROOT/source/desktop_core/Cargo.toml" 2>/dev/null | grep -q '"name":"gpui_telegram_server"'; then
    echo "=== Building in-tree gpui_telegram_server from source/desktop_core ==="
    cargo build \
      --manifest-path "$REPO_ROOT/source/desktop_core/Cargo.toml" \
      -p gpui_telegram_server \
      --bin gpui_telegram_server
    built_bin="$CARGO_TARGET_DIR/debug/gpui_telegram_server"
  else
    local upstream_dir="$E2E_TMP_ROOT/upstream-gpui-telegram"
    echo "=== Resolving upstream gpui_telegram_server @ $UPSTREAM_AUTHORITY_SHA ==="
    if [[ ! -d "$upstream_dir/.git" ]] || [[ "$(git -C "$upstream_dir" rev-parse HEAD 2>/dev/null || true)" != "$UPSTREAM_AUTHORITY_SHA" ]]; then
      rm -rf "$upstream_dir"
      mkdir -p "$upstream_dir"
      git -C "$upstream_dir" init -q
      git -C "$upstream_dir" remote add origin "$UPSTREAM_AUTHORITY_REPO"
      git -C "$upstream_dir" fetch --depth 1 origin "$UPSTREAM_AUTHORITY_SHA"
      git -C "$upstream_dir" checkout --detach -q FETCH_HEAD
    fi
    local resolved_sha
    resolved_sha="$(git -C "$upstream_dir" rev-parse HEAD)"
    if [[ "$resolved_sha" != "$UPSTREAM_AUTHORITY_SHA" ]]; then
      echo "ERROR: Upstream SHA mismatch: expected $UPSTREAM_AUTHORITY_SHA, got $resolved_sha" >&2
      exit 1
    fi
    cargo build \
      --manifest-path "$upstream_dir/Cargo.toml" \
      -p gpui_telegram_server \
      --bin gpui_telegram_server
    built_bin="$CARGO_TARGET_DIR/debug/gpui_telegram_server"
  fi

  if [[ ! -x "$built_bin" ]]; then
    echo "ERROR: Compiled gpui_telegram_server binary not found at $built_bin" >&2
    exit 1
  fi

  cp -f "$built_bin" "$E2E_TMP_ROOT/bin/gpui_telegram_server"
  chmod +x "$E2E_TMP_ROOT/bin/gpui_telegram_server"
  export FABUSHI_SERVER_BIN="$E2E_TMP_ROOT/bin/gpui_telegram_server"
}

allocate_free_port() {
  python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
    s.bind(("0.0.0.0", 0))
    print(s.getsockname()[1])
PY
}

boot_loopback_server() {
  resolve_or_build_server_binary
  local port
  port="$(allocate_free_port)"
  local data_dir="$E2E_TMP_ROOT/server_run_${port}_$$"
  mkdir -p "$data_dir/media_cas" "$data_dir/hermes_home"
  local log_file="$data_dir/server.log"

  echo "=== Booting loopback gpui_telegram_server on 127.0.0.1:$port ==="
  (
    cd "$data_dir"
    HOST="127.0.0.1" \
    PORT="$port" \
    SERVER_HOST="127.0.0.1" \
    SERVER_PORT="$port" \
    DATABASE_URL="sqlite://$data_dir/e2e.db" \
    MEDIA_STORAGE_PATH="$data_dir/media_cas" \
    HERMES_HOME="$data_dir/hermes_home" \
    GPUI_OFFLINE_TEST="1" \
    "$FABUSHI_SERVER_BIN" >"$log_file" 2>&1
  ) &
  SERVER_PID=$!

  local ready=0
  for _ in $(seq 1 120); do
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
      echo "ERROR: gpui_telegram_server exited prematurely. Log:" >&2
      cat "$log_file" >&2 || true
      exit 1
    fi
    if curl -fsS "http://127.0.0.1:$port/health" >/dev/null 2>&1 && \
       curl -fsS "http://127.0.0.1:$port/api/v1/status" >/dev/null 2>&1; then
      ready=1
      break
    fi
    sleep 0.25
  done

  if [[ "$ready" -ne 1 ]]; then
    echo "ERROR: Timed out waiting for loopback gpui_telegram_server on 127.0.0.1:$port. Log:" >&2
    cat "$log_file" >&2 || true
    exit 1
  fi

  export FABUSHI_SERVER_URL="http://127.0.0.1:$port"
  export GPUI_TELEGRAM_SERVER_URL="http://127.0.0.1:$port"
  export TEST_RUNNER_FABUSHI_SERVER_URL="http://127.0.0.1:$port"
  export TEST_RUNNER_FABUSHI_E2E_SANDBOX_ROOT="$E2E_TMP_ROOT/sandboxes"
  export TEST_RUNNER_FABUSHI_SANDBOX_ROOT="$E2E_TMP_ROOT/sandboxes"
  echo "=== Loopback server healthy at $FABUSHI_SERVER_URL (PID $SERVER_PID) ==="
}

# ------------------------------------------------------------------------------
# Mode Dispatch
# ------------------------------------------------------------------------------
if [[ "$MODE_LIST" -eq 1 ]]; then
  run_list_mode
  exit 0
fi

if [[ "$MODE_COMPILE_ONLY" -eq 1 ]]; then
  echo "=== Compiling Rust opaque-box parity suite (tests/e2e_ios_parity) ==="
  cargo test --no-run --manifest-path "$REPO_ROOT/tests/e2e_ios_parity/Cargo.toml"
  if [[ "$(uname -s)" == "Darwin" ]]; then
    run_macos_swift_compile_checks
  fi
  echo "=== --compile-only SUCCEEDED ==="
  exit 0
fi

if [[ "$MODE_SELF_TEST" -eq 1 ]]; then
  boot_loopback_server
  echo "=== Running Rust live multi-actor parity & boundary tests against $FABUSHI_SERVER_URL ==="
  cargo test --manifest-path "$REPO_ROOT/tests/e2e_ios_parity/Cargo.toml" -- --nocapture
  if [[ "$(uname -s)" == "Darwin" ]]; then
    run_macos_swift_compile_checks
  fi
  run_list_mode
  echo "=== --self-test SUCCEEDED ==="
  exit 0
fi

# Full Tier Execution Mode (--tier 1|2|3|4|all)
boot_loopback_server

echo "=== Executing Rust parity tests for tier=$TIER ==="
if [[ "$TIER" == "all" ]]; then
  cargo test --manifest-path "$REPO_ROOT/tests/e2e_ios_parity/Cargo.toml" -- --nocapture
else
  RUST_TIER_ARGS=()
  while IFS= read -r rs_path; do
    RUST_TIER_ARGS+=("--test" "$(basename "$rs_path" .rs)")
  done < <(find "$REPO_ROOT/tests/e2e_ios_parity/tests" -maxdepth 1 -type f -name "tier${TIER}_*.rs" | sort)
  if [[ "${#RUST_TIER_ARGS[@]}" -gt 0 ]]; then
    cargo test --manifest-path "$REPO_ROOT/tests/e2e_ios_parity/Cargo.toml" "${RUST_TIER_ARGS[@]}" -- --nocapture
  fi
fi

if [[ "$(uname -s)" == "Darwin" ]]; then
  xcodegen generate --spec "$REPO_ROOT/mobile/ios/project.yml"
  SIM_DEST="$(resolve_simulator_destination)"
  SIM_UDID="$(echo "$SIM_DEST" | sed -n 's/.*id=\([A-F0-9-]*\).*/\1/p')"
  if [[ -n "$SIM_UDID" ]]; then
    xcrun simctl boot "$SIM_UDID" 2>/dev/null || true
    xcrun simctl spawn "$SIM_UDID" defaults write com.apple.iphonesimulator ConnectHardwareKeyboard -bool true || true
  fi

  ONLY_TESTING_ARGS=()
  collect_tier_classes() {
    local t="$1"
    local t_dir="$REPO_ROOT/mobile/ios/FabushiUITests/Tier${t}"
    [[ -d "$t_dir" ]] || return 0
    while IFS= read -r f; do
      local base
      base="$(basename "$f" .swift)"
      if [[ -z "$DOMAIN_FILTER" ]] || [[ "$base" == *"_${DOMAIN_FILTER}_"* ]]; then
        ONLY_TESTING_ARGS+=("-only-testing:FabushiUITests/${base}")
      fi
    done < <(find "$t_dir" -type f -name '*.swift' | sort)
  }

  if [[ "$TIER" == "all" ]]; then
    for t in 1 2 3 4; do
      collect_tier_classes "$t"
    done
  else
    collect_tier_classes "$TIER"
  fi

  if [[ "${#ONLY_TESTING_ARGS[@]}" -eq 0 ]]; then
    echo "ERROR: No FabushiUITests classes matched tier=$TIER domain=${DOMAIN_FILTER:-all}" >&2
    exit 1
  fi

  echo "=== Executing Xcode Simulator XCUITest (${#ONLY_TESTING_ARGS[@]} test classes) ==="
  xcodebuild test \
    -project "$REPO_ROOT/mobile/ios/Fabushi.xcodeproj" \
    -scheme Fabushi \
    -destination "$SIM_DEST" \
    "${ONLY_TESTING_ARGS[@]}" \
    CODE_SIGNING_ALLOWED=NO
fi

run_list_mode
