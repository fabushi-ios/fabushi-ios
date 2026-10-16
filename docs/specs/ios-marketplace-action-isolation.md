# Native Marketplace action isolation

Status: active; implementation and current-HEAD acceptance required.
Owner: existing SwiftUI ContentView / MarketplaceModel.
Direct authority: bhrumom/fabushi-desktop PR #20 at 798cf51d96cb1cb98cf657af212bb47274ccb701.
Parent: docs/specs/fabushi-desktop-pr20-ios-architecture-parity.md.

## 1. Context / problem

Protected run 37560457816 attempt 1, iOS source 2f2815c51559c294dda2403ac918f9354a1868fa, failed at GlobalDharmaJourneyUITests.swift:262 when returning from Marketplace. XCTest reported an inaccessible marketplace-back control. The verified diagnostic archive has SHA-256 25c0afb902fe168f80e70fb051b2fad7a4ce0f4b422b8deb4072c5d81997b40b; its source.env matches that source/run/attempt. The test log taps install-global-dharma but never open-global-dharma before failure; the video shows a MiniApp full-screen cover. The shipping plugin row contains separate install and open Buttons inside the same SwiftUI List row with inherited automatic button style.

## 2. Goal

A real tap invokes only its chosen Marketplace action. Installing must not open a MiniApp; opening must not request installation. Return navigation remains usable after installation.

## 3. Non-goals

No coordinate-tap workaround, dismissal of an accidentally opened MiniApp in acceptance, removal of navigation assertions, test-only presentation path, or change to Host install/permission/session ownership.

## 4. Requirements

R1: Scope an explicit independently hittable button style to the Marketplace subtree so List rows do not promote multiple automatic buttons to a row-wide action.
R2: Preserve existing action closures, accessibility identifiers, disabled states, permissions and installation path.
R3: Keep explicit open-global-dharma as the only Marketplace UI action that sets openedMiniApp. Installation leaves the Marketplace visible and its return control accessible.
R4: Keep all protected journey, commerce, session cleanup, Simulator erase, and ordinary acceptance gates.

## 5. Current state

ContentView.authenticatedContent selects marketplaceView. Its plugin rows contain open and install buttons; authenticatedContent owns the fullScreenCover. The test's failed back-button tap is a downstream symptom, not permission to weaken the test.

## 6. Target state

Marketplace alone uses an explicit borderless style for its independent buttons. Other product destinations keep their existing styles. Runtime execution and account state are unchanged.

## 7. Ownership

ContentView retains navigation/presentation ownership; MarketplaceModel and Rust Host retain installation and permission ownership. No new controller or parallel routing state.

## 8. Interfaces

No API or schema changes. Preserve install-<plugin>, open-<plugin>, marketplace-back and the existing canonical callbacks.

## 9. Constraints

All builds/tests run in GitHub Actions. No release or migration-completion claim from this UI fix or a historical artifact.

## 10. Failure modes

Installing while open is available must not invoke both closures. Disabled installation remains disabled. Permission prompts still require their original explicit response. An inaccessible back button must fail acceptance rather than be bypassed by coordinates or a fallback route.

## 11. Implementation

Apply .buttonStyle(.borderless) only to marketplaceView at the existing authenticated-content composition boundary. This also keeps other multi-action Marketplace rows independent without altering their business callbacks.

## 12. Verification

Retain GlobalDharmaJourneyUITests.testGlobalDharmaMarketplaceBotWebMcpCommerceJourney unchanged: actual install tap, return via marketplace-back, open the installed Bot, and complete real WebMCP/commerce/restore assertions. Confirm no MiniApp is opened by the install tap in same-HEAD behavior evidence. Ordinary Swift Unit/UI and protected acceptance must both pass; inspect same-HEAD artifact provenance.

## 13. Acceptance criteria

AC1: Install and open are independently activated in the shipping Marketplace.
AC2: Installation does not cover or replace Marketplace; protected navigation remains usable.
AC3: Both complete workflows and artifact provenance correspond to the fixed exact HEAD.

## 14. Release / rollback

No data migration. Reverting the style can restore the known multi-action defect and must not be called a tested fix.

## 15. Observability

Record exact source SHA, run/attempt, native UI test logs and video/screenshot evidence. The old failure artifact is diagnostic only.

## 16. References

- frontend/src/recovered/features/app-shell/ContentView+Root.swift
- frontend/src/recovered/features/conversation/ContentView+ChatMarketplace.swift
- mobile/ios/FabushiUITests/GlobalDharmaJourneyUITests.swift
- Protected diagnostic artifact 11456822696

## 17. Compliance record

R1-R4 and AC1-AC3 are pending implementation/current-HEAD acceptance. Historical passing Rust tests do not verify this UI behavior. This correction does not promote the automation_run_path ledger row or declare the broader migration complete.
