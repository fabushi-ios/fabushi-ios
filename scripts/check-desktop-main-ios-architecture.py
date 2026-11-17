#!/usr/bin/env python3
"""Fail-closed live Desktop-main source/ownership authority validation for Fabushi iOS.

This validates the recorded snapshot against the actual pinned Git tree and the
live Desktop main ref in GitHub Actions. No PR20 SHA or file count is hardcoded.
--complete is intentionally stronger than ordinary inventory validation.
"""
import argparse
import hashlib
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
ACTIVE_SPEC = ROOT / "docs/specs/gpui-telegram-main-ios-parity.md"
HISTORICAL_SPECS = [
    ROOT / "docs/specs/fabushi-desktop-main-ios-parity.md",
    ROOT / "docs/specs/fabushi-desktop-pr20-ios-architecture-parity.md",
]
HISTORICAL_SPEC = HISTORICAL_SPECS[0]
UPSTREAM_REPO = "bhrum/gpui-telegram"
UPSTREAM_URL = "https://github.com/bhrum/gpui-telegram.git"
SELECTED_ROOTS = ["crates/**"]
HEX40 = re.compile(r"^[0-9a-f]{40}$")
DISPOSITIONS = {"unreviewed", "direct-port", "ios-adapted", "not-applicable-with-replacement"}
STATUSES = {"unreviewed", "understood", "mapped", "implemented", "verified", "not-applicable"}


def is_selected_path(path):
    return path.startswith("crates/")


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
    if lock.get("repository") != UPSTREAM_REPO or lock.get("branch") != "main":
        errors.append(f"authority lock is not {UPSTREAM_REPO}@main")
    inventory = lock.get("inventory", {})
    if inventory.get("selectedRoots") != SELECTED_ROOTS:
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
    for file in [ACTIVE_SPEC, *HISTORICAL_SPECS, ROOT / "MIGRATION_SOURCE.md", ROOT / "AGENTS.md"]:
        if not file.is_file():
            errors.append(f"missing canonical authority document: {file}")
    if ACTIVE_SPEC.is_file():
        active = ACTIVE_SPEC.read_text(encoding="utf-8")
        for token in [
            "bhrum/gpui-telegram",
            "c40494379cd03f71a2c1f57bae5f85d677cb0e3e",
            "Desktop main",
            "IOS-MAIN-AUTH-01",
            "IOS-MAIN-AC-21",
            "F01",
            "F44",
        ]:
            if token not in active:
                errors.append(f"canonical iOS Spec missing normative token {token}")


