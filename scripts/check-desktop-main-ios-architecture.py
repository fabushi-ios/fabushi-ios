#!/usr/bin/env python3
"""Fail-closed live Desktop-main source/ownership authority validation for Fabushi iOS.

This validates the recorded snapshot against the actual pinned Git tree and the
live Desktop main ref in GitHub Actions. No PR20 SHA or file count is hardcoded.
--complete is intentionally stronger than ordinary inventory validation.
"""
import argparse
import hashlib
import importlib.util
import json
import os
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / "manifests/desktop-main-authority.json"
MANIFEST_INDEX = ROOT / "manifests/desktop-main-reference-index.json"
LEDGER_INDEX = ROOT / "docs/parity/desktop-main-index.json"
ACTIVE_SPEC = ROOT / "docs/specs/fabushi-desktop-main-ios-parity.md"
HISTORICAL_SPEC = ROOT / "docs/specs/fabushi-desktop-pr20-ios-architecture-parity.md"
UPSTREAM_URL = "https://github.com/bhrumom/fabushi-desktop.git"
HEX40 = re.compile(r"^[0-9a-f]{40}$")
DISPOSITIONS = {"unreviewed", "direct-port", "ios-adapted", "not-applicable-with-replacement"}
STATUSES = {"unreviewed", "understood", "mapped", "implemented", "verified", "not-applicable"}

def error_source_group(message):
    """Classify a fail-closed diagnostic by upstream source group."""
    path = message.split(":", 1)[0]
    if path.startswith("frontend/"):
        return "frontend"
    if path.startswith("source/host/"):
        return "source-host"
    if path.startswith("source/mahayana/"):
        return "source-mahayana"
    if path.startswith("source/packages/"):
        return "source-packages"
    if path.startswith("source/"):
        return "source-other"
    if path.startswith(("docs/", "manifests/")):
        return "authority-ledger"
    return "global"


def error_category(message):
    """Classify diagnostics for reporting only; validation remains fail-closed."""
    rules = (
        ("incomplete-mandatory-status", "incomplete mandatory responsibility status="),
        ("missing-responsibility-owner", "reviewed row missing a real responsibility"),
        ("missing-production-evidence", "missing production evidence"),
        ("missing-focused-test-evidence", "verified missing test evidence"),
        ("missing-owner-contract-review", "final source owner/contract revalidation not documented"),
        ("missing-exact-head-field", "verified lacks current-head evidence field"),
        ("stale-desktop-main-evidence", "verified source review not bound to current Desktop main"),
        ("stale-ios-head-evidence", "verified evidence not bound to current iOS HEAD"),
        ("changed-source-review-fence", "not re-review-fenced"),
        ("missing-ios-target", "actual iOS source file"),
        ("unreviewed-nonselected-source", "nonselected tracked item unreviewed"),
        ("missing-release-evidence", "release acceptance/evidence"),
        ("dependency-review-pending", "dependency review"),
        ("missing-field-or-reference", "missing "),
    )
    for category, needle in rules:
        if needle in message:
            return category
    return "other"


def summarize_errors(errors):
    """Return grouped diagnostics without suppressing or changing validation errors."""
    groups = {}
    details = {}
    for message in errors:
        group = error_source_group(message)
        category = error_category(message)
        groups[group] = groups.get(group, 0) + 1
        key = (group, category)
        details[key] = details.get(key, 0) + 1
    return {"total": len(errors), "groups": groups, "details": details}


def print_error_summary(errors):
    summary = summarize_errors(errors)
    print(f"ERROR_TOTAL {summary['total']}")
    for group, count in sorted(summary["groups"].items()):
        print(f"ERROR_GROUP {group}={count}")
    for (group, category), count in sorted(summary["details"].items()):
        print(f"ERROR_DETAIL {group}/{category}={count}")




def document(path):
    return json.loads(path.read_text(encoding="utf-8"))


def tree_digest(records):
    """SHA-256 of canonical full Git tracked identities (including symlink/gitlink)."""
    stream = "".join(
        f"{path}\0{mode}\0{kind}\0{sha}\0{size if size is not None else '-'}\n"
        for path, (mode, kind, sha, size) in sorted(records.items())
    )
    return hashlib.sha256(stream.encode("utf-8")).hexdigest()


def git_tree(checkout):
    raw = subprocess.check_output(
        ["git", "ls-tree", "-r", "-l", "-z", "HEAD"], cwd=checkout
    )
    result = {}
    for line in raw.split(b"\0"):
        if not line:
            continue
        identity, name = line.split(b"\t", 1)
        mode, kind, sha, size = identity.decode("ascii").split()
        path = name.decode("utf-8")
        if path in result:
            raise ValueError(f"duplicate Git tree identity: {path}")
        result[path] = (mode, kind, sha, None if size == "-" else int(size))
    return result


def live_head():
    result = subprocess.check_output(
        ["git", "ls-remote", "--heads", UPSTREAM_URL, "refs/heads/main"],
        stderr=subprocess.PIPE,
        timeout=60,
    ).decode("ascii", "strict").strip()
    if not result:
        raise ValueError("Desktop main ref could not be resolved")
    sha, ref = result.split()
    if ref != "refs/heads/main" or not HEX40.fullmatch(sha):
        raise ValueError(f"unexpected Desktop main ref: {result!r}")
    return sha


