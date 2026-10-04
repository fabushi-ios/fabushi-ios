# Migration source

## Active iOS migration authority

- Source repository: bhrumom/fabushi-desktop
- Source pull request: #20
- Source branch: refactor/grok-018-architecture-rebuild
- Pinned source commit: c64035cca2bdb301e948036250ed6ffcc6ea7e42
- Target repository: fabushi-ios/fabushi-ios
- Target pull request: #3
- Active Spec: docs/specs/fabushi-desktop-pr20-ios-architecture-parity.md

Fabushi iOS is a standalone downstream port of Desktop PR #20. Source may be reused, ported, translated, or adapted into this repository, but the iOS product must not require another Fabushi source checkout or a new shared Desktop/iOS runtime repository.

Grok Bot 0.18 is historical architecture/provenance context through Desktop PR #20. It is no longer the direct iOS migration authority.

The current Desktop `frontend/** + source/**` inventory contains 7,943 source-bearing paths at the pinned exact HEAD.

The previous `ca2e4caba082cb858fa2d253da1749de5530b164` baseline and Actions runs `37209380010` / `37209379990` are historical only after the current authority advance. The `ca2e4caba082cb858fa2d253da1749de5530b164 -> c64035cca2bdb301e948036250ed6ffcc6ea7e42` delta is two commits across four `source/product/fabushi/**` files. It moves the shipping first-party Responses default from the obsolete `/v1/ai/responses` path to the deployed `/codex-deepseek/v1/responses` adapter and re-exports that policy from the account service. The iOS Rust AppHost owns the corresponding native production endpoint and is updated to the same deployed path. Chunk `sourceCommit` fields preserve extraction provenance; the top-level manifest/ledger authority and every changed Desktop blob identity are the exact-current acceptance boundary.

## Historical repository extraction provenance

- Original source repository: bhrumom/fabushi
- Original source commit: 7851b689d2fe3fc3893cd9f4363899cc4a03e83b
- Original target boundary: ios
- Original source roots: mobile/ios;mobile/native/include
- Project: FAB-P0013 / PRS
