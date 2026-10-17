#!/usr/bin/env python3
"""Build and audit a fail-closed Desktop-main rebaseline candidate.

This script only writes candidate files under RUNNER_TEMP and creates detached Git
blob objects through the GitHub API. It never moves a branch ref or creates a commit.
"""
from __future__ import annotations

import copy
import datetime as dt
import difflib
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import time
import urllib.error
import urllib.request

EXPECTED_DESKTOP_COMMIT = "b6146c9d3d8da83030798334695b27c9e4884f46"
EXPECTED_DESKTOP_TREE = "2355aa184197641c6edc465d03e87a2156d626ad"
EXPECTED_PREVIOUS_COMMIT = "428dfdaaa37de3e5da20496a4e968fe439178c7b"
EXPECTED_PREVIOUS_TREE = "94da6ac98384ccee7a8e0eff92288c46e47a48ff"
EXPECTED_FULL_TREE_SHA256 = "3bf8764b851bbfc08e6087f110b3ebb4a03aa24c3e38539876f0d8be7bbb6c66"
EXPECTED_SELECTED_TREE_SHA256 = "3de22fdd6f9e0a13de97fc913c809d8140b9abdcdebc9168f322cf4292ff34fc"
BASELINE = {
    "fullTrackedCount": 8258,
    "selectedFileCount": 7995,
    "nonselectedCount": 263,
    "responsibilityLedgerCount": 9412,
}
PREVIOUS_COUNTS = {"fullTrackedCount": 8268, "selectedFileCount": 8003, "nonselectedCount": 265}
CURRENT_COUNTS = {"fullTrackedCount": 8272, "selectedFileCount": 8006, "nonselectedCount": 266}
EXPECTED_MODIFIED = {
    "source/electron-main/account/auth-service-contract.ts",
    "source/electron-main/main-production-services.ts",
    "source/product/fabushi/fabushi-account-adapters.ts",
    "source/product/fabushi/fabushi-account-service.contract.test.ts",
    "source/product/fabushi/fabushi-account-service.ts",
}
EXPECTED_ADDED = {
    "source/product/fabushi/fabushi-official-device-registry.contract.test.ts",
    "source/product/fabushi/fabushi-official-device-registry.ts",
    "source/product/fabushi/ws-shim.d.ts",
}
HOST_MAPPED_PATH = "source/host/app/src/main.rs"
UPSTREAM_URL = "https://github.com/bhrumom/fabushi-desktop.git"
TARGET_REPO = "fabushi-ios/fabushi-ios"
API_URL = "https://api.github.com/repos/" + TARGET_REPO


def git(repo: pathlib.Path, *args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=repo, text=True).strip()


def read_json(path: pathlib.Path):
    return json.loads(path.read_text(encoding="utf-8"))


def encode_json(value) -> str:
    return json.dumps(value, indent=2, ensure_ascii=False) + "\n"


def write_candidate(out: pathlib.Path, candidates: dict[str, str], path: str, value):
    content = encode_json(value)
    candidates[path] = content
    target = out / "candidate" / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")
    return content


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def tree_digest(records: dict[str, dict]) -> str:
    stream = "".join(
        f'{path}\0{row["mode"]}\0{row["type"]}\0{row["sha"]}\0'
        f'{row["size"] if row["size"] is not None else "-"}\n'
        for path, row in sorted(records.items())
    )
    return hashlib.sha256(stream.encode("utf-8")).hexdigest()


def selected_records(records: dict[str, dict]) -> dict[str, dict]:
    return {path: row for path, row in records.items()
            if path.startswith("frontend/") or path.startswith("source/")}


def git_tree_records(repo: pathlib.Path, ref: str) -> dict[str, dict]:
    raw = subprocess.check_output(["git", "ls-tree", "-r", "-l", "-z", ref], cwd=repo)
    records = {}
    for line in raw.split(b"\0"):
        if not line:
            continue
        identity, name = line.split(b"\t", 1)
        mode, kind, sha, size = identity.decode("ascii").split()
        path = name.decode("utf-8")
        if path in records:
            raise SystemExit("duplicate tracked path: " + path)
        records[path] = {
            "path": path,
            "mode": mode,
            "type": kind,
            "sha": sha,
            "size": None if size == "-" else int(size),
        }
    return records


def ios_tree_blobs(repo: pathlib.Path) -> dict[str, dict]:
    raw = subprocess.check_output(["git", "ls-tree", "-r", "-z", "HEAD"], cwd=repo)
    result = {}
    for line in raw.split(b"\0"):
        if not line:
            continue
        identity, name = line.split(b"\t", 1)
        mode, kind, sha = identity.decode("ascii").split()
        result[name.decode("utf-8")] = {"mode": mode, "type": kind, "sha": sha}
    return result


def parse_name_status(repo: pathlib.Path, before: str, after: str) -> list[dict]:
    raw = subprocess.check_output(
        ["git", "diff", "--no-ext-diff", "--name-status", "-z",
         before, after, "--", "frontend", "source"],
        cwd=repo,
    )
    tokens = raw.decode("utf-8").split("\0")
    changes = []
    i = 0
    while i < len(tokens) and tokens[i]:
        status = tokens[i]
        i += 1
        if status.startswith(("R", "C")):
            old_path, new_path = tokens[i], tokens[i + 1]
            i += 2
            changes.append({"status": status, "oldPath": old_path, "path": new_path})
        else:
            path = tokens[i]
            i += 1
            changes.append({"status": status, "path": path})
    return changes


def live_main_sha() -> str:
    value = subprocess.check_output(
        ["git", "ls-remote", "--heads", UPSTREAM_URL, "refs/heads/main"],
        stderr=subprocess.PIPE, timeout=60, text=True,
    ).strip().split()
    if len(value) != 2 or value[1] != "refs/heads/main":
        raise SystemExit("cannot resolve canonical Desktop main")
    return value[0]


def create_git_blob(content: str, token: str) -> str:
    payload = json.dumps({"content": content, "encoding": "utf-8"},
                         ensure_ascii=False).encode("utf-8")
    request = urllib.request.Request(
        API_URL + "/git/blobs",
        data=payload,
        method="POST",
        headers={
            "Authorization": "Bearer " + token,
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "Content-Type": "application/json",
            "User-Agent": "fabushi-ios-desktop-main-rebaseline-audit",
        },
    )
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                result = json.loads(response.read().decode("utf-8"))
            sha = result.get("sha")
            if not isinstance(sha, str) or len(sha) != 40:
                raise SystemExit("GitHub create-blob response omitted a valid SHA")
            return sha
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", "replace")
            if attempt < 2 and (exc.code == 429 or exc.code >= 500):
                time.sleep(2 ** attempt)
                continue
            raise SystemExit(f"GitHub create-blob failed ({exc.code}): {detail}") from exc
    raise SystemExit("GitHub create-blob retry budget exhausted")