def check_status(row, path, complete, errors, warnings, locked_commit):
    status = (row.get("implementation_status") or "").strip()
    disposition = (row.get("ios_disposition") or "").strip()
    target = (row.get("ios_target_path") or "").strip()
    if status not in STATUSES:
        errors.append(f"{path}: invalid status: {status!r}")
    if disposition not in DISPOSITIONS:
        errors.append(f"{path}: invalid disposition: {disposition!r}")
    if status in {"mapped", "implemented", "verified", "not-applicable"}:
        if row.get("desktop_responsibility") in (None, "", "pending-review"):
            errors.append(f"{path}: reviewed row missing a real responsibility")
        if row.get("desktop_visible_effect") in (None, "", "pending-review"):
            errors.append(f"{path}: reviewed row missing a product effect")
    if status in {"implemented", "verified"}:
        if not target or not (ROOT / target).is_file():
            errors.append(f"{path}: {status} has no actual iOS source file: {target}")
        if not (row.get("production_evidence") or "").strip():
            errors.append(f"{path}: {status} missing production evidence")
    if status == "verified" and not (row.get("test_evidence") or "").strip():
        errors.append(f"{path}: verified missing test evidence")
    if status == "not-applicable":
        if disposition != "not-applicable-with-replacement":
            errors.append(f"{path}: N/A status without replacement disposition")
        if not (row.get("ios_platform_delta") or "").strip():
            errors.append(f"{path}: N/A without platform and product-effect replacement")
    if row.get("authority_review_state") in {
        "pending", "requires-current-main-review", "requires-main-responsibility-and-owner-review"
    } and status in {"verified", "implemented"}:
        errors.append(f"{path}: impacted main row improperly promoted to {status}")

    if complete:
        if status not in {"verified", "not-applicable"}:
            errors.append(f"{path}: incomplete mandatory responsibility status={status}")
        if row.get("authority_review_state") not in {"current-main-reviewed", "current-main-verified"}:
            errors.append(f"{path}: final source owner/contract revalidation not documented")
        if status == "verified":
            for field in (
                "responsibility_id", "ios_target_symbol", "production_entrypoint",
                "oracle_ids", "ios_target_commit", "workflow_run", "run_attempt",
                "job", "artifact_id", "artifact_digest", "independent_reviewer",
            ):
                if not row.get(field):
                    errors.append(f"{path}: verified lacks current-head evidence field {field}")
            if row.get("desktop_review_commit") != locked_commit:
                errors.append(f"{path}: verified source review not bound to current Desktop main")
            if row.get("ios_target_commit") != os.environ.get("IOS_EXPECTED_HEAD"):
                errors.append(f"{path}: verified evidence not bound to current iOS HEAD")
    elif status in {"verified", "not-applicable"} and row.get("desktop_review_commit") != locked_commit:
        warnings.append(f"{path}: legacy {status} is HISTORICAL, not current-main verified")


def verify_snapshot(lock, manifest_index, ledger_index, errors):
    if lock.get("schemaVersion") != 2 or lock.get("authorityMode") != "live-main":
        errors.append("missing v2 live-main authority lock")
    sha, tree = lock.get("commit"), lock.get("rootTreeSha")
    if not HEX40.fullmatch(sha or "") or not HEX40.fullmatch(tree or ""):
        errors.append("authority lock missing exact 40-hex commit and root tree")
    if lock.get("repository") != "bhrumom/fabushi-desktop" or lock.get("branch") != "main":
        errors.append("authority lock is not bhrumom/fabushi-desktop@main")
    inventory = lock.get("inventory", {})
    if inventory.get("selectedRoots") != ["frontend/**", "source/**"]:
        errors.append("selected source roots drift")
    for key, value in (("manifestIndex", "manifests/desktop-main-reference-index.json"),
                       ("ledgerIndex", "docs/parity/desktop-main-index.json"),
                       ("nonselectedRegister", "manifests/desktop-main/tracked-outside-selected.json"),
                       ("impactRegister", "docs/parity/desktop-main-impact.json")):
        if inventory.get(key) != value:
            errors.append(f"authority lock file route drift: {key}")
    for value, idx in (("manifest", manifest_index), ("ledger", ledger_index)):
        source = idx.get("authority" if value == "manifest" else "source", {})
        for k, expected in (("repository", lock.get("repository")), ("branch", "main"),
                            ("commit", sha), ("tree", tree), ("mode", "live-main")):
            if source.get(k) != expected:
                errors.append(f"{value} authority drift: {k}={source.get(k)!r}; expected {expected!r}")
    if manifest_index.get("fileCount") != inventory.get("selectedFileCount"):
        errors.append("manifest row count does not agree with authority lock")
    if ledger_index.get("rowCount") != inventory.get("selectedFileCount"):
        errors.append("ledger row count does not agree with authority lock")
    if manifest_index.get("selectedTreeSha256") != inventory.get("selectedTreeSha256"):
        errors.append("manifest inventory digest differs from authority lock")
    for file in [ACTIVE_SPEC, HISTORICAL_SPEC, ROOT / "MIGRATION_SOURCE.md", ROOT / "AGENTS.md"]:
        if not file.is_file():
            errors.append(f"missing canonical authority document: {file}")
    if ACTIVE_SPEC.is_file():
        active = ACTIVE_SPEC.read_text(encoding="utf-8")
        for token in ["Desktop main", "IOS-MAIN-AUTH-01", "IOS-MAIN-AC-21"]:
            if token not in active:
                errors.append(f"canonical iOS Spec missing normative token {token}")



