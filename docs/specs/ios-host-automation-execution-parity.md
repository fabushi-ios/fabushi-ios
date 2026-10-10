# iOS Host routine execution and recovery - Specification

Status: active
Owner: Fabushi iOS / existing Rust FeatureHostController
Last updated: 2026-10-07
Related PR: fabushi-ios/fabushi-ios#3

## 1. Context / problem

Direct authority is bhrumom/fabushi-desktop PR #20 at 798cf51d96cb1cb98cf657af212bb47274ccb701, not Grok directly and not an unmerged Desktop branch. At iOS 2f2815c51559c294dda2403ac918f9354a1868fa manual and scheduled routines share an untyped visible ChatSend path, listener events bypass the run-history owner, and automation_operations only retains a process-local (automation, run) pair. Existing automation UI is not evidence of execution/recovery parity.

## 2. Goal

Port the applicable automation_run_path.rs responsibility into the existing iOS-owned Rust Host and canonical Runtime, retaining hidden routine identity, trigger identity, admission/deduplication, durable run settlement, and account-safe native lifecycle recovery. Keep the corresponding Desktop ledger row mapped until implementation and exact-HEAD behavior evidence close the entire responsibility.

## 3. Non-goals / out of scope

No shared runtime repository, alternative scheduler, SwiftUI-owned queue, test-only production bypass, relaxed assertions, or silent replay of arbitrary tool/Shell execution. This spec does not declare the other 7,943-row responsibilities or release acceptance complete.

## 4. Requirements

- R1: Manual UI commands, due schedules and verified listener events converge on one Host routine admission/dispatch owner. The execution trigger is manual, schedule or event; it is not inferred from the saved trigger definition or a caller-controlled request-id prefix.
- R2: Routine provider input starts with [routine] and is submitted as hidden=true with show_assistant_output=false through the existing Runtime SendMessage boundary. Explicit canonical SendMessage tool delivery remains the only user-facing routine output; routine input is not a user-authored message and does not use direct-user supersession.
- R3: Only non-event executions suppress an in-flight duplicate for the same agent and automation. A duplicate creates no second run, provider dispatch or fake terminal. Distinct agents/routines and event runs remain independent. Due-schedule bookkeeping advances without turning a duplicate into a second execution.
- R4: Assign collision-resistant run identity before dispatch. Capture exact agent, canonical conversation, authenticated account/Host generation, trigger and runtime operation identity. Run summaries distinguish execution trigger from the saved listener/schedule. Completion, interruption and failure settle the matching run at most once; a stale terminal cannot settle another run.
- R5: Persist admission and pending terminal state before acknowledging the corresponding durable transition. Recreate restores only eligible work for the same authenticated account and original canonical conversation. Interrupted/dispatched work must use reviewed native checkpoint recovery, never blind prompt or Shell replay.
- R6: Native lifecycle signals feed the existing Host. Quiesce stops new dispatch, captures pending wake/completion and active-run recovery intent, then interrupts owned execution. Same-account recreate restores/rearms eligible work before clearing the quiesce fence. Logout/account replacement revokes the old generation and discards its runnable wake/completion ownership.
- R7: Completion revival for subordinate work preserves original parent/run/account/conversation ownership and quiet standing-order delivery semantics. Deleted agents/conversations and mismatched generations are rejected. Old completion cannot revive into the currently selected unrelated conversation.
- R8: Event payloads are external data, not instructions. Bound event batches and escape delimiter/control characters. Manual Run now on an event-configured routine remains manual and must not fabricate an external event.
- R9: All builds/tests execute in GitHub Actions. Old-HEAD or partial green jobs cannot promote this responsibility. Keep all existing assertion/gate strength.

## 5. Current state

The canonical shipping owner is source/packages/mahayana-rs/mahayana-feature-host/src/implementation.rs. AutomationRun invokes visible production_chat; fire_due_automation invokes the same untyped command; ingest_listener_event directly invokes production_chat and does not use AutomationRunSummary. Runtime already supports hidden input and exact operation terminal events. Account switching already resets runtime/state but does not implement durable automation carry/revival. No recovery completion is claimed here.

## 6. Target state

One subordinate routine-execution module under FeatureHostController supplies typed admission and hidden dispatch. FeatureState remains the canonical owner. Persistence and lifecycle recovery extend that owner rather than creating a parallel scheduler/runtime. The existing Runtime/Kernel/NativeEngine retain operation execution, canonical transcript, provider and checkpoint ownership.

## 7. Architecture and ownership boundaries

SwiftUI/Coordinator signal commands and scene lifecycle only. FeatureHostController owns routine definitions, execution contexts, in-flight suppression, account/conversation fencing, and projection of terminal results. Runtime owns actual operation execution and NativeEngine checkpoints. Verified listener bridges alone supply EventCard input. A module split is not a second controller or owner.

## 8. Interfaces / contracts / schemas / data flow

