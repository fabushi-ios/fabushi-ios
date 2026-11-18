#!/usr/bin/env python3
"""
scripts/clean_build_garbage.py
Comprehensive workspace build hygiene & auto-cleanup automation.
Enforces AGENTS.md mandates across local Rust builds, test executions, and CI.

Removes:
1. Compiler build garbage:
   - target/*/incremental caches
   - target/*/deps/*.dSYM debug symbol bundles
   - Stale test executable binaries in target/*/deps
   - Intermediate *.rcgu.o / *.o codegen unit object files in target/*/deps
   - target/tmp temporary build files
   - Preserves all active compiler metadata (.rlib, .rmeta, .d) and .fingerprint caches
     to guarantee dependency graph safety and prevent E0460 / E0463 errors in multi-crate workspaces.
2. Accidental redundant target directories:
   - Any .cargo-target-* in the repository
   - Any nested target/ directories inside sub-crates (enforcing single workspace target)
3. Temporary test fixtures & databases:
   - Ephemeral SQLite databases (*.db, *.db-shm, *.db-wal, *.sqlite*) in /tmp, $TMPDIR, and workspace
   - Temporary Tantivy search index directories (*tantivy*, *search_index*, *fts_index*)
   - Temporary CAS chunked upload staging files (gpui_telegram_cas_*, *.part, *.meta.json) in /tmp and $TMPDIR
"""

import argparse
import glob
import os
import re
import shutil
import sys
import tempfile
import time
from collections import defaultdict
from typing import Dict, List, Optional, Set, Tuple

try:
    import fcntl
except ImportError:
    fcntl = None


def get_path_size(path: str) -> int:
    """Returns size of a file or total recursive size of a directory."""
    if not os.path.exists(path):
        return 0
    if os.path.islink(path) or os.path.isfile(path):
        try:
            return os.path.getsize(path)
        except OSError:
            return 0
    total = 0
    for root, _, files in os.walk(path):
        for f in files:
            p = os.path.join(root, f)
            try:
                if not os.path.islink(p):
                    total += os.path.getsize(p)
            except OSError:
                pass
    return total


def format_bytes(bytes_count: int) -> str:
    """Formats bytes to human-readable string."""
    if bytes_count >= 1024**3:
        return f"{bytes_count / (1024**3):.2f} GB"
    elif bytes_count >= 1024**2:
        return f"{bytes_count / (1024**2):.2f} MB"
    elif bytes_count >= 1024:
        return f"{bytes_count / 1024:.2f} KB"
    return f"{bytes_count} B"


def clean_redundant_workspace_targets(repo_root: str, dry_run: bool = False, verbose: bool = False) -> Tuple[int, int]:
    """Cleans accidental .cargo-target-* and nested target/ directories."""
    freed = 0
    count = 0

    # 1. Accidental .cargo-target-* in workspace
    for match in glob.glob(os.path.join(repo_root, "**", ".cargo-target-*"), recursive=True):
        if os.path.isdir(match):
            sz = get_path_size(match)
            freed += sz
            count += 1
            if verbose:
                print(f"  [target-redundant] {match} ({format_bytes(sz)})")
            if not dry_run:
                shutil.rmtree(match, ignore_errors=True)

    # 2. Nested target/ directories inside sub-crates (must only use single workspace target/)
    canonical_target = os.path.abspath(os.path.join(repo_root, "target"))
    for root, dirs, _ in os.walk(repo_root):
        if "/.git" in root or "/target" in root:
            continue
        for d in dirs:
            if d == "target":
                full_p = os.path.abspath(os.path.join(root, d))
                if full_p != canonical_target and not full_p.startswith(canonical_target):
                    sz = get_path_size(full_p)
                    freed += sz
                    count += 1
                    if verbose:
                        print(f"  [nested-target] {full_p} ({format_bytes(sz)})")
                    if not dry_run:
                        shutil.rmtree(full_p, ignore_errors=True)

    return count, freed