def verify_current_source_changes(impact, recorded, ledgers, errors, actual_tree, allow_empty_candidate_register=False):
    """Bind each current Desktop-main change to manifest, ledger and exact Git identity.

    Freshly changed/new rows are allowed to remain explicitly unreviewed in strict
    architecture checks. Rich owner/target/evidence requirements apply only after
    the row has been reviewed/mapped; --complete still requires every mandatory row
    to be verified or an accepted native replacement.
    """
    changes = impact.get("currentSourceChanges")
    if not isinstance(changes, list) or not changes:
        if allow_empty_candidate_register and changes == []:
            # Candidate mode reconstructs the full delta from exact predecessor/live Git trees.
            return
        errors.append("impact register has no currentSourceChanges rows")
        return
    group_summary = impact.get("groupSummary", {})
    expected_count = sum(
        group.get("changedOrAdded", 0)
        for group in group_summary.values()
        if isinstance(group, dict)
    )
    if expected_count != len(changes):
        errors.append(f"currentSourceChanges count mismatch: {len(changes)} != {expected_count}")
    seen_paths, seen_responsibilities = set(), set()
    fenced_states = {"pending", "requires-current-main-review", "requires-main-responsibility-and-owner-review"}
    reviewed_states = {"current-main-reviewed", "current-main-verified", "reviewed-current-main"}
    for change in changes:
        path = change.get("path")
        if not path or path in seen_paths:
            errors.append(f"currentSourceChanges has missing or duplicate path: {path!r}")
            continue
        seen_paths.add(path)
        manifest, row = recorded.get(path), ledgers.get(path)
        if manifest is None:
            errors.append(f"{path}: absent from source manifest")
        if row is None:
            errors.append(f"{path}: absent from responsibility ledger")
        if manifest is not None:
            if change.get("blobSha") != manifest.get("blobSha") or change.get("size") != manifest.get("size"):
                errors.append(f"{path}: blob SHA or size differs from source manifest")
        if row is not None:
            responsibility_id = row.get("responsibility_id")
            changed_responsibility_id = change.get("responsibility_id")
            if changed_responsibility_id is not None and changed_responsibility_id != responsibility_id:
                errors.append(f"{path}: responsibility ID differs from ledger")
            if responsibility_id:
                if responsibility_id in seen_responsibilities:
                    errors.append(f"{path}: duplicate responsibility ID {responsibility_id}")
                else:
                    seen_responsibilities.add(responsibility_id)
            if row.get("desktop_blob_sha") != change.get("blobSha"):
                errors.append(f"{path}: blob SHA differs from responsibility ledger")
            status = row.get("implementation_status")
            if status not in {"unreviewed", "mapped"}:
                errors.append(f"{path}: changed source improperly promoted to {status}")
            if change.get("implementation_status") not in {"unreviewed", "mapped"}:
                errors.append(f"{path}: changed source improperly promoted to {change.get('implementation_status')}")
            if change.get("implementation_status") != status or change.get("candidateStatus") != status:
                errors.append(f"{path}: status differs from responsibility ledger")
            if change.get("countedAsClosed") is not False or row.get("countedAsClosed") is not False:
                errors.append(f"{path}: must remain countedAsClosed=false")
            owner_state = row.get("authority_review_state")
            if owner_state not in fenced_states | reviewed_states:
                errors.append(f"{path}: owner/contract review state is invalid: {owner_state}")
            changed_owner_state = change.get("owner_review_state")
            if changed_owner_state is not None and changed_owner_state != owner_state:
                errors.append(f"{path}: owner-review state differs from ledger")
            reviewed = status == "mapped" or owner_state in reviewed_states
            if reviewed:
                if change.get("ios_disposition") != row.get("ios_disposition"):
                    errors.append(f"{path}: iOS disposition differs from responsibility ledger")
                for field in ("desktop_owner","desktop_responsibility","desktop_visible_effect","ios_owner","ios_target_path","ios_language","ios_runtime_entrypoint","ios_platform_delta","production_entrypoint","ios_native_replacement","focused_tests","open_items","is_production_feature"):
                    if change.get(field) != row.get(field):
                        errors.append(f"{path}: {field} differs from responsibility ledger")
                for field in ("desktop_owner","desktop_responsibility","desktop_visible_effect","ios_owner","ios_target_path","production_entrypoint","ios_native_replacement"):
                    if not isinstance(change.get(field), str) or not change[field].strip():
                        errors.append(f"{path}: missing {field}")
                for field in ("focused_tests","open_items"):
                    value = change.get(field)
                    if not isinstance(value, list) or not value or any(not isinstance(item,str) or not item.strip() for item in value):
                        errors.append(f"{path}: missing auditable {field}")
                if change.get("ios_target_path") and not (ROOT / change["ios_target_path"]).is_file():
                    errors.append(f"{path}: iOS target is not a real source file")
            else:
                if status != "unreviewed" or owner_state not in fenced_states:
                    errors.append(f"{path}: fresh current-main row must remain explicitly unreviewed and fenced")
                for field in ("production_evidence","test_evidence","exact_head_evidence","ios_target_commit","workflow_run","artifact_id","artifact_digest"):
                    if row.get(field):
                        errors.append(f"{path}: unreviewed current-main row carries promotable evidence field {field}")
            if "/tests/" in path and (status not in {"unreviewed","mapped"} or change.get("is_production_feature") is True):
                errors.append(f"{path}: test-only path cannot be counted as production")
        if actual_tree is not None:
            if actual_tree.get(path) != (change.get("mode"),change.get("type"),change.get("blobSha"),change.get("size")):
                errors.append(f"{path}: mode/type/blob SHA/size differs from exact Desktop Git tree")
    if actual_tree is None:
        errors.append("currentSourceChanges Git identity requires a pinned Desktop checkout")


