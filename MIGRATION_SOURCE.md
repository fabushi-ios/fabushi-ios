# Migration source — live Desktop main

Status: active / incomplete (policy and source snapshot are not final acceptance)
Canonical Spec: docs/specs/fabushi-desktop-main-ios-parity.md
Machine authority: manifests/desktop-main-authority.json

- Desktop repository: bhrumom/fabushi-desktop
- Desktop live branch: main
- Desktop observed snapshot at 2026-10-11: d575b4f77b6895eea4ecaced96e5c512964bf3ed
- Desktop snapshot root tree: d4c8fe5c91635064f11673ebeefc94c1f83b5ab8
- Target repository: fabushi-ios/fabushi-ios
- Target workstream: PR #3 and successors

Never parse this Markdown for an authoritative SHA. Each migration/validation/acceptance cycle reads the JSON machine lock and resolves the live Desktop main exact ref. A new main SHA requires rebaseline before current parity can be accepted, including selected source manifest, full tracked tree, dependency/owner-impact review, ledger and exact iOS evidence.

Selected source inventory at this snapshot is 7,951 frontend/** + source/** blobs; the 92fcc2b0→d575b4f7 delta is specification-only outside selected roots. FPM-001 and OMA-001 are active requirements but remain pending production implementation and acceptance on iOS.

Previous PR #20 SHA 798cf51d96cb1cb98cf657af212bb47274ccb701 and its 7,943-row inventory are historical only. Old MIGRATION_SOURCE notes are archived at docs/history/ios-migration-source-pr20-2026-10-08.md. Grok and unmerged Desktop PRs are not alternative authorities. iOS maintains its standalone Coordinator/Host/Runner and native platform replacement; no shared runtime repository is introduced.

Full source and product migration is incomplete until all IOS-MAIN-AC-01..21 gates are independently evidenced on final exact HEAD. No stale success artifact or policy-only commit can replace that verification.
