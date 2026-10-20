#!/usr/bin/env python3
"""Regression tests for new Desktop source-path manifest-group classification."""
from __future__ import annotations

import importlib.util
import json
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / ".github/scripts/generate-ios-desktop-main-rebaseline.py"
SPEC = importlib.util.spec_from_file_location("desktop_main_rebaseline", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load Desktop main rebaseline generator")
generator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(generator)


class CurrentSnapshotOwnerStatusTests(unittest.TestCase):
    def test_changed_host_path_must_remain_unreviewed_and_non_closed(self):
        errors = []
        row = {
            "implementation_status": "unreviewed",
            "countedAsClosed": False,
            "authority_review_state": "requires-current-main-review",
            "production_evidence": "",
            "test_evidence": "",
        }
        generator.validate_host_mapped_owner_status(
            {generator.HOST_MAPPED_PATH: row}, {generator.HOST_MAPPED_PATH}, errors
        )
        self.assertEqual([], errors)

    def test_changed_host_path_cannot_be_promoted_by_rebaseline(self):
        errors = []
        row = {"implementation_status": "mapped", "countedAsClosed": True}
        generator.validate_host_mapped_owner_status(
            {generator.HOST_MAPPED_PATH: row}, {generator.HOST_MAPPED_PATH}, errors
        )
        self.assertEqual(
            ["changed source/host/app/src/main.rs must remain unreviewed/non-closed"],
            errors,
        )

    def test_unchanged_host_path_must_stay_mapped(self):
        errors = []
        row = {"implementation_status": "mapped"}
        generator.validate_host_mapped_owner_status(
            {generator.HOST_MAPPED_PATH: row}, set(), errors
        )
        self.assertEqual([], errors)

    def test_pending_host_path_survives_unrelated_desktop_main_advance(self):
        errors = []
        row = {
            "implementation_status": "unreviewed",
            "countedAsClosed": False,
            "authority_review_state": "requires-current-main-review",
            "production_evidence": "",
            "test_evidence": "",
        }
        generator.validate_host_mapped_owner_status(
            {generator.HOST_MAPPED_PATH: row}, set(), errors
        )
        self.assertEqual([], errors)

    def test_pending_host_path_requires_explicit_review_fence(self):
        errors = []
        row = {
            "implementation_status": "unreviewed",
            "countedAsClosed": False,
            "production_evidence": "",
            "test_evidence": "",
        }
        generator.validate_host_mapped_owner_status(
            {generator.HOST_MAPPED_PATH: row}, set(), errors
        )
        self.assertEqual(
            ["source/host/app/src/main.rs pending status lacks an explicit owner-review fence"],
            errors,
        )

    def test_pending_host_path_cannot_carry_promotable_evidence(self):
        errors = []
        row = {
            "implementation_status": "unreviewed",
            "countedAsClosed": False,
            "authority_review_state": "requires-current-main-review",
            "production_evidence": "unverified evidence",
            "test_evidence": "",
        }
        generator.validate_host_mapped_owner_status(
            {generator.HOST_MAPPED_PATH: row}, set(), errors
        )
        self.assertEqual(
            ["source/host/app/src/main.rs pending status carries promotable evidence field production_evidence"],
            errors,
        )


class NewPathManifestGroupTests(unittest.TestCase):
    def test_electron_preload_path_maps_to_existing_paired_group(self):
        path = "source/electron-preload/preload-miniapp-tools.ts"
        group = generator.classify_new_path_manifest_group(path)
        manifest_index = json.loads(
            (ROOT / "manifests/desktop-main-reference-index.json").read_text(encoding="utf-8")
        )
        ledger_index = json.loads(
            (ROOT / "docs/parity/desktop-main-index.json").read_text(encoding="utf-8")
        )
        self.assertEqual("source-electron-preload", group)
        self.assertIn(group, {row["name"] for row in manifest_index["groups"]})
        self.assertIn(group, {row["name"] for row in ledger_index["chunks"]})

    def test_existing_new_path_fallbacks_keep_their_groups(self):
        cases = {
            "frontend/example.ts": "frontend",
            "source/box-exec-daemon/example.ts": "source-box-exec-daemon",
            "source/electron-dev-controls/example.ts": "source-electron-dev-controls",
            "source/electron-main/example.ts": "source-electron-main",
            "source/electron-preload/example.ts": "source-electron-preload",
            "source/fabushi-network-worker/example.ts": "source-fabushi-network-worker",
            "source/host/Cargo.lock": "source-host",
            "source/internal/example.ts": "source-internal",
            "source/local-exec-daemon/example.ts": "source-local-exec-daemon",
            "source/mahayana/example.rs": "source-mahayana",
            "source/node-agent-coordinator/example.ts": "source-node-agent-coordinator",
            "source/packages/example.ts": "source-packages",
            "source/packaging/example.ts": "source-packaging",
            "source/product/example.ts": "source-product",
            "source/shared/example.ts": "source-shared",
            "source/mime-types.d.ts": "source-mime-types.d.ts",
            "source/tsconfig.json": "source-tsconfig.json",
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                self.assertEqual(expected, generator.classify_new_path_manifest_group(path))

    def test_every_classifier_target_exists_in_both_indices(self):
        paths = [
            "source/box-exec-daemon/new.ts",
            "source/electron-dev-controls/new.ts",
            "source/electron-main/new.ts",
            "source/electron-preload/preload-miniapp-tools.ts",
            "source/fabushi-network-worker/new.ts",
            "source/host/Cargo.lock",
            "source/internal/new.ts",
            "source/local-exec-daemon/new.ts",
            "source/mahayana/new.rs",
            "source/node-agent-coordinator/new.ts",
            "source/packages/new.ts",
            "source/packaging/new.ts",
            "source/product/new.ts",
            "source/shared/new.ts",
            "source/mime-types.d.ts",
            "source/tsconfig.json",
        ]
        manifest_index = json.loads(
            (ROOT / "manifests/desktop-main-reference-index.json").read_text(encoding="utf-8")
        )
        ledger_index = json.loads(
            (ROOT / "docs/parity/desktop-main-index.json").read_text(encoding="utf-8")
        )
        manifest_groups = {row["name"] for row in manifest_index["groups"]}
        ledger_groups = {row["name"] for row in ledger_index["chunks"]}
        for path in paths:
            with self.subTest(path=path):
                group = generator.classify_new_path_manifest_group(path)
                self.assertIn(group, manifest_groups)
                self.assertIn(group, ledger_groups)

    def test_unknown_new_selected_path_fails_closed(self):
        path = "assets/unclassified/example.ts"
        with self.assertRaisesRegex(
            SystemExit, "new selected path has no manifest group: " + path
        ):
            generator.classify_new_path_manifest_group(path)


if __name__ == "__main__":
    unittest.main(verbosity=2)