def verify_files(lock, manifest_index, ledger_index, errors, warnings, complete, checkout, allow_empty_current_source_changes=False):
    source_sha = lock.get("commit")
    recorded = {}
    for group in manifest_index.get("groups", []):
        path = ROOT / group["path"]
        if not path.is_file():
            errors.append(f"missing Desktop-main manifest: {group['path']}")
            continue
        block = document(path)
        rows = block.get("files", [])
        if block.get("fileCount") != len(rows) or group.get("fileCount") != len(rows):
            errors.append(f"manifest chunk row count mismatch: {group['path']}")
        if not HEX40.fullmatch(block.get("sourceCommit") or ""):
            errors.append(f"missing sourceCommit provenance: {group['path']}")
        for row in rows:
            p = row.get("path")
            if p in recorded:
                errors.append(f"duplicate manifest source path {p}")
            recorded[p] = row
    ledgers = {}
    statuses, dispositions = Counter(), Counter()
    for group in ledger_index.get("chunks", []):
        path = ROOT / group["path"]
        if not path.is_file():
            errors.append(f"missing Desktop-main parity ledger: {group['path']}")
            continue
        block = document(path)
        rows = block.get("rows", [])
        if block.get("rowCount") != len(rows) or group.get("rowCount") != len(rows):
            errors.append(f"ledger chunk row count mismatch: {group['path']}")
        if not HEX40.fullmatch(block.get("sourceCommit") or ""):
            errors.append(f"missing ledger sourceCommit provenance: {group['path']}")
        for row in rows:
            p = row.get("desktop_path")
            if p in ledgers:
                errors.append(f"duplicate ledger Desktop source path {p}")
            ledgers[p] = row
            statuses[row.get("implementation_status")] += 1
            dispositions[row.get("ios_disposition")] += 1
            check_status(row, p, complete, errors, warnings, source_sha)
    if len(recorded) != lock["inventory"]["selectedFileCount"]:
        errors.append(f"selected source count mismatch: {len(recorded)}")
    if len(ledgers) != lock["inventory"]["selectedFileCount"]:
        errors.append(f"selected parity row count mismatch: {len(ledgers)}")
    if set(recorded) != set(ledgers):
        errors.append(f"manifest/ledger paths drift: missing={list(set(recorded)-set(ledgers))[:5]} extra={list(set(ledgers)-set(recorded))[:5]}")
    for path, row in recorded.items():
        if ledgers.get(path, {}).get("desktop_blob_sha") != row.get("blobSha"):
            errors.append(f"{path}: manifest/ledger Git blob mismatch")
        if not (path.startswith("frontend/") or path.startswith("source/")):
            errors.append(f"{path}: selected root contamination")
        if not HEX40.fullmatch(row.get("blobSha") or ""):
            errors.append(f"{path}: invalid exact Git blob SHA")

    unselected = document(ROOT / lock["inventory"]["nonselectedRegister"])
    outside = {}
    if unselected.get("count") != len(unselected.get("entries", [])):
        errors.append("nonselected tracked entry count mismatch")
    for r in unselected.get("entries", []):
        p = r.get("path")
        if p in outside:
            errors.append(f"duplicate nonselected path: {p}")
        outside[p] = r
        if p in recorded or p.startswith("frontend/") or p.startswith("source/"):
            errors.append(f"nonselected register contains selected path: {p}")
    if len(outside) != lock["inventory"]["nonselectedCount"]:
        errors.append("nonselected tracked count differs from authority lock")

    impact = document(ROOT / lock["inventory"]["impactRegister"])
    if impact.get("currentCommit") != source_sha or impact.get("fullTree") != lock.get("rootTreeSha"):
        errors.append("impact register is not bound to current main")
    if impact.get("directCount") != len([x for x in impact.get("directChanges", []) if x["change"] != "metadata-corrected"]):
        errors.append("impact direct-change count inconsistent")
    for change in impact.get("directChanges", []):
        if change["change"] in {"added", "blob-changed"}:
            row = ledgers.get(change["path"])
            if row is None or row.get("authority_review_state") not in {
                "pending", "requires-current-main-review", "requires-main-responsibility-and-owner-review"
            }:
                errors.append(f"changed source not re-review-fenced: {change['path']}")
            if row and row.get("implementation_status") in {"verified", "implemented"}:
                errors.append(f"changed source incorrectly accepted: {change['path']}")

    if checkout is not None:
        actual_head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=checkout).decode().strip()
        actual_tree = subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], cwd=checkout).decode().strip()
        if actual_head != source_sha or actual_tree != lock["rootTreeSha"]:
            errors.append(f"Desktop checkout identity drift: commit={actual_head} tree={actual_tree}")
        real = git_tree(checkout)
        if len(real) != lock["inventory"]["fullTrackedCount"]:
            errors.append(f"full tracked Desktop tree count drift: {len(real)}")
        selected = {p:x for p,x in real.items() if p.startswith("frontend/") or p.startswith("source/")}
        if len(selected) != lock["inventory"]["selectedFileCount"]:
            errors.append(f"selected Desktop tree count drift: {len(selected)}")
        if tree_digest(real) != lock["inventory"]["fullTreeSha256"]:
            errors.append("full tracked Desktop root tree inventory SHA256 drift")
        if tree_digest(selected) != lock["inventory"]["selectedTreeSha256"]:
            errors.append("selected Desktop source inventory SHA256 drift")
        if set(real) != set(recorded) | set(outside):
            errors.append("full Desktop tree has missing/unexpected tracked paths")
        for path, row in recorded.items():
            got=real.get(path)
            if not got or got[1] != "blob" or got[2] != row["blobSha"] or got[3] != row.get("size"):
                errors.append(f"{path}: blob/size does not match live main exact Git tree")
        for path, row in outside.items():
            got=real.get(path)
            if not got or got != (row.get("mode"), row.get("type"), row.get("sha"), row.get("size")):
                errors.append(f"{path}: nonselected Git identity differs from live main")
    verify_current_source_changes(
        impact, recorded, ledgers, errors, real if checkout is not None else None,
        allow_empty_candidate_register=allow_empty_current_source_changes,
    )

    release_path = ROOT / "docs/parity/desktop-main-release-evidence.json"
    if not release_path.is_file():
        errors.append("missing durable current-main release acceptance/evidence matrix")
        release = {}
    else:
        release = document(release_path)
        if release.get("desktopSourceCommit") != source_sha or release.get("desktopRootTree") != lock.get("rootTreeSha"):
            errors.append("release acceptance matrix is not bound to current Desktop main")
        expected_ac={f"IOS-MAIN-AC-{i:02d}" for i in range(1,22)}
        entries=release.get("acceptanceCriteria", [])
        actual_ac=[r.get("id") for r in entries]
        if len(actual_ac)!=21 or set(actual_ac)!=expected_ac:
            errors.append("release matrix must contain exactly 21 stable acceptance criterion IDs")
        for r in entries:
            if r.get("status")=="passed":
                if not r.get("evidence") or not r.get("independentReviewer"):
                    errors.append(f"{r.get('id')}: passed claim lacks evidence or independent reviewer")
                if r.get("iosCommit") != os.environ.get("IOS_EXPECTED_HEAD"):
                    errors.append(f"{r.get('id')}: passed claim not tied to current iOS exact head")
        for key,gate in release.get("releaseGates", {}).items():
            if gate.get("status")=="passed":
                required=["iosCommit","desktopCommit","workflowRun","runAttempt","job","requiredSteps","artifactId","artifactDigest","independentReviewer"]
                for field in required:
                    if not gate.get(field):
                        errors.append(f"{key}: passed release gate lacks {field}")
                if gate.get("desktopCommit") != source_sha or gate.get("iosCommit") != os.environ.get("IOS_EXPECTED_HEAD"):
                    errors.append(f"{key}: passed release gate source identity mismatch")
                if not re.fullmatch(r"sha256:[0-9a-f]{64}",gate.get("artifactDigest") or ""):
                    errors.append(f"{key}: passed release artifact digest not sha256")
                if gate.get("requiredSteps") and any(step.get("conclusion")!="success" or step.get("status")!="completed" for step in gate["requiredSteps"]):
                    errors.append(f"{key}: passed release gate has missing, skipped or failed required step")
        if complete:
            if release.get("finalAcceptance") is not True:
                errors.append("final independent iOS release acceptance not recorded")
            for r in entries:
                if r.get("status")!="passed":
                    errors.append(f"{r.get('id')}: mandatory release criterion not accepted")
            for name,gate in release.get("releaseGates", {}).items():
                if gate.get("status")!="passed":
                    errors.append(f"release gate pending/blocked: {name}")

    if complete:
        for key in ("baselineReady", "dependenciesReviewed", "completenessAccepted", "acceptanceAccepted"):
            if lock.get(key) is not True:
                errors.append(f"release gate not closed: {key}")
        if impact.get("fullReviewAccepted") is not True or impact.get("dependencyReview", {}).get("status") != "verified":
            errors.append("upstream changed-owner/protocol impact review not closed")
        for row in outside.values():
            if row.get("coverage_status") not in {"verified", "non-runtime-accounted"}:
                errors.append(f"nonselected tracked item unreviewed: {row.get('path')}")
        if checkout is None:
            errors.append("--complete requires a pinned actual Desktop Git checkout")
    return statuses, dispositions


