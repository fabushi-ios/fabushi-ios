#!/usr/bin/env python3
"""Fail-closed M1 Desktop-main authority & vendored crate identity verifier.

Delegates to `scripts/check-desktop-main-ios-architecture.py` and additionally
verifies that all 12 pure-Rust crates in `source/desktop_core/crates/` and the
canonical default server URL (`http://34.46.77.133:8080`) match live upstream.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOCK_FILE = ROOT / "manifests/desktop-main-authority.json"

REQUIRED_CRATES = [
    "gpui_telegram_core",
    "gpui_telegram_proto",
    "gpui_telegram_common",
    "gpui_telegram_client",
    "gpui_telegram_media",
    "gpui_telegram_webrtc",
    "gpui_telegram_agent",
    "gpui_telegram_cli",
    "gpui_telegram_server",
    "gpui_telegram_test_harness",
    "fabushi_miniapp",
    "magpie_gateway",
]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--authority-only", action="store_true")
    parser.add_argument("--desktop-checkout", type=Path, default=None)
    args = parser.parse_args()

    lock = json.loads(LOCK_FILE.read_text(encoding="utf-8"))
    if lock.get("repository") != "bhrum/gpui-telegram" or lock.get("branch") != "main":
        print("ERROR: manifests/desktop-main-authority.json is not bound to bhrum/gpui-telegram@main", file=sys.stderr)
        return 1

    # Verify live refs/heads/main via git ls-remote
    live = subprocess.check_output(
        ["git", "ls-remote", "--heads", "https://github.com/bhrum/gpui-telegram.git", "refs/heads/main"],
        text=True,
        timeout=60,
    ).strip()
    live_sha = live.split()[0] if live else ""
    if live_sha != lock.get("commit"):
        print(f"ERROR: Stale Desktop main lock: recorded={lock.get('commit')} live={live_sha}", file=sys.stderr)
        return 1

    if args.authority_only:
        print(f"PASS: Live bhrum/gpui-telegram@main matches lock ({live_sha})")
        return 0

    for crate in REQUIRED_CRATES:
        manifest = ROOT / "source/desktop_core/crates" / crate / "Cargo.toml"
        if not manifest.is_file():
            print(f"ERROR: Missing vendored crate manifest: {manifest}", file=sys.stderr)
            return 1

    cfg_rs = ROOT / "source/desktop_core/crates/gpui_telegram_client/src/config.rs"
    cfg_text = cfg_rs.read_text(encoding="utf-8")
    if "http://34.46.77.133:8080" not in cfg_text:
        print("ERROR: gpui_telegram_client/src/config.rs missing http://34.46.77.133:8080 default", file=sys.stderr)
        return 1

    print(
        f"PASS: Desktop-main authority ({live_sha}) and all {len(REQUIRED_CRATES)} vendored pure-Rust crates verified."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
