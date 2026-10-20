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
            "source/electron-main/example.ts": "source-electron-main",
            "source/electron-preload/example.ts": "source-electron-preload",
            "source/shared/example.ts": "source-shared",
            "source/product/example.ts": "source-product",
            "source/mahayana/example.rs": "source-mahayana",
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                self.assertEqual(expected, generator.classify_new_path_manifest_group(path))

    def test_unknown_new_selected_path_fails_closed(self):
        path = "assets/unclassified/example.ts"
        with self.assertRaisesRegex(
            SystemExit, "new selected path has no manifest group: " + path
        ):
            generator.classify_new_path_manifest_group(path)


if __name__ == "__main__":
    unittest.main(verbosity=2)
