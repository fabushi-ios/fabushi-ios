# Migration source

## Active iOS migration authority

- Source repository: bhrumom/fabushi-desktop
- Source pull request: #20
- Source branch: refactor/grok-018-architecture-rebuild
- Pinned source commit: 6e2cfc9aba647984ec4a35bb823142d7d2e9aeaf
- Target repository: fabushi-ios/fabushi-ios
- Target pull request: #3
- Active Spec: docs/specs/fabushi-desktop-pr20-ios-architecture-parity.md

Fabushi iOS is a standalone downstream port of Desktop PR #20. Source may be reused, ported, translated, or adapted into this repository, but the iOS product must not require another Fabushi source checkout or a new shared Desktop/iOS runtime repository.

Grok Bot 0.18 is historical architecture/provenance context through Desktop PR #20. It is no longer the direct iOS migration authority.

The current Desktop `frontend/** + source/**` inventory contains 7,943 source-bearing paths at the pinned exact HEAD.


The latest `ba58a474050e05943f6282d3e57aea45a75a237e -> 6e2cfc9aba647984ec4a35bb823142d7d2e9aeaf` authority move is **2 commits across exactly 2 selected Host files**: `source/host/Cargo.toml` and `source/host/src/extensions/inference/provider_session.rs`. Desktop now keeps one process-shared Tokio provider I/O runtime and one shared Responses HTTP client/connection pool instead of recreating transport state per turn; OpenRouter also borrows that process-owned runtime. This responsibility is applicable to iOS. The native port keeps the existing `ResponsesModelRuntime`/Host/NativeEngine ownership boundaries and adapts the lifetime contract by reusing one process-owned `ureq::Agent` across Responses, Chat Completions, and Anthropic requests; the workspace already enables Tokio `rt-multi-thread`, so no parallel provider runtime is introduced. The two source-host blob identities and source-host chunk provenance are rebound to the new Desktop exact HEAD. All Actions evidence from iOS `195c3dfbb1a1d08657412b5ea62e5c39230aae85` is historical only and must not be promoted to current authority.

The previous `88dcc3b0ac83d8c3aabff8050551eabdd1726455` baseline is historical after Desktop PR #20 advanced through `efd027139c8db7cc036cd3344579f047090b6c04` to `38be2a105ccf5dbd23f2c6e81886d10539ad018f`. Existing iOS HEAD `8ddac696bbdd5a0139112f90b87f0455c4323649` and Actions runs `37216783417` / `37216783543` remain regression evidence only; they cannot be promoted as current-authority acceptance.

The full `88dcc3b0ac83d8c3aabff8050551eabdd1726455 -> 38be2a105ccf5dbd23f2c6e81886d10539ad018f` delta is five commits across exactly three files. Two selected `source/host/**` blobs carry the production change introduced at `efd027139c8db7cc036cd3344579f047090b6c04`: canonical hidden reply/closing-send nudges must reproduce the same turn's still-pending user request back into inference, preserving explicit markers, constraints, and requested output details instead of sending only the synthetic nudge text. iOS maps this responsibility to the existing `MahayanaRuntime -> NativeEngine -> canonical transcript bridge` owner: `NativeEngine` keeps bounded retry, interruption/suspension and WaitingUser/Box fences, keeps synthetic reply/closing-send identity cleanup, and appends the original pending request inside each hidden nudge without creating a parallel TurnRuntime owner.

The third changed file is Desktop packaged E2E acceptance outside the selected `frontend/** + source/**` inventory. Across the current delta it recognizes SendMessage text-card assistant completion and then makes lifecycle capture read assistant identity/busy/failed from either the outer transcript article or its nested assistant surface. iOS does not copy those Electron DOM selectors. Its native equivalent is the existing Runtime/FeatureHost/UI event protocol: canonical same-operation non-user `chat.message` carries settled visible assistant content, while `chat.delta` remains streaming and `operation.completed` / `operation.interrupted` / `operation.failed` own lifecycle settlement. The focused Swift contract accepts text and attachment-only canonical assistant messages while rejecting user, streaming-delta, stale-operation, and empty shapes.

The newest `a667bdf5b3ad98ef2e69f2443a753233c0ad0667 -> 38be2a105ccf5dbd23f2c6e81886d10539ad018f` commit changes only `desktop/e2e/openbot-packaged-acceptance.spec.ts`: packaged acceptance now reads the canonical `.sand-agent-avatar[data-avatar-shape]` identity instead of the retired avatar test attributes. This file is outside the selected `frontend/** + source/**` migration inventory, so no selected Desktop blob identity changes. iOS already renders Bot identity through one native `ClothGhostAvatar` shape owner across the Bot chat surfaces; the strict architecture checker now fences that reusable production owner and stable accessibility identity rather than copying an Electron selector.

The newest `38be2a105ccf5dbd23f2c6e81886d10539ad018f -> ba58a474050e05943f6282d3e57aea45a75a237e` commit is also acceptance-only and changes only `desktop/e2e/openbot-packaged-acceptance.spec.ts`. Desktop PERF-001 now measures local submit paint from the real Send gesture through the canonical user-turn paint boundary, and raw `timings.json` evidence is persisted before thresholds are enforced so failures remain diagnosable. No selected `frontend/** + source/**` blob changed. The iOS adaptation is an acceptance responsibility, not a new shipping runtime owner: eventual signed/archive acceptance must measure from the real native Send tap through a rendered user-turn boundary and preserve raw timing evidence before asserting the applicable latency thresholds.

The authoritative selected inventory remains exactly 7,943 paths. The `38be2a105ccf5dbd23f2c6e81886d10539ad018f -> ba58a474050e05943f6282d3e57aea45a75a237e` move changes no selected blob identity, so per-path blob SHAs, chunk snapshot `sourceCommit` provenance, and reviewed production dispositions remain unchanged after revalidation. Both top-level indexes, the strict checker, and the active authority documents are rebound to `ba58a474050e05943f6282d3e57aea45a75a237e`. Older iOS runs remain regression evidence only; current responsibilities are not promoted until the resulting single iOS exact HEAD completes both required ordinary and protected GitHub Actions, while signed/archive performance acceptance remains pending until it produces the native gesture-to-paint evidence described above.

## Historical repository extraction provenance

- Original source repository: bhrumom/fabushi
- Original source commit: 7851b689d2fe3fc3893cd9f4363899cc4a03e83b
- Original target boundary: ios
- Original source roots: mobile/ios;mobile/native/include
- Project: FAB-P0013 / PRS
