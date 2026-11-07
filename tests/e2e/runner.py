#!/usr/bin/env python3
"""
tests/e2e/runner.py
Master Python Test Orchestrator for Milestone 6 E2E Test Suite.
Governed by:
- TEST_INFRA.md §4.1 & §11
- PROJECT.md
- AGENTS.md

Executes all 428 automated test specifications across Tiers 1-4:
- Tier 1: Feature Coverage (185 tests)
- Tier 2: Boundary & Corner Cases (185 tests)
- Tier 3: Cross-Feature Interactions (38 tests)
- Tier 4: Real-World Scenarios (20 tests)
Total: 428 automated tests.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import time


def find_repo_root() -> str:
    # Check script directory ancestor first
    script_root = os.path.abspath(os.path.join(os.path.dirname(__file__), "../.."))
    if os.path.exists(os.path.join(script_root, "source", "host", "Cargo.toml")):
        return script_root
    cwd = os.path.abspath(os.getcwd())
    candidates = [
        os.path.join(cwd, "fabushi-ios"),
        cwd,
        os.path.abspath(os.path.join(cwd, "..")),
        "/Users/bhrum/Documents/ChatGPT/fabushi-ios",
    ]
    for c in candidates:
        if os.path.isdir(c) and os.path.exists(os.path.join(c, "source", "host", "Cargo.toml")):
            return c
    return script_root


def run_clean_build_garbage(repo_root: str) -> bool:
    clean_script = os.path.join(repo_root, "scripts", "clean_build_garbage.py")
    if not os.path.exists(clean_script):
        print(f"[WARN] clean_build_garbage.py not found at {clean_script}")
        return False
    print(f"\n[Build Hygiene] Running {clean_script}...")
    res = subprocess.run([sys.executable, clean_script], cwd=repo_root, capture_output=True, text=True)
    if res.returncode == 0:
        print(res.stdout.strip())
        return True
    else:
        print(f"[ERROR] clean_build_garbage failed with exit code {res.returncode}:\n{res.stderr}")
        return False


import glob

def compile_swift_e2e_suite(repo_root: str, swift_file: str, binary_path: str, verbose: bool = False) -> bool:
    os.makedirs(os.path.dirname(binary_path), exist_ok=True)

    dylib_path = os.path.join(repo_root, "target", "debug", "libmahayana_app_host.dylib")
    if not os.path.exists(dylib_path):
        print("[Build] Building Rust mobile host library...")
        cargo_res = subprocess.run(
            ["cargo", "build", "--manifest-path", "source/host/Cargo.toml"],
            cwd=repo_root,
            capture_output=True,
            text=True,
        )
        if cargo_res.returncode != 0:
            print(f"[FATAL] Cargo build failed:\n{cargo_res.stderr}")
            return False

    bridge_sources = sorted(glob.glob(os.path.join(repo_root, "mobile/ios/Fabushi/Bridge/*.swift")))
    replay_bridge = os.path.join(repo_root, "mobile/ios/Fabushi/Bridge/TelegramReplayWindow.swift")
    if replay_bridge not in bridge_sources and os.path.exists(replay_bridge):
        bridge_sources.append(replay_bridge)
    conversation_sources = sorted(glob.glob(os.path.join(repo_root, "mobile/ios/Fabushi/Presentation/Conversation/*.swift")))
    composer_sources = sorted(glob.glob(os.path.join(repo_root, "mobile/ios/Fabushi/Presentation/Composer/*.swift")))
    media_sources = sorted(glob.glob(os.path.join(repo_root, "mobile/ios/Fabushi/Presentation/MediaViewer/*.swift")))
    profile_sources = sorted(glob.glob(os.path.join(repo_root, "mobile/ios/Fabushi/Presentation/Profile/*.swift")))
    agent_sources = sorted(glob.glob(os.path.join(repo_root, "mobile/ios/Fabushi/Presentation/Agent/*.swift")))
    calls_sources = sorted(glob.glob(os.path.join(repo_root, "mobile/ios/Fabushi/Presentation/Calls/*.swift")))
    groups_sources = sorted(glob.glob(os.path.join(repo_root, "mobile/ios/Fabushi/Presentation/Groups/*.swift")))
    settings_sources = sorted(glob.glob(os.path.join(repo_root, "mobile/ios/Fabushi/Presentation/Settings/*.swift")))
    chats_sources = [os.path.join(repo_root, "mobile/ios/Fabushi/Presentation/Chats/ChatListViewModel.swift")]
    shell_sources = [
        os.path.join(repo_root, "mobile/ios/Fabushi/Presentation/Shell/TelegramTab.swift"),
        os.path.join(repo_root, "mobile/ios/Fabushi/Presentation/Shell/TelegramTabNavigationCoordinator.swift"),
        os.path.join(repo_root, "mobile/ios/Fabushi/Presentation/Shell/TabBarItemBadgeView.swift"),
        os.path.join(repo_root, "mobile/ios/Fabushi/Presentation/Shell/TelegramTabBar.swift"),
    ]

    all_sources = [
        swift_file,
        *bridge_sources,
        *conversation_sources,
        *composer_sources,
        *media_sources,
        *profile_sources,
        *agent_sources,
        *calls_sources,
        *groups_sources,
        *settings_sources,
        *chats_sources,
        *shell_sources,
    ]

    compile_cmd = [
        "swiftc",
        "-parse-as-library",
        "-swift-version", "6",
        "-strict-concurrency=complete",
        "-import-objc-header", os.path.join(repo_root, "mobile/ios/Fabushi/Fabushi-Bridging-Header.h"),
        "-I", os.path.join(repo_root, "source/host/include"),
        "-Ltarget/debug",
        "-lmahayana_app_host",
        "-Xlinker", "-rpath", "-Xlinker", os.path.join(repo_root, "target/debug"),
        *all_sources,
        "-o",
        binary_path,
    ]
    if verbose:
        print(f"[Build] Compiling {len(all_sources)} Swift sources...")
        print(f"[Build] Command: {' '.join(compile_cmd)}")
    start_time = time.time()
    res = subprocess.run(compile_cmd, cwd=repo_root, capture_output=True, text=True)
    elapsed = time.time() - start_time
    if res.returncode != 0:
        print(f"[FATAL] Compilation failed (elapsed: {elapsed:.2f}s):\n{res.stderr}")
        return False
    if verbose:
        print(f"[Build] Compilation successful in {elapsed:.2f}s")
    return True


def main():
    parser = argparse.ArgumentParser(description="Master E2E Test Suite Orchestrator")
    parser.add_argument("--tier", choices=["1", "2", "3", "4", "all"], default="all", help="Target test tier to execute (default: all)")
    parser.add_argument("--clean-after", action="store_true", help="Execute AGENTS.md clean_build_garbage.py post-test")
    parser.add_argument("--filter", type=str, default=None, help="Filter test cases by ID or description substring")
    parser.add_argument("--json-output", type=str, default=None, help="Path to write JSON test report")
    parser.add_argument("--verbose", "-v", action="store_true", help="Enable verbose logging")

    args = parser.parse_args()

    repo_root = find_repo_root()
    swift_suite = os.path.join(repo_root, "tests", "e2e", "master_e2e_suite.swift")
    if not os.path.exists(swift_suite):
        print(f"[ERROR] Master E2E suite not found at {swift_suite}")
        sys.exit(2)

    target_bin = os.path.join(repo_root, "target", "e2e", "master_e2e_runner")

    print("================================================================================")
    print(" MASTER E2E ORCHESTRATOR: Native iOS Telegram Client (fabushi-ios)")
    print(f" Repository Root: {repo_root}")
    print(f" Target Tier:     {args.tier}")
    print(f" Clean After:     {args.clean_after}")
    print("================================================================================")

    # Step 1: Compile master E2E suite
    compile_ok = compile_swift_e2e_suite(repo_root, swift_suite, target_bin, verbose=args.verbose)
    if not compile_ok:
        sys.exit(2)

    # Step 2: Execute compiled runner
    cmd = [target_bin, "--tier", args.tier]
    if args.filter:
        cmd.extend(["--filter", args.filter])
    if args.json_output:
        cmd.extend(["--json", args.json_output])

    print(f"[Execute] Running {target_bin} --tier {args.tier}...")
    run_start = time.time()
    proc = subprocess.run(cmd, cwd=repo_root, capture_output=False)
    run_elapsed = time.time() - run_start

    exit_code = proc.returncode

    # Step 3: Run build hygiene if requested
    if args.clean_after:
        clean_ok = run_clean_build_garbage(repo_root)
        if not clean_ok and exit_code == 0:
            exit_code = 3

    print(f"\n[Orchestrator] Completed in {run_elapsed:.2f}s with exit code {exit_code}")
    sys.exit(exit_code)


if __name__ == "__main__":
    main()
