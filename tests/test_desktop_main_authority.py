"""Run only in GitHub Actions: fail-closed authority checker unit contracts."""
import importlib.util
import pathlib
import unittest

ROOT=pathlib.Path(__file__).resolve().parents[1]
p=ROOT/"scripts/check-desktop-main-ios-architecture.py"
spec=importlib.util.spec_from_file_location("ios_desktop_main_authority",p)
validator=importlib.util.module_from_spec(spec)
spec.loader.exec_module(validator)


class DesktopMainAuthorityTests(unittest.TestCase):
    def test_tree_digest_stable_and_identity_sensitive(self):
        a={"frontend/a.ts":("100644","blob","a"*40,12),
           "source/b.rs":("100644","blob","b"*40,3)}
        b=dict(reversed(list(a.items())))
        self.assertEqual(validator.tree_digest(a),validator.tree_digest(b))
        c=dict(a)
        c["source/b.rs"]=("100644","blob","c"*40,3)
        self.assertNotEqual(validator.tree_digest(a),validator.tree_digest(c))

    def test_stale_main_lock_rejected(self):
        errors=[]
        data={"schemaVersion":2,"authorityMode":"live-main","repository":"bhrumom/fabushi-desktop",
              "branch":"main","commit":"a"*40,"rootTreeSha":"b"*40,
              "inventory":{"selectedRoots":["frontend/**","source/**"],"selectedFileCount":1,
              "manifestIndex":"manifests/desktop-main-reference-index.json",
              "ledgerIndex":"docs/parity/desktop-main-index.json",
              "nonselectedRegister":"manifests/desktop-main/tracked-outside-selected.json",
              "impactRegister":"docs/parity/desktop-main-impact.json","selectedTreeSha256":"c"*64}}
        mf={"authority":{"repository":data["repository"],"branch":"main","commit":"d"*40,
                          "tree":data["rootTreeSha"],"mode":"live-main"},
            "fileCount":1,"selectedTreeSha256":"c"*64}
        ld={"source":mf["authority"],"rowCount":1}
        validator.verify_snapshot(data,mf,ld,errors)
        self.assertTrue(any("manifest authority drift" in e for e in errors))

    def test_error_diagnostics_group_source_and_reason(self):
        summary = validator.summarize_errors([
            "source/mahayana/mahayana-rs/mahayana-host/src/lib.rs: incomplete mandatory responsibility status=unreviewed",
            "source/packages/agent-kv/agent-store.ts: final source owner/contract revalidation not documented",
            "frontend/packages/mcp-app-sdk/src/index.ts: verified missing test evidence",
        ])
        self.assertEqual(summary["total"], 3)
        self.assertEqual(summary["groups"]["source-mahayana"], 1)
        self.assertEqual(summary["groups"]["source-packages"], 1)
        self.assertEqual(summary["groups"]["frontend"], 1)
        self.assertEqual(summary["details"][("source-mahayana", "incomplete-mandatory-status")], 1)
        self.assertEqual(summary["details"][("source-packages", "missing-owner-contract-review")], 1)
        self.assertEqual(summary["details"][("frontend", "missing-focused-test-evidence")], 1)


    def test_current_source_changes_reject_wrong_git_identity(self):
        path = "source/mahayana/mahayana-rs/mahayana-product/src/lib.rs"
        row = {
            "desktop_blob_sha": "a" * 40,
            "responsibility_id": "RESP-1",
            "implementation_status": "unreviewed",
            "ios_disposition": "unreviewed",
            "authority_review_state": "requires-current-main-review",
            "owner_review_state": "requires-current-main-review",
            "countedAsClosed": False,
            "desktop_owner": "owner",
            "desktop_responsibility": "responsibility",
            "desktop_visible_effect": "effect",
            "ios_owner": "iOS owner",
            "ios_target_path": "source/packages/mahayana-rs/mahayana-product/src/lib.rs",
            "ios_language": "Rust",
            "ios_runtime_entrypoint": "MahayanaHost::create",
            "ios_platform_delta": "pending",
            "production_entrypoint": "MahayanaHost::create",
            "ios_native_replacement": "validated ProductClient",
            "focused_tests": ["focused test"],
            "open_items": ["exact-head evidence pending"],
            "is_production_feature": True,
        }
        change = {
            "path": path, "blobSha": "a" * 40, "size": 123, "mode": "100755", "type": "blob",
            "responsibility_id": "RESP-1", "implementation_status": "implemented",
            "candidateStatus": "implemented", "ios_disposition": "unreviewed",
            "owner_review_state": "requires-current-main-review", "countedAsClosed": False,
            **{key: row[key] for key in ("desktop_owner","desktop_responsibility","desktop_visible_effect","ios_owner","ios_target_path","ios_language","ios_runtime_entrypoint","ios_platform_delta","production_entrypoint","ios_native_replacement","focused_tests","open_items","is_production_feature")},
        }
        errors = []
        validator.verify_current_source_changes(
            {"groupSummary":{"source/mahayana":{"changedOrAdded":1}}, "currentSourceChanges":[change]},
            {path:{"blobSha":"a"*40,"size":123}}, {path:row}, errors,
            {path:("100644","blob","a"*40,123)},
        )
        self.assertTrue(any("mode/type/blob SHA/size" in e for e in errors))
        self.assertTrue(any("status differs" in e for e in errors))


    def test_candidate_tree_delta_accounts_for_added_removed_and_identity_changes(self):
        before = {
            "source/a.rs": ("100644", "blob", "a" * 40, 12),
            "docs/old.md": ("100644", "blob", "b" * 40, 4),
        }
        after = {
            "source/a.rs": ("100755", "blob", "a" * 40, 12),
            "source/new.rs": ("100644", "blob", "c" * 40, 7),
        }
        added, removed, changed = validator.tree_delta(before, after)
        self.assertEqual({"source/new.rs"}, added)
        self.assertEqual({"docs/old.md"}, removed)
        self.assertEqual({"source/a.rs"}, changed)

    def test_current_main_candidate_never_promotes_historical_verified_row(self):
        path = "source/electron-preload/preload-miniapp-tools.ts"
        before = {path: ("100644", "blob", "a" * 40, 759)}
        after = {path: ("100644", "blob", "b" * 40, 759)}
        ledger = {path: {"implementation_status": "verified", "countedAsClosed": True}}
        changes, removed_selected, _added, _removed, _changed = (
            validator.current_main_candidate_source_changes(before, after, ledger)
        )
        self.assertEqual([], removed_selected)
        self.assertEqual(1, len(changes))
        self.assertEqual("verified", changes[0]["previousStatus"])
        self.assertEqual("unreviewed", changes[0]["candidateStatus"])
        self.assertIs(changes[0]["countedAsClosed"], False)
        self.assertEqual("b" * 40, changes[0]["currentIdentity"]["sha"])

    def test_unreviewed_impact_never_verified(self):
        errors=[];warnings=[]
        row={"implementation_status":"verified","ios_disposition":"ios-adapted",
             "desktop_responsibility":"real","desktop_visible_effect":"real",
             "ios_target_path":"README.md","production_evidence":"a","test_evidence":"b",
             "authority_review_state":"requires-main-responsibility-and-owner-review"}
        validator.check_status(row,"source/a.rs",False,errors,warnings,"a"*40)
        self.assertTrue(any("impacted" in e for e in errors))


    def test_empty_current_source_register_is_skipped_only_when_candidate_delta_is_recomputed(self):
        impact = {
            "currentSourceChanges": [],
            "currentCommit": "b" * 40,
            "fullTree": "e" * 40,
            "historicalPriorImpact": {
                "previousCommit": "a" * 40,
                "currentCommit": "b" * 40,
                "fullTree": "e" * 40,
            },
        }
        errors = []
        validator.verify_current_source_changes(impact, {}, {}, errors, None)
        self.assertIn("impact register has no currentSourceChanges rows", errors)

        errors = []
        validator.verify_current_source_changes(
            impact, {}, {}, errors, None, allow_empty_candidate_register=True
        )
        self.assertEqual([], errors)
        self.assertEqual(
            ("a" * 40, "historical-prior-impact"),
            validator.candidate_delta_predecessor(impact, "b" * 40),
        )

        path = "source/electron-preload/preload-miniapp-tools.ts"
        before = {path: ("100644", "blob", "a" * 40, 759)}
        after = {path: ("100644", "blob", "b" * 40, 759)}
        rows, removed_selected, *_ = validator.current_main_candidate_source_changes(
            before, after, {path: {"implementation_status": "verified", "countedAsClosed": True}}
        )
        self.assertEqual([], removed_selected)
        self.assertEqual(1, len(rows))
        self.assertEqual("unreviewed", rows[0]["candidateStatus"])
        self.assertIs(rows[0]["countedAsClosed"], False)
        self.assertEqual("b" * 40, rows[0]["currentIdentity"]["sha"])

    def test_candidate_delta_predecessor_uses_locked_authority_when_register_has_rows(self):
        impact = {"currentSourceChanges": [{"path": "source/a.rs"}]}
        self.assertEqual(
            ("b" * 40, "locked-authority"),
            validator.candidate_delta_predecessor(impact, "b" * 40),
        )

    def test_empty_current_source_register_without_historical_commit_fails_closed(self):
        with self.assertRaisesRegex(ValueError, "historicalPriorImpact"):
            validator.candidate_delta_predecessor({"currentSourceChanges": []}, "b" * 40)

if __name__=="__main__":
    unittest.main()