def clean_target_dir(
    target_dir: str,
    dry_run: bool = False,
    verbose: bool = False,
    min_age_seconds: float = 15.0,
    force: bool = False,
) -> Tuple[int, int]:
    """Cleans target/*/incremental, *.dSYM, intermediate *.rcgu.o, and stale test binaries.

    Safe artifact retention policy:
    1. Removes all incremental compilation caches (target/*/incremental)
    2. Removes all macOS debug symbol bundles (target/*/deps/*.dSYM)
    3. Removes all intermediate rustc codegen unit objects (target/*/deps/*.rcgu.o and intermediate *.o)
    4. Removes loose test executable binaries in target/*/deps
    5. Cleans target/tmp
    6. Strictly preserves all active compiler metadata (.rlib, .rmeta, .d) and .fingerprint entries.
       NEVER deletes .rlib/.rmeta by crate stem, which avoids breaking multi-crate workspace
       dependency graphs that require distinct feature hashes of the same stem (e.g. serde, libc, naga).
    """
    if not os.path.isdir(target_dir):
        return 0, 0

    freed = 0
    count = 0
    now = time.time()

    # 1. Remove incremental build directories
    for inc in glob.glob(os.path.join(target_dir, "*", "incremental")):
        if os.path.isdir(inc):
            sz = get_path_size(inc)
            freed += sz
            count += 1
            if verbose:
                print(f"  [incremental] {inc} ({format_bytes(sz)})")
            if not dry_run:
                shutil.rmtree(inc, ignore_errors=True)

    # 2. Profile cleanup (debug, release, test, ci, etc.)
    profiles = [d for d in os.listdir(target_dir) if os.path.isdir(os.path.join(target_dir, d)) and d not in ("tmp", ".rustc_info.json")]

    for profile in profiles:
        prof_dir = os.path.join(target_dir, profile)
        deps_dir = os.path.join(prof_dir, "deps")

        if not os.path.isdir(deps_dir):
            continue

        lock_fd = None
        cargo_lock_path = os.path.join(prof_dir, ".cargo-lock")
        if fcntl is not None and os.path.isfile(cargo_lock_path) and not dry_run:
            try:
                lock_fd = os.open(cargo_lock_path, os.O_RDWR)
                fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except (BlockingIOError, OSError):
                if lock_fd is not None:
                    try:
                        os.close(lock_fd)
                    except OSError:
                        pass
                if verbose:
                    print(f"  [skip-active-build] {profile} (.cargo-lock held by active cargo)")
                continue

        try:
            # A. Remove *.dSYM bundles
            for entry in os.listdir(deps_dir):
                full_p = os.path.join(deps_dir, entry)
                if entry.endswith(".dSYM") and os.path.isdir(full_p):
                    sz = get_path_size(full_p)
                    freed += sz
                    count += 1
                    if verbose:
                        print(f"  [dsym] {entry} ({format_bytes(sz)})")
                    if not dry_run:
                        shutil.rmtree(full_p, ignore_errors=True)

            # B. Clean intermediate codegen unit objects (.rcgu.o and intermediate .o files)
            for entry in os.listdir(deps_dir):
                full_p = os.path.join(deps_dir, entry)
                if not os.path.isfile(full_p) or os.path.islink(full_p):
                    continue

                # Intermediate compiler objects: *.rcgu.o or hashed intermediate *.o
                is_rcgu = entry.endswith(".rcgu.o") or (entry.endswith(".o") and "-" in entry)
                if not is_rcgu:
                    continue

                try:
                    mtime = os.path.getmtime(full_p)
                    sz = os.path.getsize(full_p)
                except OSError:
                    continue

                if not force and (now - mtime) < min_age_seconds:
                    continue

                freed += sz
                count += 1
                if verbose:
                    print(f"  [rcgu-obj] {entry} ({format_bytes(sz)})")
                if not dry_run:
                    try:
                        os.remove(full_p)
                    except OSError:
                        pass

            # C. Remove stale test executable binaries (executable files in deps without extension)
            for entry in os.listdir(deps_dir):
                full_p = os.path.join(deps_dir, entry)
                if not os.path.isfile(full_p) or os.path.islink(full_p):
                    continue

                _, ext = os.path.splitext(entry)
                if ext == "" and os.access(full_p, os.X_OK):
                    try:
                        mtime = os.path.getmtime(full_p)
                        sz = os.path.getsize(full_p)
                    except OSError:
                        continue

                    if not force and (now - mtime) < min_age_seconds:
                        continue

                    freed += sz
                    count += 1
                    if verbose:
                        print(f"  [stale-test-bin] {entry} ({format_bytes(sz)})")
                    if not dry_run:
                        try:
                            os.remove(full_p)
                        except OSError:
                            pass
        finally:
            if lock_fd is not None:
                try:
                    fcntl.flock(lock_fd, fcntl.LOCK_UN)
                except OSError:
                    pass
                try:
                    os.close(lock_fd)
                except OSError:
                    pass

    # 3. Clean target/tmp
    target_tmp = os.path.join(target_dir, "tmp")
    if os.path.isdir(target_tmp):
        for entry in os.listdir(target_tmp):
            full_p = os.path.join(target_tmp, entry)
            sz = get_path_size(full_p)
            freed += sz
            count += 1
            if not dry_run:
                if os.path.isdir(full_p):
                    shutil.rmtree(full_p, ignore_errors=True)
                else:
                    try:
                        os.remove(full_p)
                    except OSError:
                        pass

    return count, freed


