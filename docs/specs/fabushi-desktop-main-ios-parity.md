# Fabushi Desktop main -> Fabushi iOS native product parity

Status: active
Spec ID: IOS-DESKTOP-MAIN-001
Revision: 1
Updated: 2026-10-08
Target: fabushi-ios/fabushi-ios PR #3 and successors
Canonical policy: this file, AGENTS.md, manifests/desktop-main-authority.json
Historical baseline: docs/history/ios-desktop-pr20-spec-2026-10-08.md

## 1. Goal and live authority

Build and release a standalone, native iOS edition of the **complete applicable product currently defined by bhrumom/fabushi-desktop@main**, while preserving the Fabushi Bot product, existing iOS capabilities, security, and owner boundaries. No capability survey, README, ledger, scaffolding, limited vertical slice, UI demo, or green partial CI can satisfy this goal.

- **Live source**: bhrumom/fabushi-desktop branch main; never Desktop PR #20, a release tag, Grok Bot, a draft PR, or Telegram Desktop directly.
- **Observed snapshot at this revision**: Desktop commit 3bc92400826cc4ca7ac665b467708e22261edc61, tree 3d2a0ad250ca82d0cf3b7bd917d8eca7400e5c31. This is historical provenance once main advances.
- **Machine authority**: manifests/desktop-main-authority.json; read it and query the remote main ref, not a Markdown SHA or PR description.
- **Target**: fabushi-ios/fabushi-ios, current target branch exact HEAD at validation.
- **Never use a floating checkout as acceptance evidence.** Fetch current main, record its exact commit/tree, pin the source for inventory and oracle collection, and recheck main at the end of the acceptance cycle. A HEAD change invalidates final parity until the delta is revalidated.
- **Source priority**: current Desktop main active product Specs define the target requirements, current main shipping source and composition define currently implemented owners, and current main tests/CI define behavioral oracles. When a main Spec describes an unimplemented capability (such as FBCP/TDRP requirements), keep it in scope as requirement but mark source/product completion as blocked or planned, not shipping or verified. All unmet future requirements remain in the release closure backlog.
- **Unmerged PRs**: observation/research only; they can never replace main as current authority.

## 2. Non-negotiable architectural constraints

1. Desktop main responsibility/owner/protocol/state machine/product effect are normative; filenames and OS-process boundaries are not. Existing owner first, with an ADR only for an irreducible new owner.
2. iOS owns every Swift/SwiftUI/UIKit/Apple framework adapter, all Rust, Coordinator/Host/Runner/runtime/contracts/packages, provisioning, native app target and CI. No additional Fabushi source checkout or shared Desktop/iOS runtime repository may be needed for iOS build, install or run.
3. Preserve Coordinator, Host and Runner boundaries; a single iOS process may use typed actors/modules but may not collapse ownership, cancellation, persistence, replay, tool approval or security controls.
4. Preserve **one canonical product shell** and one owner each for ConversationWorkspace, transcript, composer/draft, participant/profile, message storage, permissions, attachments/resources, navigation, search, settings, notification, marketplace and automation. Human/Agent/Group/Channel differ through typed capabilities/sections, never parallel complete workspaces.
5. A desktop-specific mechanism may change to SwiftUI Scene, ASWebAuthenticationSession, Keychain, URLSession/background transfers, BGTaskScheduler, AVFoundation, Push/CallKit or suitable remote execution. Do not lose its user-visible or security behavior.
6. Desktop provider/Runner/Computer/Telegram-network mechanics cannot be silently substituted with unrelated user-visible behavior; recorded platform deltas require an observable equivalence oracle.
7. Explicitly prohibit test-only production bypasses, synthetic acceptance outputs, duplicate canonical roots, silent fallback to online voice recognition, relaxed approvals, cross-account state, token-domain collapse, and fake success on missing capabilities.
8. Reused source requires recorded licensing/provenance/rights; do not suppress legally necessary notices. History remains history and does not become an extra runtime source of truth.

## 3. Authority protocol (IOS-MAIN-AUTH-01..08)

