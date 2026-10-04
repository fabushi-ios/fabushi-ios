#!/usr/bin/env python3
import argparse
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXPECTED_DESKTOP_REPOSITORY = "bhrumom/fabushi-desktop"
EXPECTED_DESKTOP_PR = 20
EXPECTED_DESKTOP_BRANCH = "refactor/grok-018-architecture-rebuild"
EXPECTED_DESKTOP_COMMIT = "a3d9a509144f0997df10cdc85a50cc27507f5aa1"
EXPECTED_FILES = 7943

VALID_DISPOSITIONS = {
    "unreviewed",
    "direct-port",
    "ios-adapted",
    "not-applicable-with-replacement",
}
VALID_STATUSES = {
    "unreviewed",
    "mapped",
    "implemented",
    "verified",
    "not-applicable",
}

parser = argparse.ArgumentParser()
parser.add_argument("--strict", action="store_true")
parser.add_argument(
    "--complete",
    action="store_true",
    help="require every Desktop PR #20 row to be verified or reviewed not-applicable",
)
args = parser.parse_args()

errors = []
warnings = []

manifest_index_path = ROOT / "manifests/desktop-pr20-reference-index.json"
ledger_index_path = ROOT / "docs/parity/desktop-pr20-index.json"
active_spec_path = ROOT / "docs/specs/fabushi-desktop-pr20-ios-architecture-parity.md"
old_spec_path = ROOT / "docs/specs/grok-bot-0.18-ios-architecture-parity.md"

for path in [manifest_index_path, ledger_index_path, active_spec_path, old_spec_path]:
    if not path.is_file():
        errors.append(f"missing authority file: {path.relative_to(ROOT)}")

if errors:
    for error in errors:
        print(f"ERROR: {error}")
    raise SystemExit(1)

manifest_index = json.loads(manifest_index_path.read_text(encoding="utf-8"))
ledger_index = json.loads(ledger_index_path.read_text(encoding="utf-8"))

authority = manifest_index.get("authority", {})
for key, expected in {
    "repository": EXPECTED_DESKTOP_REPOSITORY,
    "pullRequest": EXPECTED_DESKTOP_PR,
    "branch": EXPECTED_DESKTOP_BRANCH,
    "commit": EXPECTED_DESKTOP_COMMIT,
}.items():
    if authority.get(key) != expected:
        errors.append(f"desktop manifest authority drift: {key}={authority.get(key)!r}, expected {expected!r}")

source = ledger_index.get("source", {})
for key, expected in {
    "repository": EXPECTED_DESKTOP_REPOSITORY,
    "pullRequest": EXPECTED_DESKTOP_PR,
    "branch": EXPECTED_DESKTOP_BRANCH,
    "commit": EXPECTED_DESKTOP_COMMIT,
}.items():
    if source.get(key) != expected:
        errors.append(f"desktop ledger authority drift: {key}={source.get(key)!r}, expected {expected!r}")

if manifest_index.get("fileCount") != EXPECTED_FILES:
    errors.append(
        f"desktop manifest fileCount={manifest_index.get('fileCount')}, expected {EXPECTED_FILES}"
    )
if ledger_index.get("rowCount") != EXPECTED_FILES:
    errors.append(
        f"desktop ledger rowCount={ledger_index.get('rowCount')}, expected {EXPECTED_FILES}"
    )

manifest_rows = []
for chunk in manifest_index.get("groups", []):
    path = ROOT / chunk["path"]
    if not path.is_file():
        errors.append(f"missing desktop manifest chunk: {chunk['path']}")
        continue
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not (payload.get("sourceCommit") or "").strip():
        errors.append(f"{chunk['path']}: missing sourceCommit provenance")
    rows = payload.get("files", [])
    if payload.get("fileCount") != len(rows) or chunk.get("fileCount") != len(rows):
        errors.append(f"{chunk['path']}: fileCount mismatch")
    manifest_rows.extend(rows)

ledger_rows = []
for chunk in ledger_index.get("chunks", []):
    path = ROOT / chunk["path"]
    if not path.is_file():
        errors.append(f"missing desktop parity chunk: {chunk['path']}")
        continue
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not (payload.get("sourceCommit") or "").strip():
        errors.append(f"{chunk['path']}: missing sourceCommit provenance")
    rows = payload.get("rows", [])
    if payload.get("rowCount") != len(rows) or chunk.get("rowCount") != len(rows):
        errors.append(f"{chunk['path']}: rowCount mismatch")
    ledger_rows.extend(rows)

