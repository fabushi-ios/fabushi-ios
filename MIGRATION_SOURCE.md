# Migration source — live Desktop main

Status: active / incomplete (policy and source snapshot are not final acceptance)
Canonical Spec: docs/specs/gpui-telegram-main-ios-parity.md
Machine authority: manifests/desktop-main-authority.json

- Desktop repository: bhrum/gpui-telegram
- Desktop live branch: main
- Desktop observed snapshot at 2026-11-17: 889246e4621e3e8e58022895a887c63ea3130bdd
- Desktop snapshot root tree: ca25541f1993ed981a409aa1fb24b5c2f56eae89
- Target repository: fabushi-ios/fabushi-ios
- Target workstream: feat/gpui-telegram-ios-parity

Never parse this Markdown for an authoritative SHA. Each migration/validation/acceptance cycle reads the JSON machine lock and resolves the live Desktop main exact ref (`bhrum/gpui-telegram` `refs/heads/main`). Any new Desktop main SHA requires full rebaseline before current parity can be accepted, including selected source manifest, full tracked tree, dependency/owner-impact review, parity ledger, and exact iOS HEAD evidence.

Selected source inventory at this snapshot is 2,948 `crates/**` blobs out of 11,227 total tracked items (`8,279` tracked items outside `crates/**`, `0` gitlinks). All 44 product features (`F01`–`F44`) are specified in `docs/specs/gpui-telegram-main-ios-parity.md`, and zero ledger rows are promoted to `verified` without exact-HEAD GitHub Actions evidence.

Prior `bhrumom/fabushi-desktop` snapshots (`0ecf132681845231b597ece11bc5f979a9a8f0cc` / `d575b4f77b6895eea4ecaced96e5c512964bf3ed`), PR #20 (`798cf51d96cb1cb98cf657af212bb47274ccb701`), and Nov 8 local commit `8128951a71503a6f109ce0cb37618d022fe8e79a` are historical provenance only. Old MIGRATION_SOURCE notes remain archived at `docs/history/ios-migration-source-pr20-2026-10-08.md`. iOS maintains its standalone `source/desktop_core` + `source/host` + native SwiftUI/UIKit runtime; no external cross-repo build dependency or second app is introduced.

Full source and product migration is incomplete until all `IOS-MAIN-AC-01..21` gates are independently evidenced on final exact HEAD. No stale success artifact or policy-only commit can replace that verification.
