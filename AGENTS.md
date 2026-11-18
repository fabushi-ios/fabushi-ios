# Fabushi iOS — Agent Instructions

Repository: fabushi-ios/fabushi-ios (not historical bhrumom/fabushi-ios).

## Mandatory active authority

The **only live desktop product/architecture upstream** is bhrum/gpui-telegram branch main (paired with official `TelegramMessenger/Telegram-iOS` source UX geometry/interaction fidelity). The canonical iOS migration Spec is docs/specs/gpui-telegram-main-ios-parity.md and the machine lock is manifests/desktop-main-authority.json.

1. Read this AGENTS.md and the canonical iOS Spec before product-affecting work.
2. Resolve Desktop main (`bhrum/gpui-telegram@main`) and iOS branch exact HEADs; read upstream crate contracts, accepted active Specs/contracts, then current shipping owners and CI oracles.
3. Treat historical `bhrumom/fabushi-desktop`, PR #20, PR #11, Grok and unmerged PRs as historical/research only. Their old SHA, ledger count and green jobs do not prove current main parity.
4. Distinguish upstream requirement-only planned work from real shipping implementation; both remain in final scope, but neither may be fabricated as implemented/verified.
5. Rebaseline all selected source (`crates/**`) and full tracked inventory against Desktop main; propagate owner/contract impacts, don't silently keep old verified rows.
6. Existing-owner-first: unified Fabushi shell/conversation/transcript/composer/profile/settings/MiniApps/Hermes/Computers and typed capability differences. Preserve Coordinator/Host/Runner, canonical security/state and lifecycle boundaries. Do not introduce a second app, separate Telegram-only app target, Human-only workspace or shared Desktop/iOS runtime repository.
7. Port product responsibility/effect, not filenames; reuse pure-Rust crates in `source/desktop_core/` via `source/host/` C-ABI and use SwiftUI/UIKit/Apple native mechanisms where required; the source for all build/runtime inputs is iOS-owned.
8. Blocker != complete. All required GitHub Actions and native-device/signed/TestFlight/release checks need exact-head proof, non-skipped steps and artifact digests.

## Spec-first development

**Discover -> Spec -> Architecture/Plan -> Implement -> Verify -> Spec Compliance Review -> Integrate/Deliver.** Read docs/specs/spec-first-ai-development.md. Repair an absent/stale contract before coding. No deletion, warning downgrade or assertion relaxation to make CI green.

## Build/test policy

All executable builds, tests, linters, generators, validators, benchmarks and acceptance run **only in GitHub Actions or a user-authorized remote runtime**, never local Mac/Windows or assistant container. Read-only investigation, Git/API editing are permitted. Missing runner/account/device/signing = blocked, not passed.

The full completion bar is IOS-MAIN-AC-01..21 in the active Spec. Current source acceptance requires Desktop main SHA at the start **and end** of the gate, matching Git root tree/authority lock, exact iOS SHA, successful real shipping paths, and independent evidence review.