def verify_ios_owners(errors):
    """Keep preexisting shipping-path architecture regression fences intact."""
    required_roots = [
        "frontend",
        "source/ios-main",
        "source/ios-preload",
        "source/ios-dev-controls",
        "source/mahayana-agent-coordinator",
        "source/host",
        "source/local-exec-daemon",
        "source/box-exec-daemon",
        "source/internal",
        "source/packages",
        "source/shared",
    ]
    for relative in required_roots:
        if not (ROOT / relative).is_dir():
            errors.append(f"missing iOS architecture root: {relative}")

    if not (ROOT / "mobile/ios/Fabushi/FabushiApp.swift").is_file():
        errors.append("missing iOS application bootstrap")

    # The iOS product must remain standalone. These are source/build dependency checks,
    # not documentation/provenance string bans.
    dependency_files = [
        ROOT / "mobile/ios/project.yml",
        ROOT / "source/packages/mahayana-rs/Cargo.toml",
    ]
    for dependency_file in dependency_files:
        if not dependency_file.is_file():
            continue
        text = dependency_file.read_text(encoding="utf-8")
        for forbidden in [
            "bhrumom/fabushi-platform-core",
            "../fabushi-platform-core",
            "../fabushi-desktop",
            "github.com/bhrumom/fabushi-desktop",
        ]:
            if forbidden in text:
                errors.append(
                    f"{dependency_file.relative_to(ROOT)} introduces forbidden cross-repo runtime dependency: {forbidden}"
                )

    # Preserve the shipping ownership chain established by PR #3 while it is re-audited.
    preload = ROOT / "source/ios-preload/preload.swift"
    if preload.is_file():
        text = preload.read_text(encoding="utf-8")
        if "IOSCoordinatorPortClient" not in text:
            errors.append("iOS preload does not use its renderer-facing coordinator-port client")
        for forbidden in ["CoordinatorControlPortClient", "main.dispatch("]:
            if forbidden in text:
                errors.append(f"iOS preload bypasses Coordinator ownership: {forbidden}")

    app = ROOT / "mobile/ios/Fabushi/FabushiApp.swift"
    if app.is_file():
        text = app.read_text(encoding="utf-8")
        for forbidden in ["MahayanaHost(", "MahayanaCoordinator("]:
            if forbidden in text:
                errors.append(f"FabushiApp.swift directly constructs runtime owner: {forbidden}")


    # Desktop packaged acceptance reads one canonical agent-avatar responsibility.
    # iOS owns its platform-adapted dispatcher natively: custom image -> shared room ->
    # group -> persona. Shipping Agent surfaces must call MobileAgentAvatar rather than
    # bypassing that dispatcher through the legacy decorative ClothGhostAvatar.
    avatar_path = ROOT / "frontend/src/recovered/features/agent-info/ClothGhostAvatar.swift"
    summary_path = ROOT / "frontend/src/recovered/features/agent-info/MobileBotSummary.swift"
    bot_service_path = ROOT / "frontend/src/production/GrokMobileBotService.swift"
    bot_chat_path = ROOT / "frontend/src/recovered/features/conversation/MobileBotChat.swift"
    group_members_path = ROOT / "frontend/src/production/MobileBotGroupMembers.swift"
    home_path = ROOT / "frontend/src/production/GrokMobileShell+Home.swift"

    if not avatar_path.is_file():
        errors.append("missing canonical native Agent avatar owner")
    else:
        avatar_text = avatar_path.read_text(encoding="utf-8")
        for token in [
            "internal struct MobileAgentAvatar: View",
            "internal func mobileAgentAvatarKind",
            "internal func mobileResolvePersonaColor",
            "internal func mobileResolvePersonaShape",
            "AvatarImagePolicy.data(fromImageDataURL:",
            "MobileOnboardingCharacter(",
        ]:
            if token not in avatar_text:
                errors.append(f"canonical native Agent avatar contract drift: missing {token}")

    if not summary_path.is_file():
        errors.append("missing canonical native Agent summary projection")
    elif "let avatarState: MobileAgentAvatarState" not in summary_path.read_text(encoding="utf-8"):
        errors.append("native Agent summary dropped canonical avatar activity state")

    if not bot_service_path.is_file():
        errors.append("missing canonical native Bot roster adapter")
    else:
        service_text = bot_service_path.read_text(encoding="utf-8")
        for token in [
            'currentActivity: row["currentActivity"]',
            'isSharedRoom: row["isSharedRoom"] as? Bool ?? false',
            "avatarState: canonical.avatarState",
            "avatarDataURL: canonical.avatarDataURL",
        ]:
            if token not in service_text:
                errors.append(f"native Bot roster no longer preserves avatar authority: missing {token}")

    if not bot_chat_path.is_file():
        errors.append("missing native Bot chat surface")
    else:
        bot_chat_text = bot_chat_path.read_text(encoding="utf-8")
        if bot_chat_text.count("MobileAgentAvatar(bot: bot") < 4:
            errors.append(
                "native Bot chat no longer reuses canonical MobileAgentAvatar across visible Agent surfaces"
            )
        if "ClothGhostAvatar(botId: bot.id" in bot_chat_text:
            errors.append("native Bot chat bypasses canonical MobileAgentAvatar")

    if not group_members_path.is_file():
        errors.append("missing native group-member Agent surface")
    else:
        group_text = group_members_path.read_text(encoding="utf-8")
        for token in ["MobileAgentAvatar(bot: member", "MobileAgentAvatar(bot: candidate"]:
            if token not in group_text:
                errors.append(f"native group-member surface bypasses canonical avatar owner: missing {token}")
        if "ClothGhostAvatar(botId: member.id" in group_text or "ClothGhostAvatar(botId: candidate.id" in group_text:
            errors.append("native group-member surface bypasses canonical MobileAgentAvatar")

    if not home_path.is_file():
        errors.append("missing native Agent home surface")
    else:
        home_text = home_path.read_text(encoding="utf-8")
        if home_text.count("MobileAgentAvatar(bot: bot") < 2:
            errors.append("native Agent home rows no longer reuse canonical MobileAgentAvatar")
        if "ClothGhostAvatar(botId: bot.id" in home_text:
            errors.append("native Agent home rows bypass canonical MobileAgentAvatar")

    # Native owner composition and device security remain part of strict architecture.
    rust_host=ROOT / "source/packages/mahayana-rs/mahayana-host/src/lib.rs"
    if rust_host.is_file():
        source=rust_host.read_text(encoding="utf-8")
        for token in ["ModelCredentialResolver", "credential_client.session_token()", "ModelProviderMode::UserConfiguredRemote"]:
            if token not in source:
                errors.append(f"missing native credential separation contract: {token}")
        for bad in ["HostAuthExtension", "ProductionTeamRulesResolver"]:
            if bad in source:
                errors.append(f"forbidden secondary Cursor HostAuth owner in iOS: {bad}")



def tree_delta(before, after):
    """Return complete path additions, removals, and Git-identity changes."""
    before_paths, after_paths = set(before), set(after)
    added = after_paths - before_paths
    removed = before_paths - after_paths
    changed = {
        path for path in before_paths & after_paths
        if before[path] != after[path]
    }
    return added, removed, changed