At the beginning of each migration slice or independent acceptance:
1. Resolve Desktop main **exact** commit and root tree through authenticated/recorded GitHub ref resolution. Record the iOS exact HEAD, source commits, toolchain and timestamp.
2. Read root AGENTS.md, applicable Desktop main SOURCE_OF_TRUTH, active normalized Specs, product acceptance/status, shipping composition, contracts and tests. Do not mistake requirements for implemented functions.
3. Exhaustively enumerate the Desktop main Git tree (all tracked blobs and gitlinks, not just selected source roots); fail if truncated, inaccessible or if recursion/dependencies are not accounted.
4. Diff the last authority snapshot: added, removed, renamed, blob-changed, metadata-only corrected, contract/owner-changed, dependency and composition-impacting files. Read affected source and its siblings before deciding status.
5. Rebuild the machine authority lock, full tracked inventory and selected source manifest/ledger from that exact Desktop tree. No hardcoded old file count; count is calculated, not an acceptance target.
6. Record a stable impacted-responsibility set including adjacent owner, inbound/outbound protocols and consumers, not only changed blobs. Old statuses are historical, and new/affected rows remain unreviewed until independently re-mapped.
7. Only carry forward unchanged and explicitly revalidated rows. A known unchanged blob without owner/contract review does not prove semantic equivalence.
8. At the **end** of each required CI/acceptance job and again before final acceptance, resolve live main. If live main != locked commit, fail closed for current-authority acceptance and rebaseline in a new iOS exact HEAD.

Transient ref fetch/API access failure is BLOCKED, not latest/current/success. A PR merge never implicitly upgrades iOS ledger status. A new main HEAD does not halt unrelated implementation; it only blocks stale acceptance promotion.

## 4. Inventory and coverage (IOS-MAIN-INV-01..07)