def verify_files(lock, manifest_index, ledger_index, errors, warnings, complete, checkout):
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
        if not is_selected_path(path):
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
        if p in recorded or is_selected_path(p):
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
        selected = {p: x for p, x in real.items() if is_selected_path(p)}
        if len(selected) != lock["inventory"]["selectedFileCount"]:
            errors.append(f"selected Desktop tree count drift: {len(selected)}")
        if tree_digest(real) != lock["inventory"]["fullTreeSha256"]:
            errors.append("full tracked Desktop root tree inventory SHA256 drift")
        if tree_digest(selected) != lock["inventory"]["selectedTreeSha256"]:
            errors.append("selected Desktop source inventory SHA256 drift")
        if set(real) != set(recorded) | set(outside):
            errors.append("full Desktop tree has missing/unexpected tracked paths")
        for path, row in recorded.items():
            got = real.get(path)
            if not got or got[1] != "blob" or got[2] != row["blobSha"] or got[3] != row.get("size"):
                errors.append(f"{path}: blob/size does not match live main exact Git tree")
        for path, row in outside.items():
            got = real.get(path)
            if not got or got != (row.get("mode"), row.get("type"), row.get("sha"), row.get("size")):
                errors.append(f"{path}: nonselected Git identity differs from live main")
    release_path = ROOT / "docs/parity/desktop-main-release-evidence.json"
    if not release_path.is_file():
        errors.append("missing durable current-main release acceptance/evidence matrix")
        release = {}
    else:
        release = document(release_path)
        if release.get("desktopSourceCommit") != source_sha or release.get("desktopRootTree") != lock.get("rootTreeSha"):
            errors.append("release acceptance matrix is not bound to current Desktop main")
        expected_ac = {f"IOS-MAIN-AC-{i:02d}" for i in range(1, 22)}
        entries = release.get("acceptanceCriteria", [])
        actual_ac = [r.get("id") for r in entries]
        if len(actual_ac) != 21 or set(actual_ac) != expected_ac:
            errors.append("release matrix must contain exactly 21 stable acceptance criterion IDs")
        for r in entries:
            if r.get("status") == "passed":
                if not r.get("evidence") or not r.get("independentReviewer"):
                    errors.append(f"{r.get('id')}: passed claim lacks evidence or independent reviewer")
                if r.get("iosCommit") != os.environ.get("IOS_EXPECTED_HEAD"):
                    errors.append(f"{r.get('id')}: passed claim not tied to current iOS exact head")
        for key, gate in release.get("releaseGates", {}).items():
            if gate.get("status") == "passed":
                required = ["iosCommit", "desktopCommit", "workflowRun", "runAttempt", "job", "requiredSteps", "artifactId", "artifactDigest", "independentReviewer"]
                for field in required:
                    if not gate.get(field):
                        errors.append(f"{key}: passed release gate lacks {field}")
                if gate.get("desktopCommit") != source_sha or gate.get("iosCommit") != os.environ.get("IOS_EXPECTED_HEAD"):
                    errors.append(f"{key}: passed release gate source identity mismatch")
                if not re.fullmatch(r"sha256:[0-9a-f]{64}", gate.get("artifactDigest") or ""):
                    errors.append(f"{key}: passed release artifact digest not sha256")
                if gate.get("requiredSteps") and any(step.get("conclusion") != "success" or step.get("status") != "completed" for step in gate["requiredSteps"]):
                    errors.append(f"{key}: passed release gate has missing, skipped or failed required step")
        if complete:
            if release.get("finalAcceptance") is not True:
                errors.append("final independent iOS release acceptance not recorded")
            for r in entries:
                if r.get("status") != "passed":
                    errors.append(f"{r.get('id')}: mandatory release criterion not accepted")
            for name, gate in release.get("releaseGates", {}).items():
                if gate.get("status") != "passed":
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
    """Enforce gpui-telegram native iOS owner boundaries and fail closed on stubs/facades."""
    required_dirs = [
        "source/desktop_core",
        "source/host",
        "mobile/ios/Fabushi/Bridge",
        "mobile/ios/Fabushi/Presentation",
    ]
    for relative in required_dirs:
        if not (ROOT / relative).is_dir():
            errors.append(f"missing iOS architecture root: {relative}")

    required_crates = [
        "fabushi_miniapp",
        "fabushi_miniapp_wasm",
        "gpui_telegram_agent",
        "gpui_telegram_client",
        "gpui_telegram_common",
        "gpui_telegram_core",
        "gpui_telegram_media",
        "gpui_telegram_proto",
        "gpui_telegram_server",
        "gpui_telegram_test_harness",
        "gpui_telegram_webrtc",
        "magpie_gateway",
    ]
    for crate in required_crates:
        if crate != "gpui_telegram_common" and not (ROOT / "source/desktop_core/crates" / crate / "Cargo.toml").is_file():
            errors.append(f"missing vendored upstream v1.2.0 crate: source/desktop_core/crates/{crate}")
    if not (ROOT / "source/desktop_core/crates/gpui_telegram_common").is_dir():
        errors.append("missing vendored upstream directory: source/desktop_core/crates/gpui_telegram_common")

    required_files = [
        "docs/specs/gpui-telegram-main-ios-parity.md",
        "source/desktop_core/Cargo.toml",
        "source/host/Cargo.toml",
        "source/host/include/mahayana_app_host.h",
        "source/host/src/lib.rs",
        "mobile/ios/project.yml",
        "mobile/ios/Fabushi/FabushiApp.swift",
        "mobile/ios/Fabushi/Bridge/TelegramBridgeRuntime.swift",
        "mobile/ios/Fabushi/Bridge/TelegramEventStream.swift",
        "mobile/ios/Fabushi/Bridge/AccountScopeManager.swift",
        "mobile/ios/Fabushi/Bridge/MediaTransferManager.swift",
        "mobile/ios/Fabushi/Presentation/Shell/TelegramRootShellView.swift",
        "mobile/ios/Fabushi/Presentation/Chats/ChatListView.swift",
    ]
    for relative in required_files:
        if not (ROOT / relative).is_file():
            errors.append(f"missing canonical gpui-telegram iOS owner file: {relative}")

    # Standalone iOS build inputs: forbid external cross-repo path/git dependencies.
    dependency_files = [
        ROOT / "mobile/ios/project.yml",
        ROOT / "source/desktop_core/Cargo.toml",
        ROOT / "source/host/Cargo.toml",
    ]
    for dependency_file in dependency_files:
        if not dependency_file.is_file():
            continue
        text = dependency_file.read_text(encoding="utf-8")
        for forbidden in [
            "bhrumom/fabushi-platform-core",
            "../fabushi-platform-core",
            "../fabushi-desktop",
            "../gpui-telegram",
            "github.com/bhrumom/fabushi-desktop",
            "github.com/bhrum/gpui-telegram",
        ]:
            if forbidden in text:
                errors.append(
                    f"{dependency_file.relative_to(ROOT)} introduces forbidden cross-repo runtime dependency: {forbidden}"
                )

    # Anti-facade 1: SearchIndexingBridge.swift dictionary mock MUST NOT exist.
    facade_bridge = ROOT / "mobile/ios/Fabushi/Bridge/SearchIndexingBridge.swift"
    if facade_bridge.exists():
        errors.append("forbidden in-memory Swift search facade present: mobile/ios/Fabushi/Bridge/SearchIndexingBridge.swift")

    # Anti-facade 2: stale Nov 8 Mac-local TEST_READY.md MUST NOT exist.
    test_ready = ROOT / "TEST_READY.md"
    if test_ready.is_file() and "/Users/bhrum/Documents/ChatGPT" in test_ready.read_text(encoding="utf-8"):
        errors.append("stale Nov 8 local-Mac TEST_READY.md report still present at repository root")

    # Anti-facade 3: Rust C-ABI dispatch (src/dispatch.rs or src/dispatch/**) must fail closed
    # with unknown_command and contain zero catch-all 'acknowledged' or hardcoded sample_chats stubs.
    has_dispatch_owner = (
        (ROOT / "source/host/src/dispatch.rs").is_file()
        or (ROOT / "source/host/src/dispatch/mod.rs").is_file()
    )
    if not has_dispatch_owner:
        errors.append("missing Rust C-ABI dispatch owner (source/host/src/dispatch.rs or src/dispatch/mod.rs)")
    host_src = ROOT / "source/host/src"
    if host_src.is_dir():
        rs_files = sorted(host_src.rglob("*.rs"))
        combined_rs = "\n".join(p.read_text(encoding="utf-8") for p in rs_files)
        if "unknown_command" not in combined_rs:
            errors.append("source/host/src missing fail-closed unknown_command error code")
        for stub_token in [
            '"status": "acknowledged"',
            '"status":"acknowledged"',
            '"receivedParams"',
            "Welcome to Fabushi iOS Telegram Core",
            "Native iOS 1:1 Telegram UI is live!",
        ]:
            if stub_token in combined_rs:
                errors.append(f"source/host/src contains forbidden stub/mock token: {stub_token}")

    # Canonical C-ABI header & Swift bridge ownership checks.
    c_header = ROOT / "source/host/include/mahayana_app_host.h"
    if c_header.is_file():
        header_text = c_header.read_text(encoding="utf-8")
        for sym in [
            "telegram_app_host_create",
            "telegram_app_host_dispatch",
            "telegram_app_host_free_string",
            "telegram_app_host_destroy",
            "telegram_bridge_register_event_callback",
        ]:
            if sym not in header_text:
                errors.append(f"mahayana_app_host.h missing C-ABI symbol: {sym}")

    bridge_swift = ROOT / "mobile/ios/Fabushi/Bridge/TelegramBridgeRuntime.swift"
    if bridge_swift.is_file():
        bridge_text = bridge_swift.read_text(encoding="utf-8")
        for token in ["class TelegramBridgeRuntime", "static let shared"]:
            if token not in bridge_text:
                errors.append(f"TelegramBridgeRuntime.swift missing canonical token: {token}")

    app = ROOT / "mobile/ios/Fabushi/FabushiApp.swift"
    if app.is_file():
        app_text = app.read_text(encoding="utf-8")
        for forbidden in ["MahayanaHost(", "MahayanaCoordinator("]:
            if forbidden in app_text:
                errors.append(f"FabushiApp.swift directly constructs runtime owner: {forbidden}")

    # Compile-blocker & fake-benchmark regression fences.
    fabushi_swift_dir = ROOT / "mobile/ios/Fabushi"
    if fabushi_swift_dir.is_dir():
        metrics_defs = 0
        for swift_file in fabushi_swift_dir.rglob("*.swift"):
            stext = swift_file.read_text(encoding="utf-8")
            if "enum ChatListRowMetrics" in stext:
                metrics_defs += 1
            if "SearchIndexingBridge" in stext:
                errors.append(f"{swift_file.relative_to(ROOT)} still references deleted SearchIndexingBridge facade")
        if metrics_defs > 1:
            errors.append(f"duplicate ChatListRowMetrics declarations across Fabushi target ({metrics_defs} > 1)")

    shell_swift = ROOT / "mobile/ios/Fabushi/Presentation/Shell/TelegramRootShellView.swift"
    if shell_swift.is_file() and "Spacer(height:" in shell_swift.read_text(encoding="utf-8"):
        errors.append("TelegramRootShellView.swift contains invalid SwiftUI Spacer(height:) initializer")

    e2e_suite = ROOT / "tests/e2e/master_e2e_suite.swift"
    if e2e_suite.is_file():
        e2e_text = e2e_suite.read_text(encoding="utf-8")
        for bad_e2e in ["@main", "measuredLatencyTalkBargeInMs: 60.0", "measuredLatencyTantivyCommitMs: 1.25"]:
            if bad_e2e in e2e_text:
                errors.append(f"tests/e2e/master_e2e_suite.swift contains forbidden construct: {bad_e2e}")


