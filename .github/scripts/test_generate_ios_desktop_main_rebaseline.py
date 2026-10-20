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



class CurrentMainChangeRegisterTests(unittest.TestCase):
    def test_same_main_normalization_preserves_audited_change_register(self):
        row = {"path": "source/electron-main/mcp/miniapp-production.ts", "change": "modified",
               "implementation_status": "unreviewed", "countedAsClosed": False,
               "previousBlobSha": "a" * 40, "blobSha": "b" * 40}
        result = generator.carry_forward_current_main_changes(
            "f" * 40, "f" * 40,
            {"sourceChanges": [row], "responsibilityActions": [{"path": row["path"], "countedAsClosed": False}]},
            {"currentSourceChanges": [row]},
        )
        self.assertEqual({row["path"]}, result["changedPaths"])
        self.assertEqual([row], result["sourceChanges"])
        self.assertEqual([row], result["impactSourceChanges"])
        self.assertEqual("a" * 40, result["sourceChanges"][0]["previousBlobSha"])
        self.assertFalse(result["sourceChanges"][0]["countedAsClosed"])

    def test_new_main_does_not_carry_forward_old_change_register(self):
        result = generator.carry_forward_current_main_changes(
            "a" * 40, "f" * 40, {"sourceChanges": [{"path": "old"}]},
            {"currentSourceChanges": [{"path": "old"}]},
        )
        self.assertEqual(set(), result["changedPaths"])
        self.assertIsNone(result["sourceChanges"])
        self.assertIsNone(result["impactSourceChanges"])


class RebaselineStateInheritanceTests(unittest.TestCase):
    def test_pending_path_unchanged_remains_unreviewed_and_not_closed(self):
        state = generator.inherited_ledger_state(
            {"implementation_status": "unreviewed", "countedAsClosed": False}, False
        )
        self.assertEqual(
            {"implementation_status": "unreviewed", "countedAsClosed": False, "preserved": False},
            state,
        )

    def test_mapped_path_unchanged_remains_mapped_and_is_preserved(self):
        state = generator.inherited_ledger_state({"implementation_status": "mapped"}, False)
        self.assertEqual(
            {"implementation_status": "mapped", "countedAsClosed": False, "preserved": True},
            state,
        )

    def test_mapped_path_changed_is_demoted_and_not_closed(self):
        state = generator.inherited_ledger_state({"implementation_status": "mapped"}, True)
        self.assertEqual(
            {"implementation_status": "unreviewed", "countedAsClosed": False, "preserved": False},
            state,
        )

    def test_historical_implementation_status_is_not_counted_closed(self):
        for status in ("implemented", "verified", "not-applicable"):
            with self.subTest(status=status):
                state = generator.inherited_ledger_state(
                    {"implementation_status": status, "countedAsClosed": True}, False
                )
                self.assertEqual(status, state["implementation_status"])
                self.assertFalse(state["countedAsClosed"])
                self.assertFalse(state["preserved"])

    def test_stale_closed_flags_trigger_normalization(self):
        self.assertTrue(generator.ledger_requires_status_normalization([
            {"implementation_status": "mapped", "countedAsClosed": True},
        ]))
        self.assertTrue(generator.ledger_requires_status_normalization([
            {"implementation_status": "implemented", "countedAsClosed": True},
        ]))
        self.assertFalse(generator.ledger_requires_status_normalization([
            {"implementation_status": "mapped", "countedAsClosed": False},
            {"implementation_status": "unreviewed", "countedAsClosed": False},
        ]))

    def test_preserved_mapped_summary_uses_predecessor_status_and_changed_paths(self):
        predecessor = {
            "source/host/app/src/main.rs": {"implementation_status": "mapped"},
            "source/shared/mapped.ts": {"implementation_status": "mapped"},
            "source/shared/pending.ts": {"implementation_status": "unreviewed", "countedAsClosed": False},
        }
        result = generator.calculate_preserved_mapped_state(
            predecessor, {"source/shared/mapped.ts"}, set(predecessor)
        )
        by_path = {row["path"]: row for row in result["preservedMappedPaths"]}
        self.assertEqual("mapped", result["preservedMappedOwnerStatus"])
        self.assertEqual(1, result["preservedMappedCount"])
        self.assertEqual(0, result["countedAsClosed"])
        self.assertEqual(
            {"path": "source/host/app/src/main.rs", "previousImplementationStatus": "mapped",
             "implementation_status": "mapped", "preserved": True, "countedAsClosed": False},
            by_path["source/host/app/src/main.rs"],
        )
        self.assertEqual("unreviewed", by_path["source/shared/mapped.ts"]["implementation_status"])
        self.assertFalse(by_path["source/shared/mapped.ts"]["countedAsClosed"])
        pending = generator.inherited_ledger_state(predecessor["source/shared/pending.ts"], False)
        self.assertEqual("unreviewed", pending["implementation_status"])
        self.assertFalse(pending["countedAsClosed"])


class StrictAuthorityReceiptTests(unittest.TestCase):
    PINNED = "a" * 40
    TREE = "b" * 40

    def valid_receipt(self):
        return generator.receipt(
            self.PINNED, self.TREE, self.PINNED, self.PINNED, self.TREE, self.PINNED
        )

    def test_receipt_accepts_equal_real_observations(self):
        result = self.valid_receipt()
        self.assertTrue(result["authority_current"])
        self.assertEqual(self.PINNED, result["liveMainBefore"])
        self.assertEqual(self.PINNED, result["checkoutCommit"])
        self.assertEqual(self.PINNED, result["latestDesktopMain"])

    def test_receipt_rejects_before_commit_mismatch(self):
        with self.assertRaisesRegex(SystemExit, "live_main_before"):
            generator.receipt(
                self.PINNED, self.TREE, "c" * 40, self.PINNED, self.TREE, self.PINNED
            )

    def test_receipt_rejects_checkout_commit_mismatch(self):
        with self.assertRaisesRegex(SystemExit, "checkout commit"):
            generator.receipt(
                self.PINNED, self.TREE, self.PINNED, "c" * 40, self.TREE, self.PINNED
            )

    def test_receipt_rejects_after_commit_mismatch(self):
        with self.assertRaisesRegex(SystemExit, "latest_desktop_main"):
            generator.receipt(
                self.PINNED, self.TREE, self.PINNED, self.PINNED, self.TREE, "c" * 40
            )

    def test_receipt_rejects_checkout_tree_mismatch(self):
        with self.assertRaisesRegex(SystemExit, "checkout tree"):
            generator.receipt(
                self.PINNED, self.TREE, self.PINNED, self.PINNED, "c" * 40, self.PINNED
            )


if __name__ == "__main__":
    unittest.main(verbosity=2)