def candidate_delta_predecessor(impact, locked_commit):
    """Choose a candidate delta base without mistaking the live-main lock for review history."""
    changes = impact.get("currentSourceChanges")
    if not isinstance(changes, list):
        raise ValueError("impact register currentSourceChanges must be an array")
    if changes:
        if not HEX40.fullmatch(locked_commit or ""):
            raise ValueError("candidate delta requires an exact locked authority commit")
        return locked_commit, "locked-authority"
    historical = impact.get("historicalPriorImpact")
    if not isinstance(historical, dict):
        raise ValueError("empty currentSourceChanges requires historicalPriorImpact")
    previous = historical.get("previousCommit")
    if not HEX40.fullmatch(previous or ""):
        raise ValueError("empty currentSourceChanges requires exact historicalPriorImpact.previousCommit")
    if historical.get("currentCommit") != impact.get("currentCommit"):
        raise ValueError("historicalPriorImpact is not bound to current impact commit")
    if historical.get("fullTree") != impact.get("fullTree"):
        raise ValueError("historicalPriorImpact is not bound to current impact tree")
    if impact.get("currentCommit") != locked_commit:
        raise ValueError("impact register is not bound to locked authority for historical reconstruction")
    return previous, "historical-prior-impact"


def current_main_candidate_source_changes(before, after, ledgers):
    """Treat every changed/new selected source path as open, never inheriting closure."""
    added, removed, changed = tree_delta(before, after)
    selected_after = {
        path for path in after
        if path.startswith("frontend/") or path.startswith("source/")
    }
    rows = []
    for path in sorted((added | changed) & selected_after):
        previous = before.get(path)
        mode, kind, sha, size = after[path]
        old_row = ledgers.get(path)
        rows.append({
            "path": path,
            "change": "added" if previous is None else "modified",
            "previousIdentity": ({
                "mode": previous[0], "type": previous[1],
                "sha": previous[2], "size": previous[3],
            } if previous is not None else None),
            "currentIdentity": {
                "mode": mode, "type": kind, "sha": sha, "size": size,
            },
            "previousStatus": old_row.get("implementation_status") if old_row else None,
            "candidateStatus": "unreviewed",
            "countedAsClosed": False,
        })
    removed_selected = sorted(
        path for path in removed
        if path.startswith("frontend/") or path.startswith("source/")
    )
    return rows, removed_selected, added, removed, changed


def candidate_group_for_new_path(path, manifest_index, ledger_index, errors):
    """Reuse the existing rebaseline path classifier; do not add a second group owner."""
    script_path = ROOT / ".github/scripts/generate-ios-desktop-main-rebaseline.py"
    try:
        spec = importlib.util.spec_from_file_location(
            "ios_desktop_main_rebaseline_candidate_classifier", script_path
        )
        if spec is None or spec.loader is None:
            raise ValueError("cannot load the canonical new-path classifier")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        group = module.classify_new_path_manifest_group(path)
    except SystemExit as exc:
        errors.append(str(exc))
        return None
    except Exception as exc:
        errors.append(f"{path}: cannot classify new selected source path: {exc}")
        return None
    manifest_groups = {row.get("name") for row in manifest_index.get("groups", [])}
    ledger_groups = {row.get("name") for row in ledger_index.get("chunks", [])}
    if group not in manifest_groups:
        errors.append(f"{path}: candidate classifier points to absent manifest group {group}")
    if group not in ledger_groups:
        errors.append(f"{path}: candidate classifier points to absent ledger group {group}")
    return group


