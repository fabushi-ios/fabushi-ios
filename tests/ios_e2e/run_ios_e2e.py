#!/usr/bin/env python3
"""Canonical macOS-15 Simulator + Loopback `gpui_telegram_server` E2E Runner.

Modes:
  --audit-coverage  : Static & semantic opaque-box audit of `FabushiUITests/Parity/`.
  --verify-harness  : Full harness verification gate:
                        (1) `coverage_auditor` check,
                        (2) Swift 6 `-strict-concurrency=complete` typecheck of all
                            `FabushiUITests` + `Parity/**/*.swift` against Simulator SDK,
                        (3) opportunistic `xcodegen` + `xcodebuild build-for-testing`
                            (hard-blocking when `--require-xcodebuild` is set),
                        (4) live loopback `gpui_telegram_server` startup + multi-peer
                            HTTP/WS/CAS/E2EE `companion_peer.run_self_test()`.
  --tier 1|2|3|4|all: Boots Simulator, builds `Fabushi` + `FabushiUITests`, starts
                      loopback `gpui_telegram_server`, runs `xcodebuild test-without-building`
                      for the selected tier (and optional `--feature FXX`), and verifies
                      `Executed N tests, with 0 failures` (`N > 0`).
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[2]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tests.ios_e2e.companion_peer import run_self_test  # noqa: E402
from tests.ios_e2e.coverage_auditor import audit_parity_suite  # noqa: E402
from tests.ios_e2e.server_manager import (  # noqa: E402
    LoopbackServerInstance,
    ensure_server_binary,
    is_local_desktop_core_upgraded,
    resolve_temp_root,
)


def write_source_env(artifacts_dir: Path) -> dict[str, str]:
    head_sha = subprocess.check_output(
        ["git", "-C", str(REPO_ROOT), "rev-parse", "HEAD"], text=True
    ).strip()
    branch = subprocess.check_output(
        ["git", "-C", str(REPO_ROOT), "rev-parse", "--abbrev-ref", "HEAD"], text=True
    ).strip()
    ts = dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    meta = {"HEAD_SHA": head_sha, "BRANCH": branch, "TIMESTAMP_UTC": ts}
    artifacts_dir.mkdir(parents=True, exist_ok=True)
    lines = [f"{k}={v}" for k, v in meta.items()]
    (artifacts_dir / "source.env").write_text("\n".join(lines) + "\n", encoding="utf-8")
    return meta


def run_swift6_typecheck(parity_root: Path, log_path: Path) -> dict[str, Any]:
    sdk_path = subprocess.check_output(
        ["xcrun", "--sdk", "iphonesimulator", "--show-sdk-path"], text=True
    ).strip()
    platform_path = subprocess.check_output(
        ["xcrun", "--sdk", "iphonesimulator", "--show-sdk-platform-path"], text=True
    ).strip()

    ui_tests_root = REPO_ROOT / "mobile" / "ios" / "FabushiUITests"
    base_files = [
        ui_tests_root / "FabushiUITests.swift",
        ui_tests_root / "GlobalDharmaJourneyUITests.swift",
    ]
    existing_base = [str(p) for p in base_files if p.is_file()]
    parity_files = [str(p) for p in sorted(parity_root.rglob("*.swift"))]

    cmd = [
        "xcrun",
        "--sdk",
        "iphonesimulator",
        "swiftc",
        "-typecheck",
        "-target",
        "arm64-apple-ios17.0-simulator",
        "-sdk",
        sdk_path,
        "-F",
        f"{platform_path}/Developer/Library/Frameworks",
        "-I",
        f"{platform_path}/Developer/usr/lib",
        "-L",
        f"{platform_path}/Developer/usr/lib",
        "-swift-version",
        "6",
        "-strict-concurrency=complete",
        "-parse-as-library",
        "-module-name",
        "FabushiUITests",
        *existing_base,
        *parity_files,
    ]

    proc = subprocess.run(cmd, capture_output=True, text=True)
    combined = (
        f"$ {' '.join(cmd)}\n"
        f"exit_code={proc.returncode}\n"
        f"--- stdout ---\n{proc.stdout}\n"
        f"--- stderr ---\n{proc.stderr}\n"
    )
    log_path.parent.mkdir(parents=True, exist_ok=True)
    log_path.write_text(combined, encoding="utf-8")
    if proc.returncode != 0:
        raise RuntimeError(
            f"Swift 6 strict-concurrency typecheck failed (exit {proc.returncode}):\n{proc.stderr}"
        )
    return {
        "ok": True,
        "swift_files_checked": len(existing_base) + len(parity_files),
        "sdk_path": sdk_path,
        "platform_path": platform_path,
    }


def select_simulator_udid() -> tuple[str, str]:
    raw = subprocess.check_output(
        ["xcrun", "simctl", "list", "devices", "available", "-j"], text=True
    )
    data = json.loads(raw)
    candidates: list[tuple[str, str, str]] = []
    for runtime_id, devices in data.get("devices", {}).items():
        if "iOS" not in runtime_id:
            continue
        for dev in devices:
            if not dev.get("isAvailable", False):
                continue
            name = str(dev.get("name", ""))
            udid = str(dev.get("udid", ""))
            if name.startswith("iPhone") and udid:
                candidates.append((runtime_id, name, udid))
    if not candidates:
        raise RuntimeError("No available iPhone Simulator found via `xcrun simctl`")

    preferred_names = ["iPhone 16 Pro", "iPhone 16", "iPhone 15 Pro", "iPhone 15"]
    for pref in preferred_names:
        for _, name, udid in sorted(candidates, reverse=True):
            if name == pref:
                return udid, name
    _, name, udid = sorted(candidates, reverse=True)[0]
    return udid, name


def boot_simulator(udid: str) -> None:
    subprocess.run(["xcrun", "simctl", "boot", udid], check=False)
    subprocess.run(["xcrun", "simctl", "bootstatus", udid, "-b"], check=True)


def run_opportunistic_xcodebuild(
    artifacts_dir: Path, require_xcodebuild: bool
) -> dict[str, Any]:
    xcodegen_bin = shutil.which("xcodegen")
    if not xcodegen_bin:
        if require_xcodebuild:
            raise RuntimeError("xcodegen not installed while --require-xcodebuild was set")
        return {"attempted": False, "reason": "xcodegen_not_installed"}

    udid, sim_name = select_simulator_udid()
    derived_data = resolve_temp_root() / "gpui-e2e-derived-data"
    log_file = artifacts_dir / "xcodebuild_build_for_testing.log"

    gen_proc = subprocess.run(
        [xcodegen_bin, "generate"],
        cwd=str(REPO_ROOT / "mobile" / "ios"),
        capture_output=True,
        text=True,
    )
    if gen_proc.returncode != 0:
        log_file.write_text(gen_proc.stdout + "\n" + gen_proc.stderr, encoding="utf-8")
        if require_xcodebuild:
            raise RuntimeError(f"xcodegen generate failed:\n{gen_proc.stderr}")
        return {"attempted": True, "ok": False, "stage": "xcodegen"}

    build_cmd = [
        "xcodebuild",
        "build-for-testing",
        "-project",
        str(REPO_ROOT / "mobile" / "ios" / "Fabushi.xcodeproj"),
        "-scheme",
        "Fabushi",
        "-destination",
        f"platform=iOS Simulator,id={udid}",
        "-derivedDataPath",
        str(derived_data),
        "CODE_SIGNING_ALLOWED=NO",
    ]
    b_proc = subprocess.run(build_cmd, capture_output=True, text=True)
    log_file.write_text(
        (b_proc.stdout or "")[-32768:] + "\n" + (b_proc.stderr or "")[-32768:],
        encoding="utf-8",
    )
    if b_proc.returncode != 0:
        if require_xcodebuild:
            raise RuntimeError(
                f"xcodebuild build-for-testing failed on {sim_name} ({udid}); see {log_file}"
            )
        return {
            "attempted": True,
            "ok": False,
            "stage": "build-for-testing",
            "note": "App target still awaiting M1 compile remediation; FabushiUITests Swift 6 typecheck passed.",
        }
    return {
        "attempted": True,
        "ok": True,
        "simulator": sim_name,
        "udid": udid,
        "derived_data": str(derived_data),
    }


def execute_xctest_tier(
    audit_report: dict[str, Any],
    tier: str,
    feature: str | None,
    artifacts_dir: Path,
) -> dict[str, Any]:
    index: list[dict[str, Any]] = audit_report.get("test_index", [])
    selected: list[dict[str, Any]] = []
    for entry in index:
        if tier != "all" and entry["tier"] != tier:
            continue
        if feature and feature not in entry.get("features", []):
            continue
        selected.append(entry)

    if not selected:
        raise RuntimeError(f"Zero Parity tests matched tier={tier!r} feature={feature!r}")

    udid, sim_name = select_simulator_udid()
    boot_simulator(udid)

    derived_data = resolve_temp_root() / "gpui-e2e-derived-data"
    subprocess.run(
        ["xcodegen", "generate"],
        cwd=str(REPO_ROOT / "mobile" / "ios"),
        check=True,
    )
    subprocess.run(
        [
            "xcodebuild",
            "build-for-testing",
            "-project",
            str(REPO_ROOT / "mobile" / "ios" / "Fabushi.xcodeproj"),
            "-scheme",
            "Fabushi",
            "-destination",
            f"platform=iOS Simulator,id={udid}",
            "-derivedDataPath",
            str(derived_data),
            "CODE_SIGNING_ALLOWED=NO",
        ],
        check=True,
    )

    products_dir = derived_data / "Build" / "Products" / "Debug-iphonesimulator"
    for bundle_name in ("Fabushi.app", "FabushiUITests-Runner.app"):
        bundle_path = products_dir / bundle_name
        if bundle_path.exists():
            subprocess.run(["xcrun", "simctl", "install", udid, str(bundle_path)], check=True)

    server_bin, ws_info = ensure_server_binary(REPO_ROOT)
    xcresult_path = artifacts_dir / f"ParityTier_{tier}.xcresult"
    if xcresult_path.exists():
        shutil.rmtree(xcresult_path, ignore_errors=True)

    if feature:
        only_testing = [
            f"-only-testing:FabushiUITests/{item['class']}/{item['method']}"
            for item in selected
        ]
    else:
        classes = sorted({item["class"] for item in selected})
        only_testing = [f"-only-testing:FabushiUITests/{cls}" for cls in classes]

    with LoopbackServerInstance(server_bin) as server:
        env = os.environ.copy()
        env["TEST_RUNNER_GPUI_TELEGRAM_SERVER_URL"] = server.base_url
        test_cmd = [
            "xcodebuild",
            "test-without-building",
            "-project",
            str(REPO_ROOT / "mobile" / "ios" / "Fabushi.xcodeproj"),
            "-scheme",
            "Fabushi",
            "-destination",
            f"platform=iOS Simulator,id={udid}",
            "-derivedDataPath",
            str(derived_data),
            "-resultBundlePath",
            str(xcresult_path),
            *only_testing,
        ]
        proc = subprocess.run(test_cmd, env=env, capture_output=True, text=True)
        out_log = artifacts_dir / f"xcodebuild_tier_{tier}.log"
        combined_out = (proc.stdout or "") + "\n" + (proc.stderr or "")
        out_log.write_text(combined_out, encoding="utf-8")
        server.copy_log_to(artifacts_dir / f"server_tier_{tier}.log")

    executed_matches = re.findall(
        r"Executed\s+(\d+)\s+tests?,\s+with\s+(\d+)\s+failures?", combined_out
    )
    total_executed = sum(int(m[0]) for m in executed_matches)
    total_failures = sum(int(m[1]) for m in executed_matches)

    if proc.returncode != 0 or total_executed == 0 or total_failures > 0:
        raise RuntimeError(
            f"XCUITest tier={tier} failed: exit={proc.returncode}, "
            f"executed={total_executed}, failures={total_failures}"
        )

    return {
        "ok": True,
        "tier": tier,
        "feature": feature,
        "simulator": sim_name,
        "udid": udid,
        "server_source_mode": ws_info.source_mode,
        "executed_tests": total_executed,
        "failures": total_failures,
        "xcresult": str(xcresult_path),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="GPUI-Telegram iOS Opaque-Box E2E Orchestrator")
    parser.add_argument("--verify-harness", action="store_true")
    parser.add_argument("--audit-coverage", action="store_true")
    parser.add_argument("--tier", choices=["1", "2", "3", "4", "all"], default=None)
    parser.add_argument("--feature", default=None, help="Optional feature filter F01..F44")
    parser.add_argument("--min-tier1", type=int, default=220)
    parser.add_argument("--min-tier2", type=int, default=0)
    parser.add_argument("--min-tier3", type=int, default=0)
    parser.add_argument("--min-tier4", type=int, default=0)
    parser.add_argument("--min-total", type=int, default=220)
    parser.add_argument("--require-xcodebuild", action="store_true")
    parser.add_argument(
        "--artifacts-dir",
        default=str(resolve_temp_root() / "gpui-e2e-artifacts"),
    )
    args = parser.parse_args(argv)

    artifacts_dir = Path(args.artifacts_dir).resolve()
    artifacts_dir.mkdir(parents=True, exist_ok=True)
    parity_root = REPO_ROOT / "mobile" / "ios" / "FabushiUITests" / "Parity"

    source_meta = write_source_env(artifacts_dir)
    audit_report = audit_parity_suite(
        parity_root=parity_root,
        min_tier1=args.min_tier1,
        min_tier2=args.min_tier2,
        min_tier3=args.min_tier3,
        min_tier4=args.min_tier4,
        min_total=args.min_total,
    )
    coverage_out = {k: v for k, v in audit_report.items() if k != "test_index"}
    coverage_out["indexed_tests_count"] = len(audit_report.get("test_index", []))
    (artifacts_dir / "coverage_audit_report.json").write_text(
        json.dumps(coverage_out, indent=2) + "\n", encoding="utf-8"
    )

    if not audit_report["ok"]:
        print(json.dumps(coverage_out, indent=2), file=sys.stderr)
        return 1

    if args.audit_coverage and not args.verify_harness and not args.tier:
        print(json.dumps(coverage_out, indent=2))
        return 0

    summary: dict[str, Any] = {
        "source": source_meta,
        "coverage_audit": {
            "ok": True,
            "tier_counts": audit_report["tier_counts"],
            "total_tests": audit_report["total_tests"],
        },
    }

    if args.verify_harness:
        typecheck_res = run_swift6_typecheck(
            parity_root, artifacts_dir / "swiftc_typecheck.log"
        )
        summary["swift6_typecheck"] = typecheck_res

        if args.require_xcodebuild or is_local_desktop_core_upgraded(REPO_ROOT):
            summary["xcodebuild_check"] = run_opportunistic_xcodebuild(
                artifacts_dir, require_xcodebuild=args.require_xcodebuild
            )
        else:
            summary["xcodebuild_check"] = {
                "attempted": False,
                "reason": "pre_m1_local_desktop_core_v0_1_0",
            }

        server_bin, ws_info = ensure_server_binary(REPO_ROOT)
        swift_peer_file = parity_root / "Support" / "LoopbackPeerClient.swift"
        with LoopbackServerInstance(server_bin) as server:
            selftest = run_self_test(
                server.base_url, swift_support_file=swift_peer_file
            )
            selftest["server_source_mode"] = ws_info.source_mode
            selftest["server_commit_sha"] = ws_info.commit_sha
            server.copy_log_to(artifacts_dir / "loopback_server_selftest.log")

        (artifacts_dir / "server_selftest_report.json").write_text(
            json.dumps(selftest, indent=2) + "\n", encoding="utf-8"
        )
        summary["server_selftest"] = selftest

    if args.tier:
        tier_res = execute_xctest_tier(
            audit_report=audit_report,
            tier=args.tier,
            feature=args.feature,
            artifacts_dir=artifacts_dir,
        )
        summary["xctest_execution"] = tier_res

    summary["ok"] = True
    (artifacts_dir / "harness_verify_summary.json").write_text(
        json.dumps(summary, indent=2) + "\n", encoding="utf-8"
    )
    print(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
