#!/usr/bin/env python3
"""
scripts/clean_build_garbage.py
Build garbage & stale artifact cleaner conforming to AGENTS.md.

Enforces:
1. Purges target/**/incremental caches across all target profiles and architecture triples.
2. Purges target/**/*.dSYM debug symbol bundles.
3. Purges stale hashed test executable binaries in deps/ and reconciles target/**/.fingerprint.
4. Auto-recycles temporary test scratch directories under target/tmp and build/tmp (never /tmp).
5. Verifies and enforces single workspace target/ directory while preserving build/rust and build/server.
"""

import os
import re
import shutil


def clean_target_dir(target_dir: str) -> int:
    if not os.path.isdir(target_dir):
        return 0
    freed_bytes = 0

    for root, dirs, _ in os.walk(target_dir):
        for d in list(dirs):
            if d == "incremental":
                inc_path = os.path.join(root, d)
                for dp, _, fns in os.walk(inc_path):
                    for f in fns:
                        try:
                            freed_bytes += os.path.getsize(os.path.join(dp, f))
                        except OSError:
                            pass
                shutil.rmtree(inc_path, ignore_errors=True)
                dirs.remove(d)

    profile_dirs = []
    for prof in ("debug", "release", "ci"):
        p = os.path.join(target_dir, prof)
        if os.path.isdir(p):
            profile_dirs.append(p)

    for entry in os.listdir(target_dir):
        sub = os.path.join(target_dir, entry)
        if os.path.isdir(sub) and entry not in ("debug", "release", "ci", "tmp", ".rustc_info.json", "nextest"):
            for prof in ("debug", "release", "ci"):
                p = os.path.join(sub, prof)
                if os.path.isdir(p):
                    profile_dirs.append(p)

    for prof_dir in profile_dirs:
        deps_dir = os.path.join(prof_dir, "deps")
        fp_dir = os.path.join(prof_dir, ".fingerprint")
        if not os.path.isdir(deps_dir):
            continue

        removed_hashes = set()

        for entry in os.listdir(deps_dir):
            full_p = os.path.join(deps_dir, entry)
            if entry.endswith(".dSYM") and os.path.isdir(full_p):
                for dp, _, fns in os.walk(full_p):
                    for f in fns:
                        try:
                            freed_bytes += os.path.getsize(os.path.join(dp, f))
                        except OSError:
                            pass
                shutil.rmtree(full_p, ignore_errors=True)
            elif os.path.isfile(full_p):
                _, ext = os.path.splitext(entry)
                if ext == "" and os.access(full_p, os.X_OK):
                    m = re.search(r"-([0-9a-f]{16})$", entry)
                    if m:
                        removed_hashes.add(m.group(1))
                        try:
                            freed_bytes += os.path.getsize(full_p)
                            os.remove(full_p)
                        except OSError:
                            pass

        if os.path.isdir(fp_dir):
            existing_dep_hashes = set()
            for entry in os.listdir(deps_dir):
                m = re.search(r"-([0-9a-f]{16})\.(rlib|rmeta|dylib|so|a)$", entry)
                if m:
                    existing_dep_hashes.add(m.group(1))

            for fp_entry in os.listdir(fp_dir):
                m = re.search(r"-([0-9a-f]{16})$", fp_entry)
                if not m:
                    continue
                h = m.group(1)
                fp_path = os.path.join(fp_dir, fp_entry)
                stem = fp_entry.rsplit("-", 1)[0].replace("-", "_")
                has_unhashed_artifact = (
                    os.path.exists(os.path.join(deps_dir, f"lib{stem}.rlib"))
                    or os.path.exists(os.path.join(deps_dir, f"lib{stem}.dylib"))
                    or os.path.exists(os.path.join(deps_dir, f"lib{stem}.so"))
                    or os.path.exists(os.path.join(deps_dir, f"lib{stem}.a"))
                )
                if not has_unhashed_artifact and (
                    h in removed_hashes
                    or (
                        h not in existing_dep_hashes
                        and not os.path.isdir(os.path.join(prof_dir, "build", fp_entry))
                    )
                ):
                    shutil.rmtree(fp_path, ignore_errors=True)

    return freed_bytes


def clean_test_artifacts(root_dir: str) -> int:
    freed_bytes = 0
    search_dirs = [
        os.path.join(root_dir, "target", "tmp"),
        os.path.join(root_dir, "build", "tmp"),
    ]
    for s_dir in search_dirs:
        if os.path.isdir(s_dir):
            for dp, _, fns in os.walk(s_dir):
                for f in fns:
                    try:
                        freed_bytes += os.path.getsize(os.path.join(dp, f))
                    except OSError:
                        pass
            shutil.rmtree(s_dir, ignore_errors=True)

    for item in os.listdir(root_dir):
        if item.startswith(".cargo-target-") or item.startswith(".cargo-target"):
            full_p = os.path.join(root_dir, item)
            if os.path.isdir(full_p):
                for dp, _, fns in os.walk(full_p):
                    for f in fns:
                        try:
                            freed_bytes += os.path.getsize(os.path.join(dp, f))
                        except OSError:
                            pass
                shutil.rmtree(full_p, ignore_errors=True)

    canonical_target = os.path.abspath(os.path.join(root_dir, "target"))
    for root, dirs, _ in os.walk(root_dir):
        if ".git" in root or root == canonical_target or root.startswith(canonical_target + os.sep):
            continue
        for d in list(dirs):
            if d == "target":
                redundant_target = os.path.join(root, d)
                if os.path.abspath(redundant_target) != canonical_target:
                    for dp, _, fns in os.walk(redundant_target):
                        for f in fns:
                            try:
                                freed_bytes += os.path.getsize(os.path.join(dp, f))
                            except OSError:
                                pass
                    shutil.rmtree(redundant_target, ignore_errors=True)
                    dirs.remove(d)

    return freed_bytes


def main():
    script_dir = os.path.dirname(os.path.abspath(__file__))
    repo_root = os.path.abspath(os.path.join(script_dir, ".."))
    target_dir = os.path.join(repo_root, "target")
    total_freed = clean_target_dir(target_dir)
    total_freed += clean_test_artifacts(repo_root)
    print(f"[clean_build_garbage] Freed {total_freed / (1024**2):.2f} MB in {repo_root}")


if __name__ == "__main__":
    main()