def run():
    parser = argparse.ArgumentParser()
    parser.add_argument("--strict", action="store_true", help="verify shipping owner composition and fail closed on authority drift")
    parser.add_argument("--complete", action="store_true", help="require full current-main responsibility, release, and evidence closure")
    parser.add_argument("--authority-only", action="store_true", help="fast start/end live main authority check (not a complete source audit)")
    parser.add_argument("--desktop-checkout", type=Path, help="pinned Desktop-main checkout; required for source tree acceptance")
    args = parser.parse_args()
    errors = []
    warnings = []
    try:
        for p in [LOCK, MANIFEST_INDEX, LEDGER_INDEX]:
            if not p.is_file():
                errors.append(f"missing canonical machine authority file: {p}")
        if errors:
            raise ValueError("; ".join(errors))
        lock, manifest, ledger = (document(x) for x in (LOCK, MANIFEST_INDEX, LEDGER_INDEX))
        verify_snapshot(lock, manifest, ledger, errors)
        observed = live_head()
        if observed != lock.get("commit"):
            errors.append(f"STALE Desktop main lock: recorded={lock.get('commit')} live={observed}")
        expected = os.environ.get("IOS_EXPECTED_HEAD")
        if expected:
            own_head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT).decode().strip()
            if own_head != expected:
                errors.append(f"iOS exact-head checkout differs from event source: {own_head} != {expected}")
        statuses = dispositions = None
        if not args.authority_only:
            checkout = args.desktop_checkout.resolve() if args.desktop_checkout else None
            statuses, dispositions = verify_files(lock, manifest, ledger, errors, warnings, args.complete, checkout)
            if args.strict or args.complete:
                verify_ios_owners(errors)
        elif args.complete:
            errors.append("--complete is incompatible with --authority-only")
        if errors:
            for message in errors[:250]:
                print("ERROR:", message)
            if len(errors) > 250:
                print(f"ERROR: {len(errors)-250} additional errors (truncated)")
            return 1
        if statuses is not None:
            print(
                f"Desktop main -> iOS source authority {lock['commit']} tree={lock['rootTreeSha']} "
                f"tracked={lock['inventory']['fullTrackedCount']} selected={lock['inventory']['selectedFileCount']} "
                f"statuses={dict(statuses)} dispositions={dict(dispositions)}"
            )
        for message in warnings[:15]:
            print("HISTORICAL:", message)
        if len(warnings) > 15:
            print(f"HISTORICAL: {len(warnings)-15} additional row statuses are not current-main acceptance")
        print(
            "PASS: live Desktop main authority"
            + (" and source/ledger identity" if not args.authority_only else "")
            + "; full product/release completeness is NOT implied"
        )
        return 0
    except (OSError, ValueError, KeyError, subprocess.SubprocessError, json.JSONDecodeError) as exc:
        print("ERROR: fail-closed authority validation:", exc)
        return 1


if __name__ == "__main__":
    sys.exit(run())