def clean_test_sqlite_databases(repo_root: str, dry_run: bool = False, verbose: bool = False) -> Tuple[int, int]:
    """Cleans temporary SQLite databases and WAL/SHM sidecars in workspace and system temp dirs."""
    freed = 0
    count = 0
    db_extensions = (".db", ".db-wal", ".db-shm", ".sqlite", ".sqlite-wal", ".sqlite-shm", ".sqlite3", ".sqlite3-wal", ".sqlite3-shm")

    # 1. Clean test databases in repo workspace (outside target/ and .git)
    for root, dirs, files in os.walk(repo_root):
        if "/.git" in root or "/target" in root:
            continue
        for f in files:
            if f.endswith(db_extensions) and ("test" in f.lower() or "data_test" in root or "server_data" in root):
                p = os.path.join(root, f)
                sz = get_path_size(p)
                freed += sz
                count += 1
                if verbose:
                    print(f"  [workspace-db] {p} ({format_bytes(sz)})")
                if not dry_run:
                    try:
                        os.remove(p)
                    except OSError:
                        pass

    # 2. Clean temporary test databases and SQLite directories in /tmp & $TMPDIR
    temp_dirs = set(["/tmp", tempfile.gettempdir()])
    db_dir_prefixes = ("gpui_test_persist_", "fabushi-transcript-mirror-worker-", "fabushi-ios-blob-db-", "fabushi-ios-blob-recovery-", "fabushi-ios-agent-worker-pool-")

    for t_dir in temp_dirs:
        if not os.path.exists(t_dir):
            continue
        for entry in os.listdir(t_dir):
            full_p = os.path.join(t_dir, entry)
            # Match test DB directories
            if os.path.isdir(full_p) and any(entry.startswith(prefix) for prefix in db_dir_prefixes):
                sz = get_path_size(full_p)
                freed += sz
                count += 1
                if verbose:
                    print(f"  [temp-db-dir] {full_p} ({format_bytes(sz)})")
                if not dry_run:
                    shutil.rmtree(full_p, ignore_errors=True)
            # Match loose test DB files
            elif os.path.isfile(full_p) and entry.endswith(db_extensions):
                if entry.startswith(("gpui_", "telegram_", "fabushi-", "test_")):
                    sz = get_path_size(full_p)
                    freed += sz
                    count += 1
                    if verbose:
                        print(f"  [temp-db-file] {full_p} ({format_bytes(sz)})")
                    if not dry_run:
                        try:
                            os.remove(full_p)
                        except OSError:
                            pass

    return count, freed


