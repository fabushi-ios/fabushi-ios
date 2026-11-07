#!/usr/bin/env python3
"""
tests/run_m6_iter2_challenger.py
Compilation and execution harness for challenger_m6_iter2_empirical_suite.swift.
"""

import glob
import os
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


def main():
    repo_root = find_repo_root()
    print(f"[Challenger Harness] Root: {repo_root}")

    swift_suite = os.path.join(repo_root, "tests", "challenger_m6_iter2_empirical_suite.swift")
    target_bin = os.path.join(repo_root, "target", "e2e", "challenger_m6_iter2_runner")
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

    print(f"[Build] Compiling challenger suite with {len(all_sources)} Swift sources...")
    t0 = time.time()
    res = subprocess.run(compile_cmd, cwd=repo_root, capture_output=True, text=True)
    t_compile = time.time() - t0
    if res.returncode != 0:
        print(f"[FATAL] Compilation failed in {t_compile:.2f}s:\n{res.stderr}")
        sys.exit(2)
    print(f"[Build] Compilation successful in {t_compile:.2f}s")

    print(f"[Execute] Running {target_bin}...")
    t_run0 = time.time()
    exec_res = subprocess.run([target_bin], cwd=repo_root)
    t_run = time.time() - t_run0

    # Build hygiene cleanup
    clean_script = os.path.join(repo_root, "scripts", "clean_build_garbage.py")
    if os.path.exists(clean_script):
        print(f"\n[Build Hygiene] Running {clean_script}...")
        c_res = subprocess.run([sys.executable, clean_script], cwd=repo_root, capture_output=True, text=True)
        if c_res.returncode == 0:
            print(c_res.stdout.strip())
        else:
            print(f"[WARN] clean_build_garbage error:\n{c_res.stderr}")

    print(f"[Completed] Challenge execution finished in {t_run:.2f}s with exit code {exec_res.returncode}")
    sys.exit(exec_res.returncode)


if __name__ == "__main__":
    main()