if len(manifest_rows) != EXPECTED_FILES:
    errors.append(f"manifest materialized {len(manifest_rows)} rows, expected {EXPECTED_FILES}")
if len(ledger_rows) != EXPECTED_FILES:
    errors.append(f"ledger materialized {len(ledger_rows)} rows, expected {EXPECTED_FILES}")

manifest_by_path = {row.get("path"): row for row in manifest_rows}
ledger_by_path = {row.get("desktop_path"): row for row in ledger_rows}

if len(manifest_by_path) != len(manifest_rows):
    errors.append("desktop manifest contains duplicate paths")
if len(ledger_by_path) != len(ledger_rows):
    errors.append("desktop parity ledger contains duplicate desktop_path values")
if set(manifest_by_path) != set(ledger_by_path):
    missing = sorted(set(manifest_by_path) - set(ledger_by_path))
    extra = sorted(set(ledger_by_path) - set(manifest_by_path))
    errors.append(
        f"manifest/ledger path mismatch: missing={missing[:5]} extra={extra[:5]}"
    )

status_counts = Counter()
disposition_counts = Counter()
for path, row in ledger_by_path.items():
    manifest_row = manifest_by_path.get(path)
    if manifest_row and row.get("desktop_blob_sha") != manifest_row.get("blobSha"):
        errors.append(f"{path}: desktop blob SHA differs from pinned manifest")

    status = (row.get("implementation_status") or "").strip()
    disposition = (row.get("ios_disposition") or "").strip()
    target = (row.get("ios_target_path") or "").strip()
    status_counts[status] += 1
    disposition_counts[disposition] += 1

    if status not in VALID_STATUSES:
        errors.append(f"{path}: invalid implementation_status={status!r}")
    if disposition not in VALID_DISPOSITIONS:
        errors.append(f"{path}: invalid ios_disposition={disposition!r}")

    if status in {"mapped", "implemented", "verified", "not-applicable"}:
        if (row.get("desktop_responsibility") or "").strip() in {"", "pending-review"}:
            errors.append(f"{path}: reviewed row has no desktop responsibility")
        if (row.get("desktop_visible_effect") or "").strip() in {"", "pending-review"}:
            errors.append(f"{path}: reviewed row has no desktop visible effect")

    if status in {"implemented", "verified"}:
        if not target:
            errors.append(f"{path}: {status} row has no ios_target_path")
        elif not (ROOT / target).is_file():
            errors.append(f"{path}: {status} target is missing: {target}")
        if not (row.get("production_evidence") or "").strip():
            errors.append(f"{path}: {status} row has no production_evidence")

    if status == "verified" and not (row.get("test_evidence") or "").strip():
        errors.append(f"{path}: verified row has no test_evidence")

    if status == "not-applicable":
        if disposition != "not-applicable-with-replacement":
            errors.append(f"{path}: not-applicable status requires not-applicable-with-replacement disposition")
        if not (row.get("ios_platform_delta") or "").strip():
            errors.append(f"{path}: not-applicable row has no iOS platform delta/replacement")

    if args.complete and status not in {"verified", "not-applicable"}:
        errors.append(f"{path}: completion gate still has status={status}")

active_spec = active_spec_path.read_text(encoding="utf-8")
for token in [
    EXPECTED_DESKTOP_REPOSITORY,
    "pull request: `#20`",
    EXPECTED_DESKTOP_COMMIT,
    "no-shared-runtime",
]:
    if token not in active_spec:
        errors.append(f"active Desktop->iOS Spec is missing authority token: {token}")

old_spec = old_spec_path.read_text(encoding="utf-8")
if "Status: superseded" not in old_spec:
    errors.append("old Grok-direct Spec is not marked superseded")
if "Desktop PR #20 is now the direct product/architecture migration source" not in old_spec:
    errors.append("old Grok-direct Spec does not redirect authority to Desktop PR #20")

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

print(
    "Desktop PR #20 -> iOS authority: "
    f"{EXPECTED_DESKTOP_COMMIT}; rows={len(ledger_rows)}; "
    f"statuses={dict(status_counts)}; dispositions={dict(disposition_counts)}"
)

if warnings:
    for warning in warnings:
        print(f"WARNING: {warning}")

if errors:
    for error in errors:
        print(f"ERROR: {error}")
    raise SystemExit(1)

if args.strict and status_counts.get("unreviewed", 0):
    print(
        f"STRICT INFO: {status_counts['unreviewed']} Desktop rows remain intentionally unreviewed; "
        "--complete is the final closure gate."
    )

print("Desktop PR #20 -> iOS architecture baseline check passed.")