def clean_tantivy_indexes(repo_root: str, dry_run: bool = False, verbose: bool = False) -> Tuple[int, int]:
    """Cleans temporary Tantivy search index directories from temp locations and workspace."""
    freed = 0
    count = 0
    temp_dirs = set(["/tmp", tempfile.gettempdir()])

    # Check temp dirs for tantivy indexes
    for t_dir in temp_dirs:
        if not os.path.exists(t_dir):
            continue
        for entry in os.listdir(t_dir):
            full_p = os.path.join(t_dir, entry)
            if not os.path.isdir(full_p):
                continue
            # Name heuristics
            is_tantivy_name = "tantivy" in entry.lower() or "search_index" in entry.lower() or "fts_index" in entry.lower() or entry.startswith("gpui_test_search_")
            # Signature heuristics (meta.json with lock files or tantivy segments)
            is_tantivy_sig = False
            if not is_tantivy_name:
                try:
                    entries = os.listdir(full_p)
                    if "meta.json" in entries and (".tantivy-writer.lock" in entries or ".tantivy-meta.lock" in entries or any(e.endswith(".fast") for e in entries)):
                        is_tantivy_sig = True
                except OSError:
                    pass

            if is_tantivy_name or is_tantivy_sig:
                sz = get_path_size(full_p)
                freed += sz
                count += 1
                if verbose:
                    print(f"  [tantivy-index] {full_p} ({format_bytes(sz)})")
                if not dry_run:
                    shutil.rmtree(full_p, ignore_errors=True)

    # Check workspace (excluding target/ and .git)
    for root, dirs, _ in os.walk(repo_root):
        if "/.git" in root or "/target" in root:
            continue
        for d in dirs:
            if "tantivy" in d.lower() or "search_index" in d.lower() or "fts_index" in d.lower():
                full_p = os.path.join(root, d)
                sz = get_path_size(full_p)
                freed += sz
                count += 1
                if verbose:
                    print(f"  [workspace-tantivy] {full_p} ({format_bytes(sz)})")
                if not dry_run:
                    shutil.rmtree(full_p, ignore_errors=True)

    return count, freed


def clean_chunk_upload_staging(dry_run: bool = False, verbose: bool = False) -> Tuple[int, int]:
    """Cleans temporary media CAS chunked upload staging directories and files."""
    freed = 0
    count = 0
    temp_dirs = set(["/tmp", tempfile.gettempdir()])

    staging_dir_prefixes = ("gpui_telegram_cas_", "gpui_cas_", "fabushi-blob-store-", "forward-snapshot-", "forward-metadata-cas-", "forward-parallel-cas-")

    for t_dir in temp_dirs:
        if not os.path.exists(t_dir):
            continue
        for entry in os.listdir(t_dir):
            full_p = os.path.join(t_dir, entry)
            # CAS staging directory
            if os.path.isdir(full_p) and any(entry.startswith(prefix) for prefix in staging_dir_prefixes):
                sz = get_path_size(full_p)
                freed += sz
                count += 1
                if verbose:
                    print(f"  [chunk-staging-dir] {full_p} ({format_bytes(sz)})")
                if not dry_run:
                    shutil.rmtree(full_p, ignore_errors=True)
            # Loose staging parts or metadata
            elif os.path.isfile(full_p) and entry.endswith((".part", ".meta.json")):
                if entry.startswith(("gpui_", "telegram_", "cas_", "upload_")):
                    sz = get_path_size(full_p)
                    freed += sz
                    count += 1
                    if verbose:
                        print(f"  [chunk-staging-file] {full_p} ({format_bytes(sz)})")
                    if not dry_run:
                        try:
                            os.remove(full_p)
                        except OSError:
                            pass

    return count, freed


