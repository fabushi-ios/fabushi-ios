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

    def test_unreviewed_impact_never_verified(self):
        errors=[];warnings=[]
        row={"implementation_status":"verified","ios_disposition":"ios-adapted",
             "desktop_responsibility":"real","desktop_visible_effect":"real",
             "ios_target_path":"README.md","production_evidence":"a","test_evidence":"b",
             "authority_review_state":"requires-main-responsibility-and-owner-review"}
        validator.check_status(row,"source/a.rs",False,errors,warnings,"a"*40)
        self.assertTrue(any("impacted" in e for e in errors))


if __name__=="__main__":
    unittest.main()