def create_git_tree(entries: list[dict], base_tree: str, token: str) -> str:
    payload = json.dumps({"base_tree": base_tree, "tree": entries}).encode("utf-8")
    request = urllib.request.Request(
        API_URL + "/git/trees",
        data=payload,
        method="POST",
        headers={
            "Authorization": "Bearer " + token,
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "Content-Type": "application/json",
            "User-Agent": "fabushi-ios-desktop-main-rebaseline-audit",
        },
    )
    with urllib.request.urlopen(request, timeout=120) as response:
        result = json.loads(response.read().decode("utf-8"))
    sha = result.get("sha")
    if not isinstance(sha, str) or len(sha) != 40:
        raise SystemExit("GitHub create-tree response omitted a valid SHA")
    if result.get("truncated"):
        raise SystemExit("GitHub candidate tree response is truncated")
    return sha


def write_identity_files(out: pathlib.Path, records: dict[str, dict],
                         selected: dict[str, dict]) -> dict:
    full_path = out / "full-tree-identities.jsonl"
    selected_path = out / "selected-tree-identities.jsonl"
    full_data = "".join(json.dumps(records[path], sort_keys=True, ensure_ascii=False) + "\n"
                        for path in sorted(records))
    selected_data = "".join(json.dumps(selected[path], sort_keys=True, ensure_ascii=False) + "\n"
                            for path in sorted(selected))
    full_path.write_text(full_data, encoding="utf-8")
    selected_path.write_text(selected_data, encoding="utf-8")
    return {
        "fullTreeIdentityArtifact": {
            "path": full_path.name, "rows": len(records),
            "bytes": len(full_data.encode("utf-8")),
            "sha256": sha256_bytes(full_data.encode("utf-8")),
        },
        "selectedTreeIdentityArtifact": {
            "path": selected_path.name, "rows": len(selected),
            "bytes": len(selected_data.encode("utf-8")),
            "sha256": sha256_bytes(selected_data.encode("utf-8")),
        },
    }


def validate_current_snapshot(root: pathlib.Path, lock: dict, manifest_index: dict,
                              ledger_index: dict, records: dict[str, dict],
                              selected: dict[str, dict]) -> dict:
    errors = []
    if lock.get("commit") != EXPECTED_DESKTOP_COMMIT:
        errors.append("authority lock commit is not current Desktop main")
    if lock.get("rootTreeSha") != EXPECTED_DESKTOP_TREE:
        errors.append("authority lock root tree is not current Desktop main")
    if len(records) != CURRENT_COUNTS["fullTrackedCount"]:
        errors.append(f"full tracked count {len(records)} != {CURRENT_COUNTS['fullTrackedCount']}")
    if len(selected) != CURRENT_COUNTS["selectedFileCount"]:
        errors.append(f"selected count {len(selected)} != {CURRENT_COUNTS['selectedFileCount']}")
    if len(records) - len(selected) != CURRENT_COUNTS["nonselectedCount"]:
        errors.append("nonselected count mismatch")
    full_hash, selected_hash = tree_digest(records), tree_digest(selected)
    if full_hash != EXPECTED_FULL_TREE_SHA256:
        errors.append("full-tree identity SHA-256 mismatch")
    if selected_hash != EXPECTED_SELECTED_TREE_SHA256:
        errors.append("selected-tree identity SHA-256 mismatch")
    if manifest_index.get("fileCount") != len(selected):
        errors.append("manifest index fileCount mismatch")
    if ledger_index.get("rowCount") != len(selected):
        errors.append("ledger index rowCount mismatch")
    manifest_seen = {}
    for group in manifest_index.get("groups", []):
        path = root / group["path"]
        if not path.is_file():
            errors.append("missing manifest chunk " + group["path"])
            continue
        chunk = read_json(path)
        files = chunk.get("files", [])
        if chunk.get("sourceCommit") != EXPECTED_DESKTOP_COMMIT:
            errors.append("stale manifest sourceCommit " + group["path"])
        if len(files) != group.get("fileCount") or chunk.get("fileCount") != len(files):
            errors.append("manifest group count mismatch " + group["path"])
        for row in files:
            p = row.get("path")
            if p in manifest_seen:
                errors.append("duplicate manifest path " + str(p))
                continue
            manifest_seen[p] = row
            actual = selected.get(p)
            if actual is None or row.get("blobSha") != actual["sha"] or row.get("size") != actual["size"]:
                errors.append("manifest/tree identity mismatch " + str(p))
    if set(manifest_seen) != set(selected):
        errors.append("manifest path set does not equal selected tree")
    ledger_seen = {}
    for group in ledger_index.get("chunks", []):
        path = root / group["path"]
        if not path.is_file():
            errors.append("missing ledger chunk " + group["path"])
            continue
        chunk = read_json(path)
        rows = chunk.get("rows", [])
        if chunk.get("sourceCommit") != EXPECTED_DESKTOP_COMMIT:
            errors.append("stale ledger sourceCommit " + group["path"])
        if len(rows) != group.get("rowCount") or chunk.get("rowCount") != len(rows):
            errors.append("ledger group count mismatch " + group["path"])
        for row in rows:
            p = row.get("desktop_path")
            if p in ledger_seen:
                errors.append("duplicate ledger path " + str(p))
                continue
            ledger_seen[p] = row
            actual = selected.get(p)
            if actual is None or row.get("desktop_blob_sha") != actual["sha"]:
                errors.append("ledger/tree identity mismatch " + str(p))
    if set(ledger_seen) != set(selected):
        errors.append("ledger path set does not equal selected tree")
    if HOST_MAPPED_PATH not in ledger_seen or ledger_seen[HOST_MAPPED_PATH].get("implementation_status") != "mapped":
        errors.append("source/host/app/src/main.rs is not mapped")
    delta_path = root / "manifests/desktop-main/current-main-delta.json"
    if delta_path.is_file():
        delta = read_json(delta_path)
        expected_paths = EXPECTED_MODIFIED | EXPECTED_ADDED
        actual_changes = {row.get("path"): row for row in delta.get("sourceChanges", [])}
        if not expected_paths.issubset(actual_changes):
            errors.append("current-main delta is missing the audited changed/new paths")
        for p in expected_paths:
            row = actual_changes.get(p, {})
            if row.get("candidateStatus") != "unreviewed" or row.get("countedAsClosed") is not False:
                errors.append("current-main delta improperly closes " + p)
        for p in EXPECTED_MODIFIED | EXPECTED_ADDED:
            row = ledger_seen.get(p, {})
            if row.get("implementation_status") != "unreviewed" or row.get("countedAsClosed") is not False:
                errors.append("ledger row is not unreviewed/non-closed: " + p)
    else:
        errors.append("current-main delta file is missing")
    historical = lock.get("historicalBaseline", {})
    for key, expected in BASELINE.items():
        actual = historical.get(key, lock.get("inventory", {}).get(key))
        if actual != expected:
            errors.append(f"historical baseline {key}={actual!r}, expected {expected!r}")
    if errors:
        raise SystemExit("current snapshot audit failed:\n- " + "\n- ".join(errors))
    return {
        "status": "pass-already-rebased-no-op",
        "desktopCommit": EXPECTED_DESKTOP_COMMIT,
        "desktopTree": EXPECTED_DESKTOP_TREE,
        "currentCounts": {
            "fullTracked": len(records), "selected": len(selected),
            "nonselected": len(records) - len(selected),
            "gitlinks": sum(1 for r in records.values() if r["type"] == "commit" or r["mode"] == "160000"),
        },
        "fullTreeSha256": full_hash,
        "selectedTreeSha256": selected_hash,
        "manifestRows": len(manifest_seen),
        "ledgerRows": len(ledger_seen),
        "historicalBaseline": BASELINE,
        "preservedMappedOwnerStatus": ledger_seen[HOST_MAPPED_PATH]["implementation_status"],
        "candidateFiles": [],
        "branchWrite": False,
        "finalAcceptance": False,
    }