- Capture **all tracked Git blobs/gitlinks** from root tree, their repository/commit/path/mode/sha/size and class; inspect submodules, Git LFS, generated sources, tooling, dependency resolution, resources, docs, licensing, workflows and external downloads where applicable. Unknown/truncated paths, unresolved dependency identities or licensing gaps remain blockers.
- Maintain a selected source manifest and parity ledger for every blob under frontend/** and source/**; include business and lifecycle behavior embedded in presentation files. Pure visual implementations may be replaced by Apple-native UI, but their commands/validation/permissions/side effects must be mapped.
- Nonselected tracked entries require explicit coverage classification/replacement or non-runtime disposition in the full-tree register; nothing disappears merely because it is outside source/**.
- Maintain module dossier and **bidirectional** file -> module -> responsibility -> capability -> iOS owner/path/symbol -> shipping entrypoint -> test/evidence link. Permit many-to-one or one-to-many implementation only with explicit split/aggregation semantics.
- No directory-level "covered" marker can close unknown individual files. New files are unreviewed. Removed source remains documented for migration/removal disposition.
- Distinguish current Desktop shipping behavior, Desktop main requirement-only future contracts and iOS shipping implementation. Requirement-only upstream work stays blocked until appropriate implementation evidence; do not convert to N/A.
- Never assume historical 7,943, 7,950 or 8,172 is a permanent count. Detect and record all changes.

## 5. Ledger and status policy (IOS-MAIN-LEDGER-01..08)

Each source responsibility must record Desktop repository/branch/commit/tree/path/blob/symbol/module/responsibility ID; upstream requirement/oracle IDs; behavior/input/output/state machine/failure modes/side effects; selected existing owner and rejected alternatives; iOS target paths **and symbols**; protocol/state/persistence/security/lifecycle owners; Apple platform delta; shipping composition entrypoint; test layers; production evidence; exact iOS commit and Actions run/attempt/job/step; artifact ID/digest and independent acceptance reviewer.

Status progression: unreviewed -> understood (optional) -> mapped -> implemented -> verified. Blockers are orthogonal; blocked is never complete. A changed or impacted row reverts to the last independently supported status, never inherits previous verified, and its prior status is preserved only as historical metadata. An unchanged source blob may be carried forward only with explicit upstream owner/contract review, not silent bulk stamping.

Allowed iOS disposition classes: direct-port, ios-adapted, not-applicable-with-replacement (or unreviewed while pending). N/A is restricted to a truly inapplicable **mechanism** plus evidenced equivalent user-visible product effect; platform difficulty, missing code/server, App Store constraint or "not needed" cannot erase a required product capability. Unknown capability needs existing-owner search, rejected-owner evidence and minimal domain ADR, not new duplicate app/root.

**Implemented** requires real shipping target path/symbol and composition plus functional work. **Verified** additionally requires current Desktop main oracle, current iOS HEAD exercised through the shipping path, executable tests for happy/error/cancel/restart/permission/ordering, linked successful run/job/step and trustworthy artifact provenance. Stubs, strings in documents, test fixture success and PR body claims are insufficient.

Impact propagation: a changed file forces direct-row review; a changed owner, shared contract, dependency, command/event, state machine or call path also forces review of affected siblings/consumers. Store the impact set and its unresolved review blockers in an inspectable impact register; complete-mode fails if any remains.

## 6. Production behavior and Apple substitutions (IOS-MAIN-PRODUCT-01..10)

The full applicable Desktop main product, not a selected feature shortlist, is the scope. At minimum audit and close:
1. Account/auth/session/multi-account/credentials/passkeys/OAuth/privacy.
2. Canonical Human/Agent/Group/Channel/Topic messaging; identity/members/roles; reactions/replies/forwarding/editing/delete; drafts/search/read states.
3. Media/files/camera/audio/offline ASR/video/calls/permissions/transfers/cache/export.
4. Bot/Agent chat, model routing, Host/Runner tools, MCP/Plugins/Mini Apps/WebMCP, approval and credential isolation.
5. Marketplace, billing, purchases, entitlement, restore, StoreKit/Apple-native commerce requirements.
6. Automations, workflows, background tasks, push/notifications and suspend/restart recovery.
7. Remote Computer/Box/local safe capability adapters and mobile-specific security.
8. Durable transcript/checkpoints/replay/ordering/idempotency, cancellation and failure normalization.
9. iPhone/iPad unified shell, searchable collections, Settings/Profile/Resources, accessibility, localization and motion reductions.
10. Distribution, migration of installed data, upgrade/downgrade/rollback handling, legal notices, observability, performance and privacy.

If Desktop main has an active full-completeness upstream mandate (e.g. current FBCP/TDRP Revision), map its requirements and blocked upstream implementation dependencies into this product inventory without pretending all Telegram-derived behavior already ships on Desktop. As main shipping changes, re-evaluate the equivalent iOS capability and owner on that new HEAD.

## 7. Evidence matrix and fail-closed gates (IOS-MAIN-GATE-01..10)

Only GitHub Actions (or user-authorized remote runtime) may execute builds/tests. All required jobs/steps must actually execute **and succeed**; skipped, neutral, cancelled, absent, warning-only, or environment-blocked is not success. Do not downgrade validators, delete assertions, turn tests into acceptance of wrong behavior, rerun unchanged deterministic failures to hide them, or borrow a green artifact from an older SHA.

Machine gates:
- authority-lock schema, Desktop ref (start and finish), root tree SHA and full selected/nonselected Git tree identities;
- manifest and ledger exact path set, duplicate detection, row counts, blob hashes, impact set review and tracked-file closure;
- module/responsibility/owner/target-symbol production-composition mapping; no duplicate root/credential/transcript store;
- status and source evidence provenance; accept only current Desktop target baseline and exact iOS head;
- Rust Host/Runner/Coordinator, Swift unit, native UI, lifecycle/fault/recovery, security, accessibility, protected real-account and install/upgrade oracles;
- signed archive/export/physical-device/TestFlight/App Store checks as separately required final gates.

Failure classification: production defect, contract/data defect, test oracle defect, environment/not-configured. Fix the **real** cause and produce a new exact HEAD; do not weaken required gates. Collect complete run ID, attempt, jobs and step conclusions, commands, test counts, logs, xctest/xcresult, artifact ID, digest and source provenance, including negative evidence when blocked.

## 8. Release Definition of Done (IOS-MAIN-AC-01..21)

IOS-MAIN-AC-01 current live Desktop main exact commit/tree confirmed before and after acceptance; lock consistent.
IOS-MAIN-AC-02 all tracked files and submodule/dependency boundaries accounted; no partial/truncated inventory.
IOS-MAIN-AC-03 selected frontend/**+source/** 100% file and responsibility ledger with current source blob identity.
IOS-MAIN-AC-04 requirements-only upstream work distinguished from implemented shipping behavior and tracked to closure.
IOS-MAIN-AC-05 entire applicable Desktop product migrated; all mandatory responsibilities verified or legitimate N/A-with-replacement.
IOS-MAIN-AC-06 canonical existing-owner-first composition; single shell/root per product concept; no duplicate Human/Agent stores.
IOS-MAIN-AC-07 native Coordinator/Host/Runner typed boundaries; real routing/cancel/checkpoint/recovery.
IOS-MAIN-AC-08 complete iOS-owned source/build inputs, no shared Fabushi runtime repository.
IOS-MAIN-AC-09 login/session/credential/messaging/communication/identity/security continuity tested.
IOS-MAIN-AC-10 media/offline ASR/call/background/platform capability parity tested.
IOS-MAIN-AC-11 MCP/Marketplace/WebMCP/billing/entitlement/restore protected real-account journey passed.
IOS-MAIN-AC-12 real API/service-side dependencies and provisioning completed; blocked dependencies not counted as verified.
IOS-MAIN-AC-13 real UI functional/visual/accessibility/localization and iPhone/iPad flows covered.
IOS-MAIN-AC-14 Rust unit/integration/contracts, Swift unit/UI, fault/temporal/regression and required performance/soak verified.
IOS-MAIN-AC-15 app lifecycle cold/foreground/background/termination/relaunch/upgrade data recovery proven.
IOS-MAIN-AC-16 signed device archive/export and clean physical-device installation/launch passed.
IOS-MAIN-AC-17 update from previous accepted install and data migration rollback strategy/evidence passed.
IOS-MAIN-AC-18 TestFlight distribution/install and App Store identity/signing/entitlement/privacy readiness passed.
IOS-MAIN-AC-19 license/provenance/notices/rights inventory resolved for release.
IOS-MAIN-AC-20 independent reviewer checked exact source SHA, real CI job/step logs, artifacts and compliance ledger.
IOS-MAIN-AC-21 final PR merge into canonical iOS main, final main exact HEAD and necessary main/release CI revalidated.

There is no completion while a mandatory criterion or an upstream source-completeness dependency is unverified, blocked, skipped, or not-configured.

## 9. Current recorded state (not completion)

At initial live-main cutover Desktop selected source is **7,950 files** and the whole tracked tree is **8,172 blobs**, with zero gitlinks, at the observed Desktop tree above. These numbers are checkpoint facts, not fixed future expectations. The former PR #20 ledger had 7,943 rows and is historical seed only.

The iOS port is **in progress / NOT ACCEPTED**. Real rebaseline requires machine data and open changed-source reviews, then current-head ordinary + protected CI; signed device/TestFlight/App Store and full responsibility closure remain separate work. Prior green PR #20 artifacts are historical regression evidence only. A policy/lock/manifest commit by itself is not proof of shipped product parity.

## 10. Change and continuation protocol

Work continues until every applicable accepted criterion is evidenced, not until a convenient slice is green. Before each tranche: refetch Desktop main and iOS HEAD; rebaseline and close one real source responsibility in iOS-owned shipping code; run exact-head GitHub Actions; update ledger/impact/RTM/dossiers; independently audit. If an authority or test fails, leave it failing and fix the cause. Continue unaffected work without claiming global success.

Historical mechanics, detailed previous source-to-iOS mappings, and PR #20 per-commit notes live at docs/history/ios-desktop-pr20-spec-2026-10-08.md; they remain useful diagnostics only and cannot supersede this live-main Spec.
