#!/usr/bin/env python3
"""
tests/run_m6_iter3_challenger.py
Compilation, static audit, and execution harness for challenger_m6_iter3_empirical_suite.swift.
Enforces AGENTS.md build hygiene and zero dummy facade mandates.
"""

import glob
import os
import re
import subprocess
import sys
import time


def find_repo_root() -> str:
    cwd = os.path.abspath(os.getcwd())
    candidates = [
        cwd,
        os.path.join(cwd, "fabushi-ios"),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "..")),
        "/Users/bhrum/Documents/ChatGPT/fabushi-ios",
    ]
    for c in candidates:
        if os.path.isdir(c) and os.path.exists(os.path.join(c, "mobile", "ios")):
            return c
    return cwd


def audit_static_integrity(repo_root: str):
    print("\n[Audit 1/2] Checking for constant identity assertions across test suites...")
    target_files = [
        os.path.join(repo_root, "tests", "e2e", "master_e2e_suite.swift"),
        os.path.join(repo_root, "tests", "challenger_m6_iter3_empirical_suite.swift"),
    ]
    identity_pattern = re.compile(
        r"let\s+(\w+)\s*(?::\s*[^=]+)?\s*=\s*(-?\d+(?:\.\d+)?|\"[^\"]*\"|true|false)\s*;?[\s\S]*?assert\w*\(\s*\1\s*,\s*\2\s*\)"
    )

    total_violations = 0
    for path in target_files:
        if not os.path.exists(path):
            continue
        with open(path, "r", encoding="utf-8") as f:
            content = f.read()
        matches = identity_pattern.findall(content)
        if matches:
            print(f" !!! [VIOLATION] Found {len(matches)} constant identity assertions in {os.path.relpath(path, repo_root)}:")
            for m in matches[:5]:
                print(f"     let {m[0]} = {m[1]}")
            total_violations += len(matches)
        else:
            print(f"  ✓ {os.path.relpath(path, repo_root)}: 0 constant identity assertions")

    if total_violations > 0:
        print(f"[FATAL] Static integrity check failed: {total_violations} constant identity violations detected.")
        sys.exit(1)

    print("\n[Audit 2/2] Checking for deprecated dummy facades in master_e2e_suite.swift...")
    master_suite = os.path.join(repo_root, "tests", "e2e", "master_e2e_suite.swift")
    dummy_symbols = [
        "struct SlidingReplayWindow128",
        "struct AgentTurnCardModel",
        "struct ToolCallItem",
    ]
    with open(master_suite, "r", encoding="utf-8") as f:
        master_content = f.read()

    dummy_found = 0
    for dummy in dummy_symbols:
        if dummy in master_content:
            print(f" !!! [VIOLATION] Dummy facade found in master_e2e_suite.swift: '{dummy}'")
            dummy_found += 1
        else:
            print(f"  ✓ Clean: '{dummy}' absent from master_e2e_suite.swift")

    if dummy_found > 0:
        print(f"[FATAL] Dummy facade check failed: {dummy_found} dummy facades present.")
        sys.exit(1)
    print(" >>> All static integrity audits PASSED!")


def main():
    repo_root = find_repo_root()
    print(f"================================================================================")
    print(f" CHALLENGER M6 ITER 3 EXECUTION HARNESS")
    print(f" Repository Root: {repo_root}")
    print(f"================================================================================")

    # 1. Static Audits
    audit_static_integrity(repo_root)

    # 2. Compile Challenger Binary
    swift_suite = os.path.join(repo_root, "tests", "challenger_m6_iter3_empirical_suite.swift")
    target_bin = os.path.join(repo_root, "target", "e2e", "challenger_m6_iter3_runner")
    os.makedirs(os.path.dirname(target_bin), exist_ok=True)

    bridge_sources = sorted(glob.glob(os.path.join(repo_root, "mobile/ios/Fabushi/Bridge/*.swift")))
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
        swift_suite,
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
        target_bin,
    ]

    print(f"\n[Build] Compiling challenger suite with {len(all_sources)} Swift sources...")
    t0 = time.time()
    res = subprocess.run(compile_cmd, cwd=repo_root, capture_output=True, text=True)
    t_compile = time.time() - t0
    if res.returncode != 0:
        print(f"[FATAL] Compilation failed in {t_compile:.2f}s:\n{res.stderr}")
        sys.exit(2)
    print(f"[Build] Compilation successful in {t_compile:.2f}s")

    # 3. Execute Challenger Suite
    print(f"\n[Execute] Running {target_bin}...")
    t_run0 = time.time()
    exec_res = subprocess.run([target_bin], cwd=repo_root)
    t_run = time.time() - t_run0

    # 4. AGENTS.md Build Hygiene Cleanup
    clean_script = os.path.join(repo_root, "scripts", "clean_build_garbage.py")
    if os.path.exists(clean_script):
        print(f"\n[Build Hygiene] Running {clean_script}...")
        c_res = subprocess.run([sys.executable, clean_script], cwd=repo_root, capture_output=True, text=True)
        if c_res.returncode == 0:
            print(c_res.stdout.strip())
        else:
            print(f"[WARN] clean_build_garbage error:\n{c_res.stderr}")

    print(f"\n[Completed] Challenger harness execution finished in {t_run:.2f}s with exit code {exec_res.returncode}")
    sys.exit(exec_res.returncode)


if __name__ == "__main__":
    main()