def verify_current_main_candidate(lock, manifest_index, ledger_index, current_checkout,
                                  locked_checkout, expected_live_main, report_dir, errors,
                                  source_predecessor_checkout):
    """Verify live-main inventory while keeping stale ledger rows explicitly non-accepted.

    This is an architecture/source-authority candidate gate, not a rebaseline write and
    not release acceptance. It validates the recorded predecessor separately, computes
    the complete live Git tree, and records every source delta as unreviewed.
    """
    live_start = live_head()
    if not HEX40.fullmatch(expected_live_main or ""):
        errors.append("current-main candidate requires an exact 40-hex expected live SHA")
    if live_start != expected_live_main:
        errors.append(
            f"Desktop main drift before candidate audit: expected={expected_live_main} live={live_start}"
        )

    current_head = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=current_checkout
    ).decode().strip()
    current_tree_sha = subprocess.check_output(
        ["git", "rev-parse", "HEAD^{tree}"], cwd=current_checkout
    ).decode().strip()
    locked_head = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=locked_checkout
    ).decode().strip()
    locked_tree_sha = subprocess.check_output(
        ["git", "rev-parse", "HEAD^{tree}"], cwd=locked_checkout
    ).decode().strip()
    if current_head != expected_live_main:
        errors.append(f"candidate checkout commit differs from live-main lease: {current_head}")
    if locked_head != lock.get("commit") or locked_tree_sha != lock.get("rootTreeSha"):
        errors.append(
            f"recorded predecessor checkout identity drift: commit={locked_head} tree={locked_tree_sha}"
        )

    impact = document(ROOT / lock["inventory"]["impactRegister"])
    try:
        expected_delta_predecessor, source_delta_predecessor_kind = candidate_delta_predecessor(
            impact, lock.get("commit")
        )
    except ValueError as exc:
        errors.append(str(exc))
        expected_delta_predecessor = lock.get("commit")
        source_delta_predecessor_kind = "invalid"
    source_delta_head = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=source_predecessor_checkout
    ).decode().strip()
    source_delta_tree_sha = subprocess.check_output(
        ["git", "rev-parse", "HEAD^{tree}"], cwd=source_predecessor_checkout
    ).decode().strip()
    if source_delta_head != expected_delta_predecessor:
        errors.append(
            f"source-delta predecessor checkout identity drift: expected={expected_delta_predecessor} actual={source_delta_head}"
        )

    current_records = git_tree(current_checkout)
    locked_records = git_tree(locked_checkout)
    source_predecessor_records = git_tree(source_predecessor_checkout)
    current_selected = {
        path: row for path, row in current_records.items()
        if path.startswith("frontend/") or path.startswith("source/")
    }
    locked_selected = {
        path: row for path, row in locked_records.items()
        if path.startswith("frontend/") or path.startswith("source/")
    }
    full_hash = tree_digest(current_records)
    selected_hash = tree_digest(current_selected)
    gitlink_count = sum(
        1 for mode, kind, _sha, _size in current_records.values()
        if kind == "commit" or mode == "160000"
    )
    added, removed, changed = tree_delta(locked_records, current_records)

    ledger_rows = {}
    for group in ledger_index.get("chunks", []):
        path = ROOT / group["path"]
        if not path.is_file():
            continue
        block = document(path)
        for row in block.get("rows", []):
            ledger_path = row.get("desktop_path")
            if ledger_path:
                ledger_rows[ledger_path] = row

    source_changes, removed_selected, added, removed, changed = current_main_candidate_source_changes(
        source_predecessor_records, current_records, ledger_rows
    )
    if removed_selected:
        # A selected-source deletion may proceed only with an exact, independently
        # reviewed tombstone. The tombstone remains unclosed and is not a ledger row.
        reviewed = impact.get("selectedPathDeletions", [])
        if not isinstance(reviewed, list):
            errors.append("selectedPathDeletions must be an array")
        else:
            by_path = {row.get("path"): row for row in reviewed if isinstance(row, dict)}
            expected = {
                "source/packaging/dandelion-engine.json": {
                    "status": "reviewed-upstream-deletion",
                    "previousCommit": locked_head,
                    "previousTree": locked_tree_sha,
                    "previousBlobSha": locked_records.get("source/packaging/dandelion-engine.json", (None, None, None, None))[2],
                    "currentCommit": current_head,
                    "currentTree": current_tree_sha,
                },
            }
            for path in removed_selected:
                row = by_path.get(path)
                if row is None:
                    errors.append("selected source deletion lacks exact reviewed tombstone: " + path)
                    continue
                for field, value in expected.get(path, {}).items():
                    if row.get(field) != value:
                        errors.append(f"selected source deletion tombstone identity mismatch: {path} field={field}")
                if row.get("countedAsClosed") is not False:
                    errors.append("selected source deletion tombstone cannot count as closed: " + path)
                if not row.get("reason") or not row.get("reviewEvidence") or not row.get("replacementPaths"):
                    errors.append("selected source deletion tombstone lacks reason/evidence/replacement: " + path)

    for row in source_changes:
        if row["change"] == "added":
            group = candidate_group_for_new_path(
                row["path"], manifest_index, ledger_index, errors
            )
            row["manifestGroup"] = group
            row["ledgerGroup"] = group

    nonselected_changes = []
    for path in sorted(added | removed | changed):
        if path.startswith("frontend/") or path.startswith("source/"):
            continue
        previous = locked_records.get(path)
        current = current_records.get(path)
        nonselected_changes.append({
            "path": path,
            "change": "added" if previous is None else ("removed" if current is None else "modified"),
            "previousIdentity": ({
                "mode": previous[0], "type": previous[1],
                "sha": previous[2], "size": previous[3],
            } if previous is not None else None),
            "currentIdentity": ({
                "mode": current[0], "type": current[1],
                "sha": current[2], "size": current[3],
            } if current is not None else None),
            "coverageStatus": "unreviewed-candidate",
            "countedAsClosed": False,
        })

    live_end = live_head()
    if live_end != expected_live_main:
        errors.append(
            f"Desktop main drift during candidate audit: expected={expected_live_main} live={live_end}"
        )

    report_dir.mkdir(parents=True, exist_ok=True)
    def identity_jsonl(records):
        return "".join(
            json.dumps({
                "path": path, "mode": row[0], "type": row[1],
                "sha": row[2], "size": row[3],
            }, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"
            for path, row in sorted(records.items())
        )

    full_identity_text = identity_jsonl(current_records)
    selected_identity_text = identity_jsonl(current_selected)
    full_identity_path = report_dir / "full-tree-identities.jsonl"
    selected_identity_path = report_dir / "selected-tree-identities.jsonl"
    full_identity_path.write_text(full_identity_text, encoding="utf-8")
    selected_identity_path.write_text(selected_identity_text, encoding="utf-8")

    ios_head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT).decode().strip()
    report = {
        "schemaVersion": 1,
        "validationStatus": "pass" if not errors else "fail",
        "repository": "bhrumom/fabushi-desktop",
        "branch": "main",
        "desktopCommit": current_head,
        "desktopRootTree": current_tree_sha,
        "liveMainStart": live_start,
        "liveMainEnd": live_end,
        "iosRepository": "fabushi-ios/fabushi-ios",
        "iosCommit": ios_head,
        "lockedPredecessorCommit": locked_head,
        "lockedPredecessorTree": locked_tree_sha,
        "sourceDeltaPredecessorCommit": source_delta_head,
        "sourceDeltaPredecessorTree": source_delta_tree_sha,
        "sourceDeltaPredecessorKind": source_delta_predecessor_kind,
        "inventory": {
            "fullTrackedCount": len(current_records),
            "selectedFileCount": len(current_selected),
            "nonselectedCount": len(current_records) - len(current_selected),
            "gitlinkCount": gitlink_count,
            "fullTreeSha256": full_hash,
            "selectedTreeSha256": selected_hash,
        },
        "changeSummary": {
            "addedFullTreePaths": len(added),
            "removedFullTreePaths": len(removed),
            "identityChangedFullTreePaths": len(changed),
            "addedSelectedSourcePaths": sum(1 for row in source_changes if row["change"] == "added"),
            "changedSelectedSourcePaths": sum(1 for row in source_changes if row["change"] == "modified"),
            "removedSelectedSourcePaths": len(removed_selected),
            "changedNonselectedPaths": len(nonselected_changes),
        },
        "openSelectedSourceChanges": source_changes,
        "openNonselectedChanges": nonselected_changes,
        "fullTreeIdentityArtifact": {
            "path": full_identity_path.name,
            "rows": len(current_records),
            "sha256": hashlib.sha256(full_identity_text.encode("utf-8")).hexdigest(),
        },
        "selectedTreeIdentityArtifact": {
            "path": selected_identity_path.name,
            "rows": len(current_selected),
            "sha256": hashlib.sha256(selected_identity_text.encode("utf-8")).hexdigest(),
        },
        "authorityLockMutated": False,
        "manifestLedgerMutated": False,
        "responsibilityPromotion": "none",
        "finalAcceptance": False,
        "validationErrors": list(errors),
    }
    report_path = report_dir / "candidate-authority-report.json"
    report_path.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    report["reportPath"] = str(report_path)
    print(
        "CURRENT_MAIN_CANDIDATE "
        + json.dumps({
            "desktopCommit": current_head,
            "desktopRootTree": current_tree_sha,
            **report["inventory"],
            **report["changeSummary"],
            "finalAcceptance": False,
        }, sort_keys=True, separators=(",", ":"))
    )
    for row in source_changes:
        print(
            "CURRENT_MAIN_OPEN_SOURCE "
            + json.dumps({
                "path": row["path"],
                "blobSha": row["currentIdentity"]["sha"],
                "candidateStatus": "unreviewed",
                "countedAsClosed": False,
                "previousStatus": row.get("previousStatus"),
                "manifestGroup": row.get("manifestGroup"),
            }, sort_keys=True, separators=(",", ":"))
        )
    print("CURRENT_MAIN_CANDIDATE_REPORT=" + str(report_path))
    return report