def main():
    parser = argparse.ArgumentParser(description="AGENTS.md mandatory local build hygiene & auto-cleanup automation")
    parser.add_argument("--repo-root", default=None, help="Repository root path (defaults to parent of script dir)")
    parser.add_argument("--target-dir", default=None, help="Target build directory (defaults to repo_root/target)")
    parser.add_argument("--dry-run", action="store_true", help="Simulate cleanup and report space savings without deleting files")
    parser.add_argument("--verbose", "-v", action="store_true", help="Print detailed paths of cleaned items")
    parser.add_argument("--force", "-f", action="store_true", help="Bypass 15-second minimum age guard")
    scope = parser.add_mutually_exclusive_group()
    scope.add_argument("--target-only", action="store_true", help="Only clean target/ build artifacts")
    scope.add_argument("--temp-only", action="store_true", help="Only clean temporary test fixtures (SQLite, Tantivy, chunk staging)")
    args = parser.parse_args()

    script_dir = os.path.dirname(os.path.abspath(__file__))
    repo_root = os.path.abspath(args.repo_root) if args.repo_root else os.path.abspath(os.path.join(script_dir, ".."))
    target_dir = os.path.abspath(args.target_dir) if args.target_dir else os.path.join(repo_root, "target")

    prefix = "[clean_build_garbage DRY RUN]" if args.dry_run else "[clean_build_garbage]"
    print(f"{prefix} Initiating workspace build hygiene cleanup...")
    print(f"  Repository Root: {repo_root}")
    print(f"  Target Dir:      {target_dir}")

    total_count = 0
    total_freed = 0

    # 1. Clean compiler target artifacts (unless --temp-only)
    if not args.temp_only:
        # A. Redundant workspace target dirs
        c_red, b_red = clean_redundant_workspace_targets(repo_root, dry_run=args.dry_run, verbose=args.verbose)
        total_count += c_red
        total_freed += b_red
        if c_red > 0 or args.verbose:
            print(f"  Cleaned redundant targets: {c_red} items ({format_bytes(b_red)})")

        # B. Canonical target/ directory
        c_tgt, b_tgt = clean_target_dir(target_dir, dry_run=args.dry_run, verbose=args.verbose, force=args.force)
        total_count += c_tgt
        total_freed += b_tgt
        print(f"  Cleaned compiler target artifacts: {c_tgt} items ({format_bytes(b_tgt)})")

    # 2. Clean temporary test fixtures (unless --target-only)
    if not args.target_only:
        # C. Temporary SQLite databases
        c_db, b_db = clean_test_sqlite_databases(repo_root, dry_run=args.dry_run, verbose=args.verbose)
        total_count += c_db
        total_freed += b_db
        print(f"  Cleaned temporary SQLite test fixtures: {c_db} items ({format_bytes(b_db)})")

        # D. Tantivy search indexes
        c_tan, b_tan = clean_tantivy_indexes(repo_root, dry_run=args.dry_run, verbose=args.verbose)
        total_count += c_tan
        total_freed += b_tan
        print(f"  Cleaned Tantivy search indexes: {c_tan} items ({format_bytes(b_tan)})")

        # E. Media CAS chunked upload staging
        c_cas, b_cas = clean_chunk_upload_staging(dry_run=args.dry_run, verbose=args.verbose)
        total_count += c_cas
        total_freed += b_cas
        print(f"  Cleaned chunk upload staging files: {c_cas} items ({format_bytes(b_cas)})")

    action = "Would free" if args.dry_run else "Freed"
    print(f"{prefix} Done! {action} {format_bytes(total_freed)} ({total_freed} bytes across {total_count} items).")


if __name__ == "__main__":
    main()