def main():
    if len(sys.argv) != 4:
        raise SystemExit("usage: generator.py IOS_ROOT DESKTOP_ROOT OUT_DIR")
    root, desktop, out = map(pathlib.Path, sys.argv[1:])
    out.mkdir(parents=True, exist_ok=True)
    expected_ios_head = git(root, "rev-parse", "HEAD")
    if os.environ.get("EXPECTED_IOS_HEAD") and expected_ios_head != os.environ["EXPECTED_IOS_HEAD"]:
        raise SystemExit("iOS checkout does not match EXPECTED_IOS_HEAD")
    if git(desktop, "rev-parse", "HEAD") != EXPECTED_DESKTOP_COMMIT:
        raise SystemExit("Desktop checkout commit mismatch")
    current_tree_sha = git(desktop, "rev-parse", "HEAD^{tree}")
    if current_tree_sha != EXPECTED_DESKTOP_TREE:
        raise SystemExit("Desktop checkout root tree mismatch")
    live = live_main_sha()
    if live != EXPECTED_DESKTOP_COMMIT:
        raise SystemExit("live Desktop main advanced or checkout is stale: " + live)

    lock_path = root / "manifests/desktop-main-authority.json"
    manifest_index_path = root / "manifests/desktop-main-reference-index.json"
    ledger_index_path = root / "docs/parity/desktop-main-index.json"
    lock = read_json(lock_path)
    manifest_index = read_json(manifest_index_path)
    ledger_index = read_json(ledger_index_path)
    current_records = git_tree_records(desktop, "HEAD")
    current_selected = selected_records(current_records)
    identity_artifacts = write_identity_files(out, current_records, current_selected)

    if len(current_records) != CURRENT_COUNTS["fullTrackedCount"]:
        raise SystemExit(f"full tracked count mismatch: {len(current_records)}")
    if len(current_selected) != CURRENT_COUNTS["selectedFileCount"]:
        raise SystemExit(f"selected count mismatch: {len(current_selected)}")
    if len(current_records) - len(current_selected) != CURRENT_COUNTS["nonselectedCount"]:
        raise SystemExit("nonselected count mismatch")
    full_hash = tree_digest(current_records)
    selected_hash = tree_digest(current_selected)
    if full_hash != EXPECTED_FULL_TREE_SHA256:
        raise SystemExit("full-tree SHA-256 mismatch: " + full_hash)
    if selected_hash != EXPECTED_SELECTED_TREE_SHA256:
        raise SystemExit("selected-tree SHA-256 mismatch: " + selected_hash)
    if live_main_sha() != EXPECTED_DESKTOP_COMMIT:
        raise SystemExit("Desktop main changed during inventory generation")

    previous_commit = lock.get("commit")
    previous_tree = lock.get("rootTreeSha")
    if previous_commit == EXPECTED_DESKTOP_COMMIT:
        result = validate_current_snapshot(root, lock, manifest_index, ledger_index,
                                           current_records, current_selected)
        report = {
            "schemaVersion": 2,
            **result,
            "iosHead": expected_ios_head,
            "repository": TARGET_REPO,
            "branch": os.environ.get("GITHUB_HEAD_REF", "spec/grok-bot-018-ios-parity-20260922"),
            "identityArtifacts": identity_artifacts,
            "note": "Snapshot already matches the requested canonical Desktop main; no ref or candidate files were changed.",
        }
        (out / "audit-report.json").write_text(encode_json(report), encoding="utf-8")
        (out / "candidate-files.json").write_text(encode_json({"candidateFiles": []}), encoding="utf-8")
        (out / "candidate-rebaseline.diff").write_text("", encoding="utf-8")
        print("CANDIDATE_BLOB_MANIFEST=" + json.dumps({"candidateFiles": []}, separators=(",", ":")))
        print("CANDIDATE_AUDIT_SUMMARY=" + json.dumps(report, separators=(",", ":")))
        return

    if previous_commit != EXPECTED_PREVIOUS_COMMIT or previous_tree != EXPECTED_PREVIOUS_TREE:
        raise SystemExit("authority predecessor does not match the audited dc81eac2/04473e18 snapshot")
    if git(desktop, "rev-parse", previous_commit + "^{tree}") != EXPECTED_PREVIOUS_TREE:
        raise SystemExit("previous Desktop commit/tree identity mismatch")
    previous_records = git_tree_records(desktop, previous_commit)
    previous_selected = selected_records(previous_records)
    if len(previous_records) != PREVIOUS_COUNTS["fullTrackedCount"] or len(previous_selected) != PREVIOUS_COUNTS["selectedFileCount"]:
        raise SystemExit("predecessor source denominators do not match 8260/7997/263")
    if len(previous_records) - len(previous_selected) != PREVIOUS_COUNTS["nonselectedCount"]:
        raise SystemExit("predecessor nonselected count does not match 263")
    if tree_digest(previous_records) != lock.get("inventory", {}).get("fullTreeSha256"):
        raise SystemExit("historical full-tree identity does not match authority lock")
    if tree_digest(previous_selected) != lock.get("inventory", {}).get("selectedTreeSha256"):
        raise SystemExit("historical selected-tree identity does not match authority lock")
    if manifest_index.get("fileCount") != PREVIOUS_COUNTS["selectedFileCount"] or ledger_index.get("rowCount") != PREVIOUS_COUNTS["selectedFileCount"]:
        raise SystemExit("predecessor manifest/ledger denominator drifted")
    if lock.get("repository") != "bhrumom/fabushi-desktop" or lock.get("branch") != "main":
        raise SystemExit("authority repository/branch mismatch")

    old_manifest_by_path = {}
    manifest_group_by_path = {}
    for group in manifest_index.get("groups", []):
        chunk_path = root / group["path"]
        if not chunk_path.is_file():
            raise SystemExit("missing predecessor manifest chunk: " + group["path"])
        chunk = read_json(chunk_path)
        files = chunk.get("files", [])
        if chunk.get("sourceCommit") != previous_commit or chunk.get("group") != group["name"]:
            raise SystemExit("predecessor manifest authority/group mismatch: " + group["path"])
        if len(files) != group.get("fileCount") or chunk.get("fileCount") != len(files):
            raise SystemExit("predecessor manifest count mismatch: " + group["path"])
        for row in files:
            path = row.get("path")
            if path in old_manifest_by_path:
                raise SystemExit("duplicate predecessor manifest path: " + str(path))
            old_manifest_by_path[path] = row
            manifest_group_by_path[path] = group["name"]
            old = previous_selected.get(path)
            if old is None or row.get("blobSha") != old["sha"] or row.get("size") != old["size"]:
                raise SystemExit("predecessor manifest/tree mismatch: " + str(path))
    if set(old_manifest_by_path) != set(previous_selected):
        raise SystemExit("predecessor manifest path set differs from the selected tree")

    old_ledger_by_path = {}
    ledger_group_by_path = {}
    for group in ledger_index.get("chunks", []):
        chunk_path = root / group["path"]
        if not chunk_path.is_file():
            raise SystemExit("missing predecessor ledger chunk: " + group["path"])
        chunk = read_json(chunk_path)
        rows = chunk.get("rows", [])
        expected_chunk_group = "source-mahayana" if group["name"].startswith("source-mahayana") else group["name"]
        if chunk.get("sourceCommit") != previous_commit or chunk.get("group") != expected_chunk_group:
            raise SystemExit("predecessor ledger authority/group mismatch: " + group["path"])
        if len(rows) != group.get("rowCount") or chunk.get("rowCount") != len(rows):
            raise SystemExit("predecessor ledger count mismatch: " + group["path"])
        for row in rows:
            path = row.get("desktop_path")
            if path in old_ledger_by_path:
                raise SystemExit("duplicate predecessor ledger path: " + str(path))
            old_ledger_by_path[path] = row
            ledger_group_by_path[path] = group["name"]
            old = previous_selected.get(path)
            if old is None or row.get("desktop_blob_sha") != old["sha"]:
                raise SystemExit("predecessor ledger/tree mismatch: " + str(path))
    if set(old_ledger_by_path) != set(previous_selected):
        raise SystemExit("predecessor ledger path set differs from the selected tree")
    if HOST_MAPPED_PATH not in old_ledger_by_path or old_ledger_by_path[HOST_MAPPED_PATH].get("implementation_status") != "mapped":
        raise SystemExit("source/host/app/src/main.rs must remain mapped")

    raw_changes = parse_name_status(desktop, previous_commit, EXPECTED_DESKTOP_COMMIT)
    expected_statuses = {path: "M" for path in EXPECTED_MODIFIED}
    expected_statuses.update({path: "A" for path in EXPECTED_ADDED})
    observed_statuses = {row.get("path"): row.get("status") for row in raw_changes}
    if observed_statuses != expected_statuses or len(raw_changes) != len(expected_statuses):
        raise SystemExit("selected Desktop delta differs from the audited current-main path set: "
                         + json.dumps(raw_changes, sort_keys=True))

    historical = copy.deepcopy(lock.get("historicalBaseline", BASELINE))
    for key, value in BASELINE.items():
        if historical.get(key, value) != value:
            raise SystemExit("historical denominator was altered: " + key)
        historical[key] = value
    historical.setdefault("desktopCommit", previous_commit)
    historical.setdefault("rootTreeSha", previous_tree)

    groups = {g["name"]: g for g in manifest_index["groups"]}
    ledger_groups = {g["name"]: g for g in ledger_index["chunks"]}
    current_group_for_path = {}
    current_ledger_group_for_path = {}
    for path in current_selected:
        manifest_group_name = manifest_group_by_path.get(path)
        if manifest_group_name is None:
            if path.startswith("frontend/"):
                manifest_group_name = "frontend"
            elif path.startswith("source/electron-main/"):
                manifest_group_name = "source-electron-main"
            elif path.startswith("source/shared/"):
                manifest_group_name = "source-shared"
            elif path.startswith("source/mahayana/"):
                manifest_group_name = "source-mahayana"
            else:
                raise SystemExit("new selected path has no manifest group: " + path)
        ledger_group_name = ledger_group_by_path.get(path, manifest_group_name)
        if manifest_group_name not in groups or ledger_group_name not in ledger_groups:
            raise SystemExit("selected path has no paired manifest/ledger group: " + path)
        current_group_for_path[path] = manifest_group_name
        current_ledger_group_for_path[path] = ledger_group_name

    for path in set(previous_selected) - set(current_selected):
        raise SystemExit("selected path deletion requires a separately reviewed rebaseline: " + path)

    current_manifest_rows = {name: [] for name in groups}
    current_ledger_rows = {name: [] for name in ledger_groups}
    changed_paths = EXPECTED_MODIFIED | EXPECTED_ADDED
    new_owner = {
        "source/mahayana/mahayana-rs/mahayana-app-host/tests/native_desktop_account_handoff.rs":
            "mahayana-app-host native desktop account handoff test; owner review pending",
        "source/mahayana/mahayana-rs/mahayana-product/src/native_desktop_session.rs":
            "mahayana-product native desktop session runtime; owner review pending",
    }
    mahayana_template = next(
        (row for row in old_ledger_by_path.values()
         if row.get("desktop_path", "").startswith("source/mahayana/")
         and row.get("implementation_status") == "unreviewed"),
        None,
    )
    if mahayana_template is None:
        mahayana_template = next(row for row in old_ledger_by_path.values()
                                 if row.get("desktop_path", "").startswith("source/mahayana/"))

    for path, record in sorted(current_selected.items()):
        manifest_group_name = current_group_for_path[path]
        ledger_group_name = current_ledger_group_for_path[path]
        current_manifest_rows[manifest_group_name].append({
            "path": path, "blobSha": record["sha"], "size": record["size"],
        })
        if path in old_ledger_by_path:
            row = copy.deepcopy(old_ledger_by_path[path])
            row["desktop_path"] = path
            row["desktop_blob_sha"] = record["sha"]
        else:
            row = copy.deepcopy(next(candidate for old_path, candidate in old_ledger_by_path.items()
                                     if ledger_group_by_path.get(old_path) == ledger_group_name))
            row["desktop_path"] = path
            row["desktop_blob_sha"] = record["sha"]
            row["desktop_responsibility"] = "pending-current-main-responsibility-review"
            row["desktop_owner"] = new_owner.get(path, "Current-main owner review pending")
            row["desktop_visible_effect"] = "Pending current-main review; no iOS product effect is accepted yet."
            row["ios_disposition"] = "unreviewed"
            row["ios_target_path"] = ""
            row["ios_language"] = "unknown"
            row["ios_platform_delta"] = "Pending current-main owner/contract review; no iOS replacement or implementation is claimed."
            row["implementation_status"] = "unreviewed"
            row["production_evidence"] = ""
            row["test_evidence"] = ""
            row["notes"] = "New Desktop-main source path with no predecessor ledger row; explicit owner, contract, production and test review required."
            for key in (
                "responsibility_id", "ios_target_symbol", "ios_symbol", "ios_target_commit",
                "production_entrypoint", "oracle_ids", "workflow_run", "run_attempt", "job",
                "artifact_id", "artifact_digest", "independent_reviewer", "desktop_review_commit",
                "exact_head_evidence",
            ):
                row.pop(key, None)
        if path in changed_paths:
            row["implementation_status"] = "unreviewed"
            row["authority_review_state"] = "requires-current-main-review"
            row["countedAsClosed"] = False
            prior_notes = row.get("notes", "")
            row["notes"] = (prior_notes + " Current Desktop-main blob changed/added; predecessor status and evidence are historical and non-promotable until explicit current-main review.").strip()
            for key in ("workflow_run", "run_attempt", "job", "artifact_id", "artifact_digest",
                        "ios_target_commit", "desktop_review_commit", "independent_reviewer",
                        "exact_head_evidence"):
                row.pop(key, None)
            row["production_evidence"] = ""
            row["test_evidence"] = ""
        current_ledger_rows[ledger_group_name].append(row)

    candidates: dict[str, str] = {}
    for name, group in groups.items():
        rows = sorted(current_manifest_rows[name], key=lambda r: r["path"])
        manifest = {
            "sourceCommit": EXPECTED_DESKTOP_COMMIT,
            "group": "source-mahayana" if name.startswith("source-mahayana") else name,
            "fileCount": len(rows),
            "files": rows,
        }
        write_candidate(out, candidates, group["path"], manifest)
        group["fileCount"] = len(rows)
    for name, group in ledger_groups.items():
        rows = sorted(current_ledger_rows[name], key=lambda r: r["desktop_path"])
        chunk = {
            "schemaVersion": 1,
            "sourceCommit": EXPECTED_DESKTOP_COMMIT,
            "group": name,
            "rowCount": len(rows),
            "rows": rows,
        }
        write_candidate(out, candidates, group["path"], chunk)
        group["rowCount"] = len(rows)

    observed_at = dt.datetime.now(dt.timezone.utc).date().isoformat()
    current_full_count = len(current_records)
    current_selected_count = len(current_selected)
    current_nonselected_count = current_full_count - current_selected_count
    gitlink_count = sum(1 for r in current_records.values()
                        if r["type"] == "commit" or r["mode"] == "160000")
    inventory = {
        "fullTrackedCount": current_full_count,
        "selectedFileCount": current_selected_count,
        "nonselectedCount": current_nonselected_count,
        "gitlinkCount": gitlink_count,
        "fullTreeSha256": full_hash,
        "selectedTreeSha256": selected_hash,
    }
    source_change_rows = []
    responsibility_actions = []
    for path in sorted(changed_paths):
        current = current_records[path]
        previous = previous_records.get(path)
        old_row = old_ledger_by_path.get(path)
        status = "added" if path in EXPECTED_ADDED else "modified"
        responsibility_id = old_row.get("responsibility_id") if old_row else None
        previous_status = old_row.get("implementation_status") if old_row else None
        entry = {
            "path": path,
            "change": status,
            "blobSha": current["sha"],
            "size": current["size"],
            "mode": current["mode"],
            "type": current["type"],
            "responsibility_id": responsibility_id,
            "previousStatus": previous_status,
            "candidateStatus": "unreviewed",
            "implementation_status": "unreviewed",
            "countedAsClosed": False,
        }
        if previous:
            entry["previousBlobSha"] = previous["sha"]
            entry["previousSize"] = previous["size"]
            entry["previousMode"] = previous["mode"]
            entry["previousType"] = previous["type"]
        source_change_rows.append(entry)
        responsibility_actions.append({
            "path": path,
            "responsibility_id": responsibility_id,
            "previousStatus": previous_status,
            "candidateStatus": "unreviewed",
            "countedAsClosed": False,
        })

    lock_candidate = copy.deepcopy(lock)
    lock_candidate["commit"] = EXPECTED_DESKTOP_COMMIT
    lock_candidate["rootTreeSha"] = EXPECTED_DESKTOP_TREE
    lock_candidate["observedAt"] = observed_at
    lock_candidate["previousSourceCommit"] = previous_commit
    lock_candidate["inventory"] = {
        **lock_candidate.get("inventory", {}),
        "selectedRoots": ["frontend/**", "source/**"],
        "selectedFileCount": current_selected_count,
        "fullTrackedCount": current_full_count,
        "gitlinkCount": gitlink_count,
        "nonselectedCount": current_nonselected_count,
        "fullTreeSha256": full_hash,
        "selectedTreeSha256": selected_hash,
        "manifestIndex": "manifests/desktop-main-reference-index.json",
        "ledgerIndex": "docs/parity/desktop-main-index.json",
        "nonselectedRegister": "manifests/desktop-main/tracked-outside-selected.json",
        "impactRegister": "docs/parity/desktop-main-impact.json",
    }
    lock_candidate["historicalBaseline"] = historical
    lock_candidate["baselineReady"] = False
    lock_candidate["dependenciesReviewed"] = False
    lock_candidate["completenessAccepted"] = False
    lock_candidate["acceptanceAccepted"] = False
    lock_candidate["notes"] = (
        "Rebased to canonical Desktop main " + EXPECTED_DESKTOP_COMMIT + "/" + EXPECTED_DESKTOP_TREE
        + ". Current-main changed/new frontend/MCP paths remain unreviewed and non-closed; "
        "historical source denominators 8258/7995/263 and responsibility denominator 9412 are preserved."
    )
    rebaseline = copy.deepcopy(lock_candidate.get("iosVerificationRebaseline", {}))
    rebaseline.update({
        "observedAt": observed_at,
        "preEvidenceIosHead": expected_ios_head,
        "desktopCommit": EXPECTED_DESKTOP_COMMIT,
        "desktopTree": EXPECTED_DESKTOP_TREE,
        "status": "desktop-authority-drift-rebaseline-current-main-review-required",
        "promotion": "none",
    })
    lock_candidate["iosVerificationRebaseline"] = rebaseline
    write_candidate(out, candidates, "manifests/desktop-main-authority.json", lock_candidate)

    manifest_index_candidate = copy.deepcopy(manifest_index)
    manifest_index_candidate["authority"] = {
        **manifest_index_candidate.get("authority", {}),
        "repository": "bhrumom/fabushi-desktop",
        "branch": "main",
        "commit": EXPECTED_DESKTOP_COMMIT,
        "tree": EXPECTED_DESKTOP_TREE,
        "mode": "live-main",
        "observedAt": observed_at,
    }
    manifest_index_candidate["fileCount"] = current_selected_count
    manifest_index_candidate["selectedTreeSha256"] = selected_hash
    manifest_index_candidate["historicalBaseline"] = historical
    write_candidate(out, candidates, "manifests/desktop-main-reference-index.json", manifest_index_candidate)

    ledger_index_candidate = copy.deepcopy(ledger_index)
    ledger_index_candidate["source"] = {
        **ledger_index_candidate.get("source", {}),
        "repository": "bhrumom/fabushi-desktop",
        "branch": "main",
        "commit": EXPECTED_DESKTOP_COMMIT,
        "tree": EXPECTED_DESKTOP_TREE,
        "mode": "live-main",
    }
    ledger_index_candidate["rowCount"] = current_selected_count
    ledger_index_candidate["carryForwardStatusesAreHistorical"] = True
    ledger_index_candidate["readiness"] = "desktop-authority-drift-rebaseline-current-main-review-required"
    ledger_index_candidate["historicalBaseline"] = historical
    verification = copy.deepcopy(ledger_index_candidate.get("iosVerificationRebaseline", {}))
    verification.update({
        "observedAt": observed_at,
        "preEvidenceIosHead": expected_ios_head,
        "desktopCommit": EXPECTED_DESKTOP_COMMIT,
        "desktopTree": EXPECTED_DESKTOP_TREE,
        "status": "desktop-authority-drift-rebaseline-current-main-review-required",
        "promotion": "none",
    })
    ledger_index_candidate["iosVerificationRebaseline"] = verification
    write_candidate(out, candidates, "docs/parity/desktop-main-index.json", ledger_index_candidate)

    outside = {
        "sourceCommit": EXPECTED_DESKTOP_COMMIT,
        "rootTree": EXPECTED_DESKTOP_TREE,
        "count": current_nonselected_count,
        "entries": [
            {"path": p, "mode": current_records[p]["mode"], "type": current_records[p]["type"],
             "sha": current_records[p]["sha"], "size": current_records[p]["size"]}
            for p in sorted(current_records) if p not in current_selected
        ],
    }
    if len(outside["entries"]) != CURRENT_COUNTS["nonselectedCount"]:
        raise SystemExit("nonselected path register count mismatch")
    write_candidate(out, candidates, "manifests/desktop-main/tracked-outside-selected.json", outside)

    old_impact_path = root / "docs/parity/desktop-main-impact.json"
    old_impact = read_json(old_impact_path)
    impact = copy.deepcopy(old_impact)
    impact["previousCommit"] = previous_commit
    impact["currentCommit"] = EXPECTED_DESKTOP_COMMIT
    impact["fullTree"] = EXPECTED_DESKTOP_TREE
    impact["directCount"] = 0
    impact["metadataCorrections"] = []
    impact["fullReviewAccepted"] = False
    impact["directChanges"] = []
    impact["dependencyReview"] = {
        "status": "pending-current-main-rebaseline",
        "reason": "Current Desktop main changed 12 selected frontend/source paths. All changed/new rows require explicit owner, contract, production and test review; no row is counted closed.",
        "previousAuthority": {"commit": previous_commit, "tree": previous_tree},
        "currentAuthority": {"commit": EXPECTED_DESKTOP_COMMIT, "tree": EXPECTED_DESKTOP_TREE},
    }
    impact["historyPolicy"] = (
        "Prior authority and CI evidence are predecessor-only after live-main drift. "
        "Unchanged selected source statuses remain historical until explicit owner/contract revalidation. "
        "Changed/new current-main paths are unreviewed and countedAsClosed=false."
    )
    impacted_groups = sorted({current_group_for_path[path] for path in changed_paths})
    impact["groupSummary"] = {
        name: {
            "fileCount": len(current_manifest_rows[name]),
            "changedOrAdded": sum(1 for path in changed_paths if current_group_for_path[path] == name),
        }
        for name in impacted_groups
    }
    impact["totalTrackedChangeCount"] = current_full_count
    impact["outsideSelectedChanges"] = []
    impact["currentSourceChanges"] = source_change_rows
    impact["historicalPriorImpact"] = {
        "previousCommit": old_impact.get("previousCommit"),
        "currentCommit": old_impact.get("currentCommit"),
        "fullTree": old_impact.get("fullTree"),
        "metadataCorrections": old_impact.get("metadataCorrections", []),
    }
    impact["currentInventory"] = inventory
    impact["historicalBaseline"] = historical
    write_candidate(out, candidates, "docs/parity/desktop-main-impact.json", impact)

    delta = {
        "schemaVersion": 2,
        "desktopRepository": "bhrumom/fabushi-desktop",
        "desktopBranch": "main",
        "previousCommit": previous_commit,
        "previousTree": previous_tree,
        "currentCommit": EXPECTED_DESKTOP_COMMIT,
        "currentTree": EXPECTED_DESKTOP_TREE,
        "baselineDenominators": {
            "fullTracked": BASELINE["fullTrackedCount"],
            "selectedFrontendAndSource": BASELINE["selectedFileCount"],
            "nonselected": BASELINE["nonselectedCount"],
            "responsibilityLedger": BASELINE["responsibilityLedgerCount"],
        },
        "historicalBaseline": historical,
        "currentInventory": inventory,
        "fullTreeSha256": full_hash,
        "selectedTreeSha256": selected_hash,
        "status": "candidate-only-not-accepted",
        "finalAcceptance": False,
        "sourceChanges": source_change_rows,
        "responsibilityActions": responsibility_actions,
        "preservedMappedPaths": [{
            "path": HOST_MAPPED_PATH,
            "implementation_status": "mapped",
            "preserved": True,
        }],
        "preservedMappedOwnerStatus": "mapped",
        "newSourceFiles": [
            {"path": path, "blobSha": current_records[path]["sha"], "size": current_records[path]["size"],
             "mode": current_records[path]["mode"], "type": current_records[path]["type"],
             "implementation_status": "unreviewed", "countedAsClosed": False}
            for path in sorted(EXPECTED_ADDED)
        ],
        "requiredChangedPathsRemainUnreviewed": True,
        "safety": {"branchWrite": False, "finalAcceptance": False, "promotion": "none"},
    }
    delta_content = write_candidate(out, candidates, "manifests/desktop-main/current-main-delta.json", delta)
    delta_sha256 = sha256_bytes(delta_content.encode("utf-8"))

    release = read_json(root / "docs/parity/desktop-main-release-evidence.json")
    release = copy.deepcopy(release)
    release["desktopSourceCommit"] = EXPECTED_DESKTOP_COMMIT
    release["desktopRootTree"] = EXPECTED_DESKTOP_TREE
    release["status"] = "desktop-authority-drift-rebaseline-current-main-review-required"
    release["finalAcceptance"] = False
    release["finalIosCommit"] = None
    release["note"] = (
        "Desktop main is pinned to " + EXPECTED_DESKTOP_COMMIT + " (tree " + EXPECTED_DESKTOP_TREE
        + "). Candidate manifests/ledger were generated and audited, but all predecessor CI/artifacts are non-promotable. "
        "Current-main changed/new rows remain unreviewed; required acceptance and production evidence are pending."
    )
    current_head = copy.deepcopy(release.get("currentHeadRebaseline", {}))
    current_head.update({
        "observedAt": observed_at,
        "preEvidenceIosHead": expected_ios_head,
        "desktopCommit": EXPECTED_DESKTOP_COMMIT,
        "desktopTree": EXPECTED_DESKTOP_TREE,
        "status": "desktop-authority-drift-rebaseline-current-main-review-required",
        "promotion": "none",
        "currentInventory": inventory,
        "currentMainDeltaPath": "manifests/desktop-main/current-main-delta.json",
        "currentMainDeltaSha256": delta_sha256,
        "historicalBaseline": historical,
    })
    release["currentHeadRebaseline"] = current_head
    for criterion in release.get("acceptanceCriteria", []):
        criterion["status"] = "pending"
        criterion["iosCommit"] = None
        criterion["evidence"] = []
        criterion["independentReviewer"] = None
    for gate in release.get("releaseGates", {}).values():
        gate["status"] = "pending"
        gate["iosCommit"] = None
        gate["desktopCommit"] = None
        gate["workflowRun"] = None
        gate["runAttempt"] = None
        gate["job"] = None
        gate["requiredSteps"] = []
        gate["artifactId"] = None
        gate["artifactDigest"] = None
        gate["independentReviewer"] = None
    release["historicalBaseline"] = historical
    write_candidate(out, candidates, "docs/parity/desktop-main-release-evidence.json", release)

    # Validate every candidate inventory/ledger path against the exact Desktop tree.
    candidate_manifest_paths = {}
    for name, group in groups.items():
        manifest = json.loads(candidates[group["path"]])
        if manifest["sourceCommit"] != EXPECTED_DESKTOP_COMMIT:
            raise SystemExit("candidate manifest sourceCommit mismatch")
        for row in manifest["files"]:
            p = row["path"]
            if p in candidate_manifest_paths:
                raise SystemExit("candidate manifest duplicate path: " + p)
            candidate_manifest_paths[p] = row
            actual = current_selected.get(p)
            if actual is None or row["blobSha"] != actual["sha"] or row["size"] != actual["size"]:
                raise SystemExit("candidate manifest/tree mismatch: " + p)
    if set(candidate_manifest_paths) != set(current_selected):
        raise SystemExit("candidate manifest does not cover all selected paths")
    candidate_ledger_paths = {}
    for name, group in ledger_groups.items():
        chunk = json.loads(candidates[group["path"]])
        if chunk["sourceCommit"] != EXPECTED_DESKTOP_COMMIT:
            raise SystemExit("candidate ledger sourceCommit mismatch")
        for row in chunk["rows"]:
            p = row["desktop_path"]
            if p in candidate_ledger_paths:
                raise SystemExit("candidate ledger duplicate path: " + p)
            candidate_ledger_paths[p] = row
            actual = current_selected.get(p)
            if actual is None or row["desktop_blob_sha"] != actual["sha"]:
                raise SystemExit("candidate ledger/tree mismatch: " + p)
    if set(candidate_ledger_paths) != set(current_selected):
        raise SystemExit("candidate ledger does not cover all selected paths")
    for path in changed_paths:
        row = candidate_ledger_paths[path]
        if row.get("implementation_status") != "unreviewed" or row.get("countedAsClosed") is not False:
            raise SystemExit("candidate changed/new path is not unreviewed/non-closed: " + path)
    if candidate_ledger_paths[HOST_MAPPED_PATH].get("implementation_status") != "mapped":
        raise SystemExit("candidate accidentally changed source/host/app/src/main.rs from mapped")
    if sum(g["fileCount"] for g in groups.values()) != CURRENT_COUNTS["selectedFileCount"]:
        raise SystemExit("candidate manifest group counts do not sum to current selected count")
    if sum(g["rowCount"] for g in ledger_groups.values()) != CURRENT_COUNTS["selectedFileCount"]:
        raise SystemExit("candidate ledger group counts do not sum to current selected count")
    if live_main_sha() != EXPECTED_DESKTOP_COMMIT:
        raise SystemExit("Desktop main changed during candidate audit")

    # Produce a reviewable diff of every candidate file against the current iOS branch.
    diff_parts = []
    for path in sorted(candidates):
        old_path = root / path
        old_text = old_path.read_text(encoding="utf-8") if old_path.is_file() else ""
        new_text = candidates[path]
        if old_text == new_text:
            continue
        diff_parts.extend(difflib.unified_diff(
            old_text.splitlines(keepends=True), new_text.splitlines(keepends=True),
            fromfile="a/" + path, tofile="b/" + path,
        ))
    diff_text = "".join(diff_parts)
    (out / "candidate-rebaseline.diff").write_text(diff_text, encoding="utf-8")

    # Detached Git blobs allow a later GitHub Connector create-tree/commit/ref update
    # after the model has independently inspected this artifact. No branch ref is moved here.
    token = os.environ.get("GITHUB_TOKEN", "")
    if not token:
        raise SystemExit("GITHUB_TOKEN is required to create detached candidate blob objects")
    ios_blobs = ios_tree_blobs(root)
    candidate_file_records = []
    for path in sorted(candidates):
        old_text = (root / path).read_text(encoding="utf-8") if (root / path).is_file() else None
        content = candidates[path]
        if old_text == content:
            continue
        old_meta = ios_blobs.get(path)
        if old_meta is None:
            raise SystemExit("candidate target path is absent from iOS HEAD: " + path)
        candidate_blob_sha = create_git_blob(content, token)
        candidate_file_records.append({
            "path": path,
            "expectedOldBlobSha": old_meta["sha"],
            "expectedMode": old_meta["mode"],
            "expectedType": old_meta["type"],
            "candidateBlobSha": candidate_blob_sha,
            "candidateBytes": len(content.encode("utf-8")),
            "candidateSha256": sha256_bytes(content.encode("utf-8")),
            "candidateMode": "100644",
            "candidateType": "blob",
        })
    base_tree_sha = git(root, "rev-parse", "HEAD^{tree}")
    tree_entries = [
        {"path": row["path"], "mode": row["candidateMode"], "type": row["candidateType"], "sha": row["candidateBlobSha"]}
        for row in candidate_file_records
    ]
    candidate_tree_sha = create_git_tree(tree_entries, base_tree_sha, token)
    (out / "candidate-files.json").write_text(
        encode_json({"repository": TARGET_REPO, "branch": os.environ.get("GITHUB_HEAD_REF", ""),
                     "expectedIosHead": expected_ios_head, "baseTreeSha": base_tree_sha,
                     "candidateTreeSha": candidate_tree_sha, "candidateFiles": candidate_file_records}),
        encoding="utf-8",
    )
    report = {
        "schemaVersion": 2,
        "status": "pass-candidate-integrity",
        "candidateOnly": True,
        "repository": TARGET_REPO,
        "branch": os.environ.get("GITHUB_HEAD_REF", "spec/grok-bot-018-ios-parity-20260922"),
        "iosHead": expected_ios_head,
        "desktopRepository": "bhrumom/fabushi-desktop",
        "desktopBranch": "main",
        "previousDesktopCommit": previous_commit,
        "previousDesktopTree": previous_tree,
        "currentDesktopCommit": EXPECTED_DESKTOP_COMMIT,
        "currentDesktopRootTree": EXPECTED_DESKTOP_TREE,
        "baselineDenominators": BASELINE,
        "currentInventory": inventory,
        "fullTreeSha256": full_hash,
        "selectedTreeSha256": selected_hash,
        "identityArtifacts": identity_artifacts,
        "sourceChangeCount": len(source_change_rows),
        "sourceChanges": source_change_rows,
        "responsibilityActions": responsibility_actions,
        "manifestRows": len(candidate_manifest_paths),
        "ledgerRows": len(candidate_ledger_paths),
        "uniqueManifestPaths": len(set(candidate_manifest_paths)),
        "uniqueLedgerPaths": len(set(candidate_ledger_paths)),
        "preservedMappedOwnerStatus": candidate_ledger_paths[HOST_MAPPED_PATH]["implementation_status"],
        "candidateFileCount": len(candidate_file_records),
        "baseTreeSha": base_tree_sha,
        "candidateTreeSha": candidate_tree_sha,
        "candidateFiles": candidate_file_records,
        "candidateDiffBytes": len(diff_text.encode("utf-8")),
        "candidateDiffSha256": sha256_bytes(diff_text.encode("utf-8")),
        "branchWrite": False,
        "refMoved": False,
        "finalAcceptance": False,
        "requiredAcceptance": "pending",
        "note": "All candidate files are under RUNNER_TEMP; only detached Git blob objects were created. A connector lease-protected ref update is required after independent artifact review.",
    }
    (out / "audit-report.json").write_text(encode_json(report), encoding="utf-8")
    print("CANDIDATE_BLOB_MANIFEST=" + json.dumps({
        "repository": TARGET_REPO,
        "branch": os.environ.get("GITHUB_HEAD_REF", ""),
        "expectedIosHead": expected_ios_head,
        "desktopCommit": EXPECTED_DESKTOP_COMMIT,
        "desktopTree": EXPECTED_DESKTOP_TREE,
        "baseTreeSha": base_tree_sha,
        "candidateTreeSha": candidate_tree_sha,
        "candidateFiles": candidate_file_records,
    }, separators=(",", ":")))
    print("CANDIDATE_AUDIT_SUMMARY=" + json.dumps({
        "status": report["status"],
        "iosHead": expected_ios_head,
        "desktopCommit": EXPECTED_DESKTOP_COMMIT,
        "desktopTree": EXPECTED_DESKTOP_TREE,
        "currentInventory": inventory,
        "sourceChangeCount": len(source_change_rows),
        "candidateFileCount": len(candidate_file_records),
        "fullTreeSha256": full_hash,
        "selectedTreeSha256": selected_hash,
        "preservedMappedOwnerStatus": report["preservedMappedOwnerStatus"],
        "finalAcceptance": False,
    }, separators=(",", ":")))


if __name__ == "__main__":
    main()