def run():
    parser=argparse.ArgumentParser()
    parser.add_argument("--strict",action="store_true",help="verify shipping owner composition and fail closed on authority drift")
    parser.add_argument("--complete",action="store_true",help="require full current-main responsibility, release, and evidence closure")
    parser.add_argument("--authority-only",action="store_true",help="fast start/end live main authority check (not a complete source audit)")
    parser.add_argument("--expected-live-main",help="exact live Desktop-main SHA pinned by the current workflow run")
    parser.add_argument("--current-main-candidate",action="store_true",help="audit exact live main against the recorded predecessor without promoting stale ledger rows")
    parser.add_argument("--desktop-checkout",type=Path,help="pinned live Desktop-main checkout; required for source tree acceptance")
    parser.add_argument("--locked-desktop-checkout",type=Path,help="pinned recorded predecessor checkout for candidate authority validation")
    parser.add_argument("--candidate-report-dir",type=Path,help="directory for complete candidate inventory and audit report artifacts")
    parser.add_argument("--source-predecessor-checkout",type=Path,help="exact Git checkout used to reconstruct the source delta")
    parser.add_argument("--resolve-source-delta-predecessor",action="store_true",help="print the exact source-delta predecessor SHA and kind as GitHub Actions outputs")
    args=parser.parse_args()
    if args.resolve_source_delta_predecessor:
        try:
            lock = document(LOCK)
            impact = document(ROOT / lock["inventory"]["impactRegister"])
            sha, kind = candidate_delta_predecessor(impact, lock.get("commit"))
            print(f"sha={sha}")
            print(f"kind={kind}")
            return 0
        except (OSError, ValueError, KeyError, json.JSONDecodeError) as exc:
            print("ERROR: source-delta predecessor resolution failed closed:", exc)
            return 1
    errors=[]; warnings=[]
    try:
        for p in [LOCK,MANIFEST_INDEX,LEDGER_INDEX]:
            if not p.is_file():
                errors.append(f"missing canonical machine authority file: {p}")
        if errors:
            raise ValueError("; ".join(errors))
        lock,manifest,ledger=(document(x) for x in (LOCK,MANIFEST_INDEX,LEDGER_INDEX))
        verify_snapshot(lock,manifest,ledger,errors)
        observed=live_head()
        if args.current_main_candidate:
            if not HEX40.fullmatch(args.expected_live_main or ""):
                errors.append("--current-main-candidate requires --expected-live-main with an exact SHA")
            elif observed != args.expected_live_main:
                errors.append(f"Desktop main differs from current-run candidate lease: expected={args.expected_live_main} live={observed}")
        elif args.expected_live_main and args.authority_only:
            if observed != args.expected_live_main:
                errors.append(f"Desktop main drift from current-run authority: expected={args.expected_live_main} live={observed}")
        elif observed!=lock.get("commit"):
            errors.append(f"STALE Desktop main lock: recorded={lock.get('commit')} live={observed}")
        expected=os.environ.get("IOS_EXPECTED_HEAD")
        if expected:
            own_head=subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT).decode().strip()
            if own_head!=expected:
                errors.append(f"iOS exact-head checkout differs from event source: {own_head} != {expected}")
        statuses=dispositions=None
        candidate_report=None
        if args.current_main_candidate:
            if args.authority_only:
                errors.append("--current-main-candidate is incompatible with --authority-only")
            if args.complete:
                errors.append("--current-main-candidate is incompatible with --complete")
            if not args.strict:
                errors.append("--current-main-candidate requires --strict architecture owner validation")
            if not args.desktop_checkout or not args.locked_desktop_checkout or not args.source_predecessor_checkout or not args.candidate_report_dir:
                errors.append("--current-main-candidate requires live, locked, and source-delta predecessor checkouts plus report directory")
            if not args.expected_live_main:
                errors.append("--current-main-candidate requires --expected-live-main")
            if args.desktop_checkout and args.locked_desktop_checkout and args.source_predecessor_checkout and args.candidate_report_dir and args.expected_live_main:
                checkout=args.desktop_checkout.resolve()
                locked_checkout=args.locked_desktop_checkout.resolve()
                source_predecessor_checkout=args.source_predecessor_checkout.resolve()
                statuses,dispositions=verify_files(lock,manifest,ledger,errors,warnings,False,locked_checkout,allow_empty_current_source_changes=True)
                verify_ios_owners(errors)
                report_dir=args.candidate_report_dir
                if not report_dir.is_absolute():
                    report_dir=(ROOT / report_dir).resolve()
                candidate_report=verify_current_main_candidate(
                    lock,manifest,ledger,checkout,locked_checkout,args.expected_live_main,report_dir,errors,
                    source_predecessor_checkout
                )
        elif not args.authority_only:
            checkout=args.desktop_checkout.resolve() if args.desktop_checkout else None
            statuses,dispositions=verify_files(lock,manifest,ledger,errors,warnings,args.complete,checkout)
            if args.strict or args.complete:
                verify_ios_owners(errors)
        elif args.complete:
            errors.append("--complete is incompatible with --authority-only")
        if errors:
            print_error_summary(errors)
            for message in errors[:250]:
                print("ERROR:",message)
            if len(errors)>250:
                print(f"ERROR: {len(errors)-250} additional errors (truncated)")
            return 1
        if statuses is not None and not args.current_main_candidate:
            print(f"Desktop main -> iOS source authority {lock['commit']} tree={lock['rootTreeSha']} "
                  f"tracked={lock['inventory']['fullTrackedCount']} selected={lock['inventory']['selectedFileCount']} "
                  f"statuses={dict(statuses)} dispositions={dict(dispositions)}")
        for message in warnings[:15]:
            print("HISTORICAL:",message)
        if len(warnings)>15:
            print(f"HISTORICAL: {len(warnings)-15} additional row statuses are not current-main acceptance")
        if args.current_main_candidate:
            print("PASS: live Desktop-main candidate inventory and strict iOS architecture owner contracts; changed source remains open and final acceptance=false")
        elif args.expected_live_main and args.authority_only:
            print("PASS: live Desktop main remained at the run-pinned exact authority; source ledger/release acceptance is not implied")
        else:
            print("PASS: live Desktop main authority"+(" and source/ledger identity" if not args.authority_only else "")+
                  "; full product/release completeness is NOT implied")
        return 0
    except (OSError,ValueError,KeyError,subprocess.SubprocessError,json.JSONDecodeError) as exc:
        print("ERROR: fail-closed authority validation:",exc)
        return 1


if __name__ == "__main__":
    sys.exit(run())
