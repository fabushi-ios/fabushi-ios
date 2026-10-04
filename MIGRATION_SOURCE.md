# Migration source

## Active iOS migration authority

- Source repository: bhrumom/fabushi-desktop
- Source pull request: #20
- Source branch: refactor/grok-018-architecture-rebuild
- Pinned source commit: a3d9a509144f0997df10cdc85a50cc27507f5aa1
- Target repository: fabushi-ios/fabushi-ios
- Target pull request: #3
- Active Spec: docs/specs/fabushi-desktop-pr20-ios-architecture-parity.md

Fabushi iOS is a standalone downstream port of Desktop PR #20. Source may be reused, ported, translated, or adapted into this repository, but the iOS product must not require another Fabushi source checkout or a new shared Desktop/iOS runtime repository.

Grok Bot 0.18 is historical architecture/provenance context through Desktop PR #20. It is no longer the direct iOS migration authority.

The current Desktop `frontend/** + source/**` inventory contains 7,943 source-bearing paths at the pinned exact HEAD.

The `266ca48a...` baseline and Actions runs `37206582726` / `37206582744` are historical only after the current authority advance. The `266ca48a... -> a3d9a509144f0997df10cdc85a50cc27507f5aa1` delta is five commits across nine files: first-party Fabushi provider/routing/settings/experiments/checkpoint responsibility changes plus a Linux-only pressure-profiler readiness fix. Applicable inference responsibilities are implemented natively in iOS; the Linux `perf` readiness change is reviewed not-applicable. Chunk `sourceCommit` fields record the provenance SHA of each materialized snapshot; the top-level manifest/ledger authority and each changed blob identity are the exact-current acceptance boundary.

## Historical repository extraction provenance

- Original source repository: bhrumom/fabushi
- Original source commit: 7851b689d2fe3fc3893cd9f4363899cc4a03e83b
- Original target boundary: ios
- Original source roots: mobile/ios;mobile/native/include
- Project: FAB-P0013 / PRS
