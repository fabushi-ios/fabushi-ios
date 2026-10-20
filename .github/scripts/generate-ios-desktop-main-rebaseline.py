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

EXPECTED_DESKTOP_COMMIT = ""
EXPECTED_DESKTOP_TREE = ""
BASELINE = {
    "fullTrackedCount": 8258,
    "selectedFileCount": 7995,
    "nonselectedCount": 263,
    "responsibilityLedgerCount": 9412,
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


def validate_host_mapped_owner_status(ledger_seen: dict, changed_paths: set[str],
                                      errors: list[str]) -> None:
    """Keep Host ownership fail-closed without auto-promoting inherited pending work.

    A current source change must be unreviewed/non-closed. An already-unreviewed
    Host entrypoint may remain pending across unrelated upstream commits only when
    its review fence is explicit and it carries no promotable evidence.
    """
    row = ledger_seen.get(HOST_MAPPED_PATH)
    if row is None:
        errors.append("source/host/app/src/main.rs is missing from the current ledger")
        return
    status = row.get("implementation_status")
    if status == "mapped":
        if HOST_MAPPED_PATH in changed_paths:
            errors.append("changed source/host/app/src/main.rs must remain unreviewed/non-closed")
        return
    if status != "unreviewed":
        errors.append("source/host/app/src/main.rs has unsupported implementation status: " + str(status))
        return
    if row.get("countedAsClosed") is not False:
        errors.append("source/host/app/src/main.rs pending status must remain countedAsClosed=false")
    if row.get("authority_review_state") not in {
        "pending", "requires-current-main-review", "requires-main-responsibility-and-owner-review"
    }:
        errors.append("source/host/app/src/main.rs pending status lacks an explicit owner-review fence")
    for field in (
        "production_evidence", "test_evidence", "exact_head_evidence", "ios_target_commit",
        "workflow_run", "run_attempt", "job", "artifact_id", "artifact_digest",
        "desktop_review_commit", "independent_reviewer",
    ):
        if row.get(field):
            errors.append("source/host/app/src/main.rs pending status carries promotable evidence field " + field)


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


def reconstruct_historical_change_register(repo: pathlib.Path, historical_previous_commit: str,
                                          current_commit: str, current_records: dict[str, dict],
                                          ledger_by_path: dict[str, dict]) -> dict:
    """Rebuild a lost same-main register from its retained historical authority commit."""
    if len(historical_previous_commit) != 40 or git(
        repo, "rev-parse", historical_previous_commit + "^{commit}"
    ) != historical_previous_commit:
        raise SystemExit("historical impact previousCommit is not an exact commit SHA")
    historical_records = git_tree_records(repo, historical_previous_commit)
    historical_selected = selected_records(historical_records)
    current_selected = selected_records(current_records)
    raw_changes = parse_name_status(repo, historical_previous_commit, current_commit)
    changed_paths = set()
    added_paths = set()
    for change in raw_changes:
        path = change.get("path")
        old_path = change.get("oldPath")
        status = change.get("status", "")
        if old_path and old_path in historical_selected and old_path not in current_selected:
            raise SystemExit("historical selected path rename requires a separately reviewed rebaseline: " + old_path)
        if path in current_selected:
            if path not in historical_selected:
                if not status.startswith("A"):
                    raise SystemExit("historical new selected path has unsupported Git change status: " + path + " " + status)
                added_paths.add(path)
            elif status.startswith("M"):
                changed_paths.add(path)
            else:
                raise SystemExit("historical selected path has unsupported Git change status: " + path + " " + status)
    removed_selected = set(historical_selected) - set(current_selected)
    if removed_selected:
        raise SystemExit("historical selected path deletion requires a separately reviewed rebaseline: "
                         + sorted(removed_selected)[0])
    changed_paths.update(added_paths)

    source_changes = []
    responsibility_actions = []
    for path in sorted(changed_paths):
        current = current_records[path]
        previous = historical_records.get(path)
        old_row = ledger_by_path.get(path)
        responsibility_id = old_row.get("responsibility_id") if old_row else None
        # The current ledger was already demoted during the original rebaseline. Do not
        # pretend its status is the historical pre-change status when reconstructing it.
        previous_status = None
        entry = {
            "path": path,
            "change": "added" if path in added_paths else "modified",
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
        source_changes.append(entry)
        responsibility_actions.append({
            "path": path,
            "responsibility_id": responsibility_id,
            "previousStatus": previous_status,
            "candidateStatus": "unreviewed",
            "countedAsClosed": False,
        })
    return {
        "changedPaths": changed_paths,
        "addedPaths": added_paths,
        "sourceChanges": source_changes,
        "responsibilityActions": responsibility_actions,
        "impactSourceChanges": copy.deepcopy(source_changes),
    }


def classify_new_path_manifest_group(path: str) -> str:
    """Map newly added selected paths to an existing group; fail closed otherwise."""
    if path.startswith("frontend/"):
        return "frontend"
    prefix_groups = (
        ("source/box-exec-daemon/", "source-box-exec-daemon"),
        ("source/electron-dev-controls/", "source-electron-dev-controls"),
        ("source/electron-main/", "source-electron-main"),
        ("source/electron-preload/", "source-electron-preload"),
        ("source/fabushi-network-worker/", "source-fabushi-network-worker"),
        ("source/host/", "source-host"),
        ("source/internal/", "source-internal"),
        ("source/local-exec-daemon/", "source-local-exec-daemon"),
        ("source/mahayana/", "source-mahayana"),
        ("source/node-agent-coordinator/", "source-node-agent-coordinator"),
        ("source/packages/", "source-packages"),
        ("source/packaging/", "source-packaging"),
        ("source/product/", "source-product"),
        ("source/shared/", "source-shared"),
    )
    for prefix, group in prefix_groups:
        if path.startswith(prefix):
            return group
    exact_groups = {
        "source/mime-types.d.ts": "source-mime-types.d.ts",
        "source/tsconfig.json": "source-tsconfig.json",
    }
    if path in exact_groups:
        return exact_groups[path]
    raise SystemExit("new selected path has no manifest group: " + path)


def live_main_sha() -> str:
    value = subprocess.check_output(
        ["git", "ls-remote", "--heads", UPSTREAM_URL, "refs/heads/main"],
        stderr=subprocess.PIPE, timeout=60, text=True,
    ).strip().split()
    if len(value) != 2 or value[1] != "refs/heads/main":
        raise SystemExit("cannot resolve canonical Desktop main")
    return value[0]


def receipt(pinned_commit: str, pinned_tree: str, live_main_before: str,
            checkout_commit: str, checkout_tree: str,
            latest_desktop_main: str) -> dict:
    """Return a fail-closed receipt for three independent commit observations."""
    authority_current = (
        live_main_before == pinned_commit
        and checkout_commit == pinned_commit
        and latest_desktop_main == pinned_commit
        and checkout_tree == pinned_tree
    )
    if live_main_before != pinned_commit:
        raise SystemExit(
            "Desktop receipt rejected: live_main_before does not equal EXPECTED_DESKTOP_COMMIT"
        )
    if checkout_commit != pinned_commit:
        raise SystemExit(
            "Desktop receipt rejected: checkout commit does not equal EXPECTED_DESKTOP_COMMIT"
        )
    if latest_desktop_main != pinned_commit:
        raise SystemExit(
            "Desktop receipt rejected: latest_desktop_main does not equal EXPECTED_DESKTOP_COMMIT"
        )
    if checkout_tree != pinned_tree:
        raise SystemExit(
            "Desktop receipt rejected: checkout tree does not equal EXPECTED_DESKTOP_TREE"
        )
    return {
        "pinnedDesktopCommit": pinned_commit,
        "pinnedDesktopTree": pinned_tree,
        "liveMainBefore": live_main_before,
        "checkoutCommit": checkout_commit,
        "checkoutTree": checkout_tree,
        "latestDesktopMain": latest_desktop_main,
        "authority_current": authority_current,
    }


def inherited_ledger_state(predecessor_row: dict | None, changed: bool) -> dict:
    """Inherit the predecessor status, but never inherit closure across a current-main rebaseline."""
    if changed or not predecessor_row:
        return {
            "implementation_status": "unreviewed",
            "countedAsClosed": False,
            "preserved": False,
        }
    status = predecessor_row.get("implementation_status", "unreviewed")
    if status == "mapped":
        return {
            "implementation_status": "mapped",
            "countedAsClosed": False,
            "preserved": True,
        }
    if status == "unreviewed":
        return {
            "implementation_status": "unreviewed",
            "countedAsClosed": False,
            "preserved": False,
        }
    # implemented/verified/not-applicable are predecessor labels only until current-main
    # owner/contract review and exact-HEAD evidence are refreshed.
    return {
        "implementation_status": status,
        "countedAsClosed": False,
        "preserved": False,
    }


def ledger_requires_status_normalization(ledger_rows: list[dict]) -> bool:
    """Detect inherited closure flags that are stale after current-main rebaseline."""
    for row in ledger_rows:
        inherited = inherited_ledger_state(row, False)
        if row.get("implementation_status", "unreviewed") != inherited["implementation_status"]:
            return True
        if row.get("countedAsClosed") is not inherited["countedAsClosed"]:
            return True
    return False


def carry_forward_current_main_changes(previous_commit: str, expected_commit: str,
                                       previous_delta: dict, previous_impact: dict,
                                       repo: pathlib.Path | None = None,
                                       current_records: dict[str, dict] | None = None,
                                       ledger_by_path: dict[str, dict] | None = None) -> dict:
    """Preserve a register on same-main normalization, reconstructing it if both copies were lost."""
    if previous_commit != expected_commit:
        return {"changedPaths": set(), "addedPaths": set(), "sourceChanges": None,
                "responsibilityActions": None, "impactSourceChanges": None}
    delta_rows = previous_delta.get("sourceChanges", [])
    impact_rows = previous_impact.get("currentSourceChanges", [])
    if not delta_rows and not impact_rows:
        historical = previous_impact.get("historicalPriorImpact", {})
        historical_previous_commit = historical.get("previousCommit")
        if (historical_previous_commit and historical_previous_commit != expected_commit
                and repo is not None and current_records is not None):
            return reconstruct_historical_change_register(
                repo, historical_previous_commit, expected_commit, current_records,
                ledger_by_path or {},
            )
    paths = {row.get("path") for row in delta_rows if row.get("path")}
    added = {row["path"] for row in delta_rows if row.get("path") and row.get("change") == "added"}
    return {
        "changedPaths": paths,
        "addedPaths": added,
        "sourceChanges": copy.deepcopy(delta_rows) if delta_rows else None,
        "responsibilityActions": copy.deepcopy(previous_delta.get("responsibilityActions", [])) if delta_rows else None,
        "impactSourceChanges": copy.deepcopy(impact_rows) if impact_rows else None,
    }


def historical_prior_impact_snapshot(previous_commit: str, expected_commit: str,
                                     previous_impact: dict) -> dict:
    """Retain the original prior-impact pointer across no-op same-main normalization."""
    historical = previous_impact.get("historicalPriorImpact")
    if (previous_commit == expected_commit and isinstance(historical, dict)
            and historical.get("previousCommit")):
        return copy.deepcopy(historical)
    return {
        "previousCommit": previous_impact.get("previousCommit"),
        "currentCommit": previous_impact.get("currentCommit"),
        "fullTree": previous_impact.get("fullTree"),
        "metadataCorrections": previous_impact.get("metadataCorrections", []),
    }


def calculate_preserved_mapped_state(predecessor_ledger: dict[str, dict],
                                     changed_paths: set[str],
                                     current_paths: set[str]) -> dict:
    """Report every predecessor mapped path and demote changed responsibilities."""
    mapped_paths = []
    for path, predecessor_row in sorted(predecessor_ledger.items()):
        if predecessor_row.get("implementation_status") != "mapped" or path not in current_paths:
            continue
        state = inherited_ledger_state(predecessor_row, path in changed_paths)
        mapped_paths.append({
            "path": path,
            "previousImplementationStatus": "mapped",
            "implementation_status": state["implementation_status"],
            "preserved": state["preserved"],
            "countedAsClosed": state["countedAsClosed"] is True,
        })
    owner_state = inherited_ledger_state(
        predecessor_ledger.get(HOST_MAPPED_PATH), HOST_MAPPED_PATH in changed_paths
    )
    return {
        "preservedMappedPaths": mapped_paths,
        "preservedMappedOwnerStatus": owner_state["implementation_status"],
        "preservedMappedCount": sum(1 for row in mapped_paths if row["preserved"]),
        "countedAsClosed": sum(1 for row in mapped_paths if row["countedAsClosed"]),
    }


def stamp_candidate_authority_observation(candidates: dict[str, str],
                                          out: pathlib.Path,
                                          observation: dict) -> None:
    """Stamp every generated JSON candidate with real before/checkout/after observations."""
    before = observation["liveMainBefore"]
    after = observation["latestDesktopMain"]
    for path, content in list(candidates.items()):
        try:
            value = json.loads(content)
        except json.JSONDecodeError:
            continue
        if not isinstance(value, dict):
            continue
        value["desktopMainBefore"] = before
        value["desktopMainAfter"] = after
        value["authority_current"] = observation["authority_current"]
        value["authorityObservation"] = copy.deepcopy(observation)
        value["baselineObservation"] = {
            "desktopMainBefore": before,
            "desktopMainAfter": after,
            "authority_current": observation["authority_current"],
        }
        if path == "manifests/desktop-main-authority.json":
            verification = copy.deepcopy(value.get("iosVerificationRebaseline", {}))
            verification.update({
                "liveMainBefore": before,
                "checkoutCommit": observation["checkoutCommit"],
                "checkoutTree": observation["checkoutTree"],
                "latestDesktopMain": after,
                "authority_current": observation["authority_current"],
            })
            value["iosVerificationRebaseline"] = verification
        elif path == "manifests/desktop-main-reference-index.json":
            authority = copy.deepcopy(value.get("authority", {}))
            authority.update({
                "liveMainBefore": before,
                "checkoutCommit": observation["checkoutCommit"],
                "checkoutTree": observation["checkoutTree"],
                "latestDesktopMain": after,
                "authority_current": observation["authority_current"],
            })
            value["authority"] = authority
        elif path == "docs/parity/desktop-main-index.json":
            source = copy.deepcopy(value.get("source", {}))
            source.update({
                "liveMainBefore": before,
                "checkoutCommit": observation["checkoutCommit"],
                "checkoutTree": observation["checkoutTree"],
                "latestDesktopMain": after,
                "authority_current": observation["authority_current"],
            })
            value["source"] = source
            verification = copy.deepcopy(value.get("iosVerificationRebaseline", {}))
            verification.update({
                "liveMainBefore": before,
                "checkoutCommit": observation["checkoutCommit"],
                "checkoutTree": observation["checkoutTree"],
                "latestDesktopMain": after,
                "authority_current": observation["authority_current"],
            })
            value["iosVerificationRebaseline"] = verification
        elif path == "docs/parity/desktop-main-impact.json":
            value["authorityObservation"] = copy.deepcopy(observation)
        elif path == "docs/parity/desktop-main-release-evidence.json":
            current_head = copy.deepcopy(value.get("currentHeadRebaseline", {}))
            current_head.update({
                "liveMainBefore": before,
                "checkoutCommit": observation["checkoutCommit"],
                "checkoutTree": observation["checkoutTree"],
                "latestDesktopMain": after,
                "authority_current": observation["authority_current"],
                "authorityObservation": copy.deepcopy(observation),
            })
            value["currentHeadRebaseline"] = current_head
        candidates[path] = encode_json(value)
        target = out / "candidate" / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(candidates[path], encoding="utf-8")

    delta_path = "manifests/desktop-main/current-main-delta.json"
    release_path = "docs/parity/desktop-main-release-evidence.json"
    if delta_path in candidates and release_path in candidates:
        delta_digest = sha256_bytes(candidates[delta_path].encode("utf-8"))
        release = json.loads(candidates[release_path])
        current_head = copy.deepcopy(release.get("currentHeadRebaseline", {}))
        current_head["currentMainDeltaSha256"] = delta_digest
        release["currentHeadRebaseline"] = current_head
        candidates[release_path] = encode_json(release)
        target = out / "candidate" / release_path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(candidates[release_path], encoding="utf-8")


def validate_candidate_authority_observation(candidates: dict[str, str],
                                             observation: dict) -> None:
    for path, content in candidates.items():
        value = json.loads(content)
        if value.get("authorityObservation") != observation:
            raise SystemExit("candidate authority observation mismatch: " + path)
        if value.get("desktopMainBefore") != observation["liveMainBefore"]:
            raise SystemExit("candidate desktopMainBefore is not a real observation: " + path)
        if value.get("desktopMainAfter") != observation["latestDesktopMain"]:
            raise SystemExit("candidate desktopMainAfter is not a real observation: " + path)
        if value.get("authority_current") is not observation["authority_current"]:
            raise SystemExit("candidate authority_current mismatch: " + path)
    lock = json.loads(candidates["manifests/desktop-main-authority.json"])
    manifest = json.loads(candidates["manifests/desktop-main-reference-index.json"])
    ledger = json.loads(candidates["docs/parity/desktop-main-index.json"])
    delta = json.loads(candidates["manifests/desktop-main/current-main-delta.json"])
    release = json.loads(candidates["docs/parity/desktop-main-release-evidence.json"])
    pinned = observation["pinnedDesktopCommit"]
    tree = observation["pinnedDesktopTree"]
    if lock.get("commit") != pinned or lock.get("rootTreeSha") != tree:
        raise SystemExit("authority lock does not match pinned Desktop commit/tree")
    if manifest.get("authority", {}).get("commit") != pinned or manifest.get("authority", {}).get("tree") != tree:
        raise SystemExit("manifest authority does not match pinned Desktop commit/tree")
    if ledger.get("source", {}).get("commit") != pinned or ledger.get("source", {}).get("tree") != tree:
        raise SystemExit("ledger authority does not match pinned Desktop commit/tree")
    if delta.get("currentCommit") != pinned or delta.get("currentTree") != tree:
        raise SystemExit("delta authority does not match pinned Desktop commit/tree")
    if release.get("desktopSourceCommit") != pinned or release.get("desktopRootTree") != tree:
        raise SystemExit("release baseline authority does not match pinned Desktop commit/tree")
    criteria = release.get("acceptanceCriteria", [])
    gates = release.get("releaseGates", {})
    if len(criteria) != 21 or any(row.get("status") != "pending" for row in criteria):
        raise SystemExit("all 21 IOS-MAIN-AC criteria must remain pending")
    if any(gate.get("status") != "pending" for gate in gates.values()):
        raise SystemExit("all release gates must remain pending")
    if release.get("currentHeadRebaseline", {}).get("currentMainDeltaSha256") != sha256_bytes(candidates["manifests/desktop-main/current-main-delta.json"].encode("utf-8")):
        raise SystemExit("release evidence delta digest is inconsistent")


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
    inventory = lock.get("inventory", {})
    if lock.get("commit") != EXPECTED_DESKTOP_COMMIT:
        errors.append("authority lock commit is not current Desktop main")
    if lock.get("rootTreeSha") != EXPECTED_DESKTOP_TREE:
        errors.append("authority lock root tree is not current Desktop main")
    actual_gitlinks = sum(1 for row in records.values()
                          if row["type"] == "commit" or row["mode"] == "160000")
    if len(records) != inventory.get("fullTrackedCount"):
        errors.append("full tracked count does not match the authority inventory")
    if len(selected) != inventory.get("selectedFileCount"):
        errors.append("selected count does not match the authority inventory")
    if len(records) - len(selected) != inventory.get("nonselectedCount"):
        errors.append("nonselected count does not match the authority inventory")
    if actual_gitlinks != inventory.get("gitlinkCount"):
        errors.append("gitlink count does not match the authority inventory")
    full_hash, selected_hash = tree_digest(records), tree_digest(selected)
    if full_hash != inventory.get("fullTreeSha256"):
        errors.append("full-tree identity SHA-256 mismatch")
    if selected_hash != inventory.get("selectedTreeSha256"):
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
    delta_changed_paths: set[str] = set()
    delta_path = root / "manifests/desktop-main/current-main-delta.json"
    if not delta_path.is_file():
        errors.append("current-main delta file is missing")
    else:
        delta = read_json(delta_path)
        if delta.get("currentCommit") != EXPECTED_DESKTOP_COMMIT or delta.get("currentTree") != EXPECTED_DESKTOP_TREE:
            errors.append("current-main delta authority does not match the live source lock")
        delta_inventory = delta.get("currentInventory", {})
        for key in ("fullTrackedCount", "selectedFileCount", "nonselectedCount",
                    "gitlinkCount", "fullTreeSha256", "selectedTreeSha256"):
            if delta_inventory.get(key) != inventory.get(key):
                errors.append("current-main delta inventory mismatch: " + key)
        delta_rows = delta.get("sourceChanges", [])
        actual_changes = {row.get("path"): row for row in delta_rows if row.get("path")}
        expected_paths = {row.get("path") for row in delta_rows if row.get("path")}
        delta_changed_paths = expected_paths
        if len(actual_changes) != len(delta_rows):
            errors.append("current-main delta contains duplicate or missing source paths")
        for p in expected_paths:
            row = actual_changes.get(p, {})
            if row.get("candidateStatus") != "unreviewed" or row.get("countedAsClosed") is not False:
                errors.append("current-main delta improperly closes " + p)
            ledger_row = ledger_seen.get(p)
            if ledger_row is None:
                errors.append("current-main delta path is absent from the current ledger: " + p)
            elif ledger_row.get("implementation_status") != "unreviewed" or ledger_row.get("countedAsClosed") is not False:
                errors.append("ledger row is not unreviewed/non-closed: " + p)
    validate_host_mapped_owner_status(ledger_seen, delta_changed_paths, errors)
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


def rebaseline():
    global EXPECTED_DESKTOP_COMMIT, EXPECTED_DESKTOP_TREE
    if len(sys.argv) != 4:
        raise SystemExit("usage: generator.py IOS_ROOT DESKTOP_ROOT OUT_DIR")
    root, desktop, out = map(pathlib.Path, sys.argv[1:])
    out.mkdir(parents=True, exist_ok=True)
    pinned_commit = os.environ.get("EXPECTED_DESKTOP_COMMIT", "").strip()
    pinned_tree = os.environ.get("EXPECTED_DESKTOP_TREE", "").strip()
    if not pinned_commit or not pinned_tree:
        raise SystemExit("EXPECTED_DESKTOP_COMMIT and EXPECTED_DESKTOP_TREE are required workflow pins")
    live_main_before = live_main_sha()
    if live_main_before != pinned_commit:
        raise SystemExit("live_main_before does not equal workflow EXPECTED_DESKTOP_COMMIT: " + live_main_before)
    checked_out_commit = git(desktop, "rev-parse", "HEAD")
    checkout_tree_sha = git(desktop, "rev-parse", "HEAD^{tree}")
    if checked_out_commit != pinned_commit:
        raise SystemExit("Desktop checkout commit does not equal workflow EXPECTED_DESKTOP_COMMIT: " + checked_out_commit)
    if checkout_tree_sha != pinned_tree:
        raise SystemExit("Desktop checkout tree does not equal workflow EXPECTED_DESKTOP_TREE: " + checkout_tree_sha)
    EXPECTED_DESKTOP_COMMIT = pinned_commit
    EXPECTED_DESKTOP_TREE = pinned_tree
    current_tree_sha = checkout_tree_sha
    expected_ios_head = git(root, "rev-parse", "HEAD")
    if os.environ.get("EXPECTED_IOS_HEAD") and expected_ios_head != os.environ["EXPECTED_IOS_HEAD"]:
        raise SystemExit("iOS checkout does not match EXPECTED_IOS_HEAD")

    lock_path = root / "manifests/desktop-main-authority.json"
    manifest_index_path = root / "manifests/desktop-main-reference-index.json"
    ledger_index_path = root / "docs/parity/desktop-main-index.json"
    lock = read_json(lock_path)
    manifest_index = read_json(manifest_index_path)
    ledger_index = read_json(ledger_index_path)
    current_records = git_tree_records(desktop, "HEAD")
    current_selected = selected_records(current_records)
    identity_artifacts = write_identity_files(out, current_records, current_selected)

    full_hash = tree_digest(current_records)
    selected_hash = tree_digest(current_selected)
    if live_main_sha() != EXPECTED_DESKTOP_COMMIT:
        raise SystemExit("Desktop main changed during inventory generation")

    previous_commit = lock.get("commit")
    previous_tree = lock.get("rootTreeSha")
    current_snapshot_result = None
    current_ledger_rows = []
    if previous_commit == EXPECTED_DESKTOP_COMMIT:
        current_snapshot_result = validate_current_snapshot(root, lock, manifest_index, ledger_index,
                                                            current_records, current_selected)
        for group in ledger_index.get("chunks", []):
            current_ledger_rows.extend(read_json(root / group["path"]).get("rows", []))
    if previous_commit == EXPECTED_DESKTOP_COMMIT and not ledger_requires_status_normalization(current_ledger_rows):
        result = current_snapshot_result
        latest_desktop_main = live_main_sha()
        authority_observation = receipt(
            EXPECTED_DESKTOP_COMMIT, EXPECTED_DESKTOP_TREE, live_main_before,
            checked_out_commit, checkout_tree_sha, latest_desktop_main,
        )
        report = {
            "schemaVersion": 2,
            **result,
            "authorityObservation": authority_observation,
            "desktopMainBefore": live_main_before,
            "desktopMainAfter": latest_desktop_main,
            "authority_current": authority_observation["authority_current"],
            "iosHead": expected_ios_head,
            "repository": TARGET_REPO,
            "branch": os.environ.get("GITHUB_HEAD_REF", "spec/grok-bot-018-ios-parity-20260922"),
            "identityArtifacts": identity_artifacts,
            "note": "Snapshot already matches the requested canonical Desktop main; no ref or candidate files were changed.",
        }
        (out / "audit-report.json").write_text(encode_json(report), encoding="utf-8")
        (out / "candidate-files.json").write_text(encode_json({
            "candidateFiles": [],
            "authorityObservation": authority_observation,
            "desktopMainBefore": live_main_before,
            "desktopMainAfter": latest_desktop_main,
            "authority_current": authority_observation["authority_current"],
        }), encoding="utf-8")
        (out / "candidate-rebaseline.diff").write_text("", encoding="utf-8")
        print("CANDIDATE_BLOB_MANIFEST=" + json.dumps({
            "candidateFiles": [],
            "authorityObservation": authority_observation,
            "desktopMainBefore": live_main_before,
            "desktopMainAfter": latest_desktop_main,
            "authority_current": authority_observation["authority_current"],
        }, separators=(",", ":")))
        print("CANDIDATE_AUDIT_SUMMARY=" + json.dumps(report, separators=(",", ":")))
        return

    if not previous_commit or not previous_tree:
        raise SystemExit("authority predecessor is missing an exact commit/tree identity")
    if git(desktop, "rev-parse", previous_commit + "^{tree}") != previous_tree:
        raise SystemExit("previous Desktop commit/tree identity mismatch")
    previous_records = git_tree_records(desktop, previous_commit)
    previous_selected = selected_records(previous_records)
    previous_inventory = lock.get("inventory", {})
    if len(previous_records) != previous_inventory.get("fullTrackedCount") or len(previous_selected) != previous_inventory.get("selectedFileCount"):
        raise SystemExit("predecessor source counts do not match the recorded authority inventory")
    if len(previous_records) - len(previous_selected) != previous_inventory.get("nonselectedCount"):
        raise SystemExit("predecessor nonselected count does not match the recorded authority inventory")
    if tree_digest(previous_records) != previous_inventory.get("fullTreeSha256"):
        raise SystemExit("historical full-tree identity does not match authority lock")
    if tree_digest(previous_selected) != previous_inventory.get("selectedTreeSha256"):
        raise SystemExit("historical selected-tree identity does not match authority lock")
    if manifest_index.get("fileCount") != len(previous_selected) or ledger_index.get("rowCount") != len(previous_selected):
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
        expected_chunk_group = group["name"]
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
    predecessor_host_errors = []
    validate_host_mapped_owner_status(
        {HOST_MAPPED_PATH: old_ledger_by_path.get(HOST_MAPPED_PATH)}, set(), predecessor_host_errors
    )
    if predecessor_host_errors:
        raise SystemExit("invalid predecessor Host owner status: " + "; ".join(predecessor_host_errors))

    raw_changes = parse_name_status(desktop, previous_commit, EXPECTED_DESKTOP_COMMIT)
    changed_paths = set()
    added_paths = set()
    for change in raw_changes:
        path = change.get("path")
        old_path = change.get("oldPath")
        status = change.get("status", "")
        if old_path and old_path in previous_selected and old_path not in current_selected:
            raise SystemExit("selected path rename requires a separately reviewed rebaseline: " + old_path)
        if path in current_selected:
            if path not in previous_selected:
                if not status.startswith("A"):
                    raise SystemExit("new selected path has unsupported Git change status: " + path + " " + status)
                added_paths.add(path)
            elif status.startswith("M"):
                changed_paths.add(path)
            else:
                raise SystemExit("selected path has unsupported Git change status: " + path + " " + status)
    removed_selected = set(previous_selected) - set(current_selected)
    if removed_selected:
        raise SystemExit("selected path deletion requires a separately reviewed rebaseline: "
                         + sorted(removed_selected)[0])
    changed_paths.update(added_paths)
    previous_delta = read_json(root / "manifests/desktop-main/current-main-delta.json")
    previous_impact = read_json(root / "docs/parity/desktop-main-impact.json")
    carry_forward = carry_forward_current_main_changes(
        previous_commit, EXPECTED_DESKTOP_COMMIT, previous_delta, previous_impact,
        repo=desktop, current_records=current_records, ledger_by_path=old_ledger_by_path,
    )
    if carry_forward["changedPaths"]:
        changed_paths.update(carry_forward["changedPaths"])
        added_paths.update(carry_forward["addedPaths"])
    preserved_state = calculate_preserved_mapped_state(
        old_ledger_by_path, changed_paths, set(current_selected)
    )

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
            manifest_group_name = classify_new_path_manifest_group(path)
        ledger_group_name = ledger_group_by_path.get(path, manifest_group_name)
        if manifest_group_name not in groups or ledger_group_name not in ledger_groups:
            raise SystemExit("selected path has no paired manifest/ledger group: " + path)
        current_group_for_path[path] = manifest_group_name
        current_ledger_group_for_path[path] = ledger_group_name

    for path in set(previous_selected) - set(current_selected):
        raise SystemExit("selected path deletion requires a separately reviewed rebaseline: " + path)

    current_manifest_rows = {name: [] for name in groups}
    current_ledger_rows = {name: [] for name in ledger_groups}
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
            change_note = "Current Desktop-main blob changed/added; predecessor status and evidence are historical and non-promotable until explicit current-main review."
            if change_note not in prior_notes:
                row["notes"] = (prior_notes + " " + change_note).strip()
            for key in ("workflow_run", "run_attempt", "job", "artifact_id", "artifact_digest",
                        "ios_target_commit", "desktop_review_commit", "independent_reviewer",
                        "exact_head_evidence"):
                row.pop(key, None)
            row["production_evidence"] = ""
            row["test_evidence"] = ""
        else:
            inherited = inherited_ledger_state(old_ledger_by_path.get(path), False)
            row["implementation_status"] = inherited["implementation_status"]
            if inherited["countedAsClosed"] is not None:
                row["countedAsClosed"] = inherited["countedAsClosed"]
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
    if carry_forward["sourceChanges"] is not None:
        source_change_rows = carry_forward["sourceChanges"]
        responsibility_actions = carry_forward["responsibilityActions"] or []
    else:
        for path in sorted(changed_paths):
            current = current_records[path]
            previous = previous_records.get(path)
            old_row = old_ledger_by_path.get(path)
            status = "added" if path in added_paths else "modified"
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
    if len(outside["entries"]) != current_nonselected_count:
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
        "reason": f"Current Desktop main changed {len(changed_paths)} selected frontend/source paths. All changed/new rows require explicit owner, contract, production and test review; no row is counted closed.",
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
    impact["currentSourceChanges"] = (
        carry_forward["impactSourceChanges"]
        if carry_forward["impactSourceChanges"] is not None
        else source_change_rows
    )
    impact["historicalPriorImpact"] = historical_prior_impact_snapshot(
        previous_commit, EXPECTED_DESKTOP_COMMIT, old_impact
    )
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
        "preservedMappedPaths": preserved_state["preservedMappedPaths"],
        "preservedMappedOwnerStatus": preserved_state["preservedMappedOwnerStatus"],
        "preservedMappedCount": preserved_state["preservedMappedCount"],
        "countedAsClosed": preserved_state["countedAsClosed"],
        "newSourceFiles": [
            {"path": path, "blobSha": current_records[path]["sha"], "size": current_records[path]["size"],
             "mode": current_records[path]["mode"], "type": current_records[path]["type"],
             "implementation_status": "unreviewed", "countedAsClosed": False}
            for path in sorted(added_paths)
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
    expected_host_status = preserved_state["preservedMappedOwnerStatus"]
    if candidate_ledger_paths[HOST_MAPPED_PATH].get("implementation_status") != expected_host_status:
        raise SystemExit("candidate host entrypoint status does not match predecessor status and changed paths")
    expected_host_closed = inherited_ledger_state(
        old_ledger_by_path[HOST_MAPPED_PATH], HOST_MAPPED_PATH in changed_paths
    )["countedAsClosed"]
    if candidate_ledger_paths[HOST_MAPPED_PATH].get("countedAsClosed") is not expected_host_closed:
        raise SystemExit("candidate host countedAsClosed does not match predecessor status and changed paths")
    if sum(g["fileCount"] for g in groups.values()) != current_selected_count:
        raise SystemExit("candidate manifest group counts do not sum to current selected count")
    if sum(g["rowCount"] for g in ledger_groups.values()) != current_selected_count:
        raise SystemExit("candidate ledger group counts do not sum to current selected count")
    latest_desktop_main = live_main_sha()
    authority_observation = receipt(
        EXPECTED_DESKTOP_COMMIT, EXPECTED_DESKTOP_TREE, live_main_before,
        checked_out_commit, checkout_tree_sha, latest_desktop_main,
    )
    stamp_candidate_authority_observation(candidates, out, authority_observation)
    validate_candidate_authority_observation(candidates, authority_observation)

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
                     "candidateTreeSha": candidate_tree_sha, "candidateFiles": candidate_file_records,
                     "authorityObservation": authority_observation,
                     "desktopMainBefore": live_main_before, "desktopMainAfter": latest_desktop_main,
                     "authority_current": authority_observation["authority_current"]}),
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
        "preservedMappedOwnerStatus": preserved_state["preservedMappedOwnerStatus"],
        "preservedMappedPaths": preserved_state["preservedMappedPaths"],
        "preservedMappedCount": preserved_state["preservedMappedCount"],
        "countedAsClosed": preserved_state["countedAsClosed"],
        "previouslyMappedHostPathChanged": HOST_MAPPED_PATH in changed_paths,
        "authorityObservation": authority_observation,
        "desktopMainBefore": live_main_before,
        "desktopMainAfter": latest_desktop_main,
        "authority_current": authority_observation["authority_current"],
        "acceptanceCriteriaCount": 21,
        "pendingAcceptanceCriteriaCount": 21,
        "releaseGateCount": len(json.loads(candidates["docs/parity/desktop-main-release-evidence.json"]).get("releaseGates", {})),
        "pendingReleaseGateCount": len(json.loads(candidates["docs/parity/desktop-main-release-evidence.json"]).get("releaseGates", {})),
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
        "authorityObservation": authority_observation,
        "desktopMainBefore": live_main_before,
        "desktopMainAfter": latest_desktop_main,
        "authority_current": authority_observation["authority_current"],
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
        "preservedMappedCount": report["preservedMappedCount"],
        "countedAsClosed": report["countedAsClosed"],
        "authorityObservation": authority_observation,
        "authority_current": authority_observation["authority_current"],
        "finalAcceptance": False,
    }, separators=(",", ":")))


def main():
    rebaseline()


if __name__ == "__main__":
    main()
