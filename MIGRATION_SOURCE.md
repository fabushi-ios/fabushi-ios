# Migration source

## Active iOS migration authority

- Source repository: bhrumom/fabushi-desktop
- Source pull request: #20
- Source branch: refactor/grok-018-architecture-rebuild
- Pinned source commit: a667bdf5b3ad98ef2e69f2443a753233c0ad0667
- Target repository: fabushi-ios/fabushi-ios
- Target pull request: #3
- Active Spec: docs/specs/fabushi-desktop-pr20-ios-architecture-parity.md

Fabushi iOS is a standalone downstream port of Desktop PR #20. Source may be reused, ported, translated, or adapted into this repository, but the iOS product must not require another Fabushi source checkout or a new shared Desktop/iOS runtime repository.

Grok Bot 0.18 is historical architecture/provenance context through Desktop PR #20. It is no longer the direct iOS migration authority.

The current Desktop `frontend/** + source/**` inventory contains 7,943 source-bearing paths at the pinned exact HEAD.

The previous `88dcc3b0ac83d8c3aabff8050551eabdd1726455` baseline is historical after Desktop PR #20 advanced through `efd027139c8db7cc036cd3344579f047090b6c04` to `a667bdf5b3ad98ef2e69f2443a753233c0ad0667`. Existing iOS HEAD `8ddac696bbdd5a0139112f90b87f0455c4323649` and Actions runs `37216783417` / `37216783543` remain regression evidence only; they cannot be promoted as current-authority acceptance.

The full `88dcc3b0ac83d8c3aabff8050551eabdd1726455 -> a667bdf5b3ad98ef2e69f2443a753233c0ad0667` delta is four commits across exactly three files. Two selected `source/host/**` blobs carry the production change introduced at `efd027139c8db7cc036cd3344579f047090b6c04`: canonical hidden reply/closing-send nudges must reproduce the same turn's still-pending user request back into inference, preserving explicit markers, constraints, and requested output details instead of sending only the synthetic nudge text. iOS maps this responsibility to the existing `MahayanaRuntime -> NativeEngine -> canonical transcript bridge` owner: `NativeEngine` keeps bounded retry, interruption/suspension and WaitingUser/Box fences, keeps synthetic reply/closing-send identity cleanup, and appends the original pending request inside each hidden nudge without creating a parallel TurnRuntime owner.

The third changed file is Desktop packaged E2E acceptance outside the selected `frontend/** + source/**` inventory. Across the current delta it recognizes SendMessage text-card assistant completion and then makes lifecycle capture read assistant identity/busy/failed from either the outer transcript article or its nested assistant surface. iOS does not copy those Electron DOM selectors. Its native equivalent is the existing Runtime/FeatureHost/UI event protocol: canonical same-operation non-user `chat.message` carries settled visible assistant content, while `chat.delta` remains streaming and `operation.completed` / `operation.interrupted` / `operation.failed` own lifecycle settlement. The focused Swift contract accepts text and attachment-only canonical assistant messages while rejecting user, streaming-delta, stale-operation, and empty shapes.

The authoritative selected inventory remains exactly 7,943 paths. The affected `source-host` manifest/ledger provenance, both top-level indexes, the strict checker, and the two changed selected blob identities are rebound to `a667bdf5b3ad98ef2e69f2443a753233c0ad0667`. Changed responsibilities remain `implemented`, not `verified`, until the resulting single iOS exact HEAD completes both required ordinary and protected GitHub Actions.

## Historical repository extraction provenance

- Original source repository: bhrumom/fabushi
- Original source commit: 7851b689d2fe3fc3893cd9f4363899cc4a03e83b
- Original target boundary: ios
- Original source roots: mobile/ios;mobile/native/include
- Project: FAB-P0013 / PRS
