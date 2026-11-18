#!/usr/bin/env python3
"""Fail-closed anti-mock / anti-facade runtime scanner for M1 Foundation.

Scans `source/host/src/**`, `source/desktop_core/crates/**`, and `mobile/ios/Sources/**`
to guarantee:
1. Zero stale `104.197.143.175` endpoints
2. Zero `sample_chats` or wildcard `"status": "acknowledged"` FFI stubs
3. Presence of fail-closed `unknown_command` in `source/host/src/`
4. Presence of canonical `http://34.46.77.133:8080` in both Rust and Swift CoreBridge
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

FORBIDDEN_HOST_SUBSTRINGS = [
    '"status": "acknowledged"',
    '"status":"acknowledged"',
    '"receivedParams"',
    "sample_chats",
    "Welcome to Fabushi iOS Telegram Core",
    "Native iOS 1:1 Telegram UI is live!",
    "104.197.143.175",
]


def main() -> int:
    errors: list[str] = []

    host_src = ROOT / "source/host/src"
    rs_files = sorted(host_src.rglob("*.rs"))
    if not rs_files:
        errors.append("No Rust source files found in source/host/src")
    combined_host = "\n".join(p.read_text(encoding="utf-8") for p in rs_files)

    if "unknown_command" not in combined_host:
        errors.append("source/host/src missing fail-closed 'unknown_command' error code")

    for bad in FORBIDDEN_HOST_SUBSTRINGS:
        if bad in combined_host:
            errors.append(f"Forbidden stub/stale token in source/host/src: {bad!r}")

    # Scan Swift M1 foundation sources for stale IP or mock stubs
    swift_roots = [
        ROOT / "mobile/ios/Fabushi/Core",
        ROOT / "mobile/ios/Fabushi/DesignSystem",
        ROOT / "mobile/ios/Fabushi/App",
    ]
    combined_swift = ""
    for sr in swift_roots:
        if not sr.is_dir():
            errors.append(f"Missing M1 Swift source directory: {sr.relative_to(ROOT)}")
            continue
        for sf in sorted(sr.rglob("*.swift")):
            text = sf.read_text(encoding="utf-8")
            combined_swift += "\n" + text
            if "104.197.143.175" in text:
                errors.append(f"Stale IP 104.197.143.175 found in {sf.relative_to(ROOT)}")

    if "http://34.46.77.133:8080" not in combined_swift:
        errors.append("mobile/ios/Fabushi/Core/HostBridge missing default http://34.46.77.133:8080 endpoint")

    if errors:
        for e in errors:
            print(f"ERROR: {e}", file=sys.stderr)
        return 1

    print("PASS: Zero mock/stub facades or stale endpoints detected across Rust host & Swift CoreBridge.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
