# Migration source

## Active iOS migration authority

- Source repository: bhrumom/fabushi-desktop
- Source pull request: #20
- Source branch: refactor/grok-018-architecture-rebuild
- Pinned source commit: 88dcc3b0ac83d8c3aabff8050551eabdd1726455
- Target repository: fabushi-ios/fabushi-ios
- Target pull request: #3
- Active Spec: docs/specs/fabushi-desktop-pr20-ios-architecture-parity.md

Fabushi iOS is a standalone downstream port of Desktop PR #20. Source may be reused, ported, translated, or adapted into this repository, but the iOS product must not require another Fabushi source checkout or a new shared Desktop/iOS runtime repository.

Grok Bot 0.18 is historical architecture/provenance context through Desktop PR #20. It is no longer the direct iOS migration authority.

The current Desktop `frontend/** + source/**` inventory contains 7,943 source-bearing paths at the pinned exact HEAD.

The previous `c64035cca2bdb301e948036250ed6ffcc6ea7e42` baseline and Actions runs `37212537932` / `37212537938` are historical only after the current authority advance. The `c64035cca2bdb301e948036250ed6ffcc6ea7e42 -> 88dcc3b0ac83d8c3aabff8050551eabdd1726455` delta is one commit across two `source/host/**` inference files. It adds a strict whole-step DSML compatibility boundary for first-party Fabushi Responses: only canonical DSML that names currently declared tools is normalized into function calls, while mixed prose, malformed syntax, duplicate parameters, and undeclared tools fail closed as ordinary text. iOS maps that responsibility to the Rust `mahayana-native-engine` model-to-tool boundary so provider-neutral `mahayana-model` remains inference-only; canonical iOS `send_message` DSML follows the native tool schema and the resulting function-call/output history continues through the existing authorization and Agent loop. Chunk `sourceCommit` fields preserve extraction provenance; the top-level manifest/ledger authority and every changed Desktop blob identity are the exact-current acceptance boundary.

## Historical repository extraction provenance

- Original source repository: bhrumom/fabushi
- Original source commit: 7851b689d2fe3fc3893cd9f4363899cc4a03e83b
- Original target boundary: ios
- Original source roots: mobile/ios;mobile/native/include
- Project: FAB-P0013 / PRS