Public AutomationRun means manual. Scheduler invokes a typed schedule request; verified listener ingress invokes a typed event request. Admission returns the original CommandAccepted request identity and optional canonical operation id. A retained run context ties agent + automation + run + trigger + conversation + account generation to that operation. Hidden SendMessage carries stable client identity, no selected images, no fork/reply relation, and no synthesized visible user turn. Run history is updated by the same terminal owner. Existing persisted automation definitions/run history remain readable.

## 9. Constraints and non-functional requirements

Do not change source authority, ownership topology, account credential boundaries, approval policy or direct-user turn semantics. Do not store credentials in recovery records. Recovery state must be bounded and fail closed on malformed/mismatched identity. Never hold unrelated filesystem/network operations inside a lock without checking reentrancy and settlement races.

## 10. Failure modes and edge cases

Cover parallel manual/schedule admission, event overlap, terminal arrival before acceptance tracking, duplicate terminal, dispatch failure, agent deletion, canonical-conversation replacement, logout/relogin, account switch, scene quiesce, interrupted Host recreation, persistence failure, pending-completion restoration and stale old-generation completion. A lost/ineligible dispatched operation is not silently reported successful or blindly rerun.

## 11. Implementation strategy

First close production admission/hidden dispatch/typed identity and exact terminal fencing together, including tests invoking shipping owners. Then add durable wake/completion and checkpoint-qualified quiesce/recreate recovery, followed by subordinate completion revival. Keep uncovered requirements explicit and the ledger mapped throughout. Each new HEAD obtains its own ordinary and protected acceptance; neither workflow alone is completion evidence for this spec.

## 12. Verification / test strategy

Focused Rust tests exercise the shipping admission method for manual/schedule/event identity, duplicate suppression and independent keys; hidden Runtime command shape and output policy; idempotent terminal settlement and stale account/conversation rejection; existing history/persistence compatibility. Production Runtime integration must prove that routine input is absent from visible user history. Recovery tests must exercise actual durable paths and same-account recreate, and prove logout/account switch never restores prior work. GitHub Actions retains architecture, canonical rust-host, iOS-owned Host/internal/runtime, native contracts, Swift Unit/UI, device archive/package/upload and real protected acceptance including cleanup/Simulator erase.

## 13. Acceptance criteria / Definition of Done

- AC-1: All three production entry points use one typed routine owner and real hidden Runtime dispatch.
- AC-2: Non-event deduplication and exact/idempotent terminal settlement pass focused shipping-path tests.
- AC-3: Durable wake/run/completion identity survives eligible same-account recreate; interrupted execution resumes only through qualified checkpoint ownership.
- AC-4: Quiesce, logout/account switch, deletion and stale completion fences pass real lifecycle tests.
- AC-5: Pending subordinate completion revival preserves quiet delivery and original parent/run/conversation identity.
- AC-6: The exact candidate HEAD completes both full workflows and inspectable archive/protected artifact provenance. Production behavior evidence covers every applicable requirement before ledger promotion.

## 14. Release / migration / rollback

Preserve existing automation definitions and run-history readability. New state must use an explicit version and account scope. A rollback must not reinterpret pending/dispatched work as an unexecuted user request; unsupported recovery records fail closed. Publishing/install/upgrade/TestFlight/App Store evidence remains independently required.

## 15. Observability / evidence

Record immutable commit and Desktop authority, focused test names/results, workflow/run/attempt/actual checkout SHA and artifact name/head_sha/digest/internal provenance. Running, failed and superseded CI remain distinguished. Admission and terminal projection expose the saved run identity and execution trigger without credentials.

## 16. References / provenance

- docs/specs/fabushi-desktop-pr20-ios-architecture-parity.md
- Desktop authority source/host/src/extensions/transcript/automation_run_path.rs
- Desktop authority source/host/src/extensions/transcript/automation_runtime.rs
- Desktop authority source/host/src/extensions/transcript/production_runtime.rs
- Desktop authority source/host/src/extensions/transcript/pending_wake_rearm.rs
- Desktop authority source/host/src/extensions/transcript/sand_pending_wake_store.rs
- Desktop authority source/host/src/extensions/transcript/completion_revivals.rs
- iOS existing FeatureHostController, RuntimeCommand::SendMessage and KernelConversationProvider

## 17. Spec compliance record

| Requirement / AC | Status | Evidence / reason |
| --- | --- | --- |
| R1-R4, R8 / AC-1, AC-2 | pending | Shipping-path implementation and same-HEAD tests required. |
| R5-R7 / AC-3, AC-4, AC-5 | pending | Complete durable/checkpoint-qualified lifecycle and subordinate revival not yet implemented or evidenced. |
| R9 / AC-6 | pending | New candidate requires its own full ordinary and protected acceptance and artifact inspection. |

No requirement is passed merely by this specification or an existing automation UI. The source automation_run_path.rs row remains mapped.
