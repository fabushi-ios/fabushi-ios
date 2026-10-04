# Fabushi Desktop PR #20 -> Fabushi iOS Standalone Architecture & Product Parity — Specification

Status: active  
Owner: Fabushi iOS  
Last updated: 2026-10-04
Related PR: `bhrumom/fabushi-ios#3`

## 1. Decision and authority

Fabushi iOS is a **standalone downstream iOS implementation of Fabushi Desktop PR #20**.

The direct migration source and product/architecture authority for this work is:

- source repository: `bhrumom/fabushi-desktop`
- source pull request: `#20`
- source branch: `refactor/grok-018-architecture-rebuild`
- pinned source commit for this baseline: `8556281e5eb20aeb6dba781bd5ecf8c623206d6e`
- source specification: `docs/specs/grok-bot-018-runtime-product-parity-recovery.md`

The previous direct iOS baseline, `b-nnett/grok-bot-0.18-reconstructed@a9f633e09d49a85829b8236331b9e21f7e612634`, is **no longer the direct iOS migration authority**. Grok Bot 0.18 remains historical architecture/provenance context because Desktop PR #20 itself derives from that work, but iOS parity, implementation status, completion, and acceptance are judged against the pinned Desktop PR #20 source and product behavior.

Authority chain:

```
Grok Bot 0.18
    |
    | historical architecture / provenance
    v
Fabushi Desktop PR #20
    |
    | direct iOS migration authority
    v
Fabushi iOS PR #3
```

If Desktop PR #20 moves to a new exact HEAD, all Desktop-bound source inventory, blob identities, stale implementation-status claims, and acceptance evidence tied to the old SHA must be revalidated before they can be used for the new baseline.

### 1.1 Exact-HEAD rebaseline: 2026-10-03 / `486c478c6ca5e511de989190c63ec9b35d2ff2a6`

The live Desktop PR #20 HEAD moved again after the previous `f2a2e1811bbd2f1d7677672fd4b88f48e54b4cef` baseline. The current authority is `486c478c6ca5e511de989190c63ec9b35d2ff2a6`; all Desktop-bound manifest chunks, ledger chunks, blob identities and the strict architecture checker are rebound to that SHA before further parity promotion. The selected `frontend/** + source/**` inventory remains 7,926 paths.

The `f2a2e181 -> 4166a73b` Desktop delta changes eight Host source/contract files. The current production ownership that iOS must follow is:

- `TranscriptManager` owns the memory gateway for `getAgentMemories`, `deleteAgentMemory`, and `clearAgentMemories`; memory mutations invalidate the persisted agent-memory prompt snapshot before returning success. Shipping Host routing checks that manager-owned memory gateway before lifecycle and generic session fallbacks.
- Created-agent lifecycle and kickstart gateway dispatch is routed through `TranscriptManager`; shipping Host still injects the production kickstart/deletion runtime adapters, so lifecycle ownership moves behind the manager without creating a second executor.
- Session/group dispatch remains manager-owned and follows the same ordering constraint: specialized memory/lifecycle owners win before generic session fallback.
- ForeverBox disk-pressure watch installation/replacement is a lifecycle-owned resource; replacing a watch disposes the previous watch. This is a Desktop mechanism whose iOS applicability must be judged by the product effect and iOS lifecycle replacement, not by copying a desktop daemon.

No affected parity row is promoted merely because its `sourceCommit` or blob SHA was refreshed. The `d7b12fe6 -> 486c478c` delta additionally makes `TranscriptManager` the single durable pending-wake rearm owner: shipping Host injects the production runtime only after Gateway/Runner resources are live, duplicate binding fails closed, durable wakes are replayed through the manager, and dispose drops the rearm owner. The subsequent `4166a73b -> d7b12fe6` single-file change only makes the shipping `box_store_sync_deletion_slot` explicitly typed as `ProductionBoxStoreSyncApi`; this preserves the existing Host deletion owner and does not create a second lifecycle owner. Memory/transcript/group/lifecycle/ForeverBox rows require current iOS production wiring plus focused same-HEAD evidence before `verified`.

### 1.2 Exact-HEAD rebaseline: 2026-10-03 / `636b5a35ee1e9e30354052fcbf19c769add4e00b`

Desktop PR #20 advanced from `486c478c6ca5e511de989190c63ec9b35d2ff2a6` to `636b5a35ee1e9e30354052fcbf19c769add4e00b` in two source-bearing Host commits. The selected `frontend/** + source/**` inventory remains exactly 7,926 paths; all chunk `sourceCommit` values, every `desktop_blob_sha`, the index, and the strict checker are rebound to this exact HEAD before parity work continues. No status is promoted merely by rebasing source identity.

The first commit, `76e48fd8399ae86f9154dbcba7bc4faec50a68a2`, makes `TranscriptManager` the shipping facade for upgrade/recreate lifecycle state. Created-agent kickstart readiness reads manager-owned quiescing state; created-agent and background-revival paths mark durable upgrade-resume intent through the manager; Gateway health and prepare-upgrade read running turns/quiescing/carryable pending-wake state through the manager; box-store idle runtime and Host-upgrade composition receive the same manager owner; and resume-after-recreate is exposed through that owner. iOS must preserve the responsibility rather than emulate Electron upgrade machinery: the single Host owner must receive native scene suspend/resume lifecycle, quiesce new work while the scene is not runnable, retain durable recovery intent outside presentation state, and resume/replay only after Coordinator/Host/Runner resources are ready. SwiftUI and Coordinator may signal lifecycle but may not become a parallel canonical owner.

The second commit, `636b5a35ee1e9e30354052fcbf19c769add4e00b`, moves ForeverBox handoff state and hand-back settlement behind `TranscriptManager`. Request start, pending lookup, forget-on-failed-persistence, hand-back decision, transcript request resolution, awaiting-user settlement, and roster projection must be performed by the same transcript-adjacent owner; shipping Gateway/SendMessage paths call that owner rather than `BoxHandoffService` directly. On iOS, any applicable remote/box handoff must therefore be represented in the canonical Host controller/runtime owner with one settlement path. UI/Swift lifecycle code may request or project handoff state but must not own pending handoff truth or transcript settlement.

The changed Desktop rows (`source/host/app/src/main.rs`, `source/host/src/extensions/transcript/transcript_manager.rs`, `source/host/tests/transcript_manager_contract.rs`) remain at their existing non-verified status until current iOS production wiring plus focused contracts and same-iOS-HEAD CI prove the adapted ownership.

### 1.3 Exact-HEAD rebaseline: 2026-10-03 / `7b41050434d76ac2a18b3b042b4dcb018122b414`

Desktop PR #20 moved again from `636b5a35ee1e9e30354052fcbf19c769add4e00b` to `7b41050434d76ac2a18b3b042b4dcb018122b414` before the iOS rebaseline was committed. The intermediate `636b5a35` baseline is therefore historical only. The selected inventory remains exactly 7,926 paths and all manifests, ledger blob identities, index authorities and the strict checker are rebound again to `7b410504`.

This two-commit delta tightens the same ownership change rather than adding a second product subsystem. The pending-wake production contract is strengthened, and the shipping routed-provider `ProductionSendMessageSink` no longer retains a parallel `BoxHandoffService`; it receives the canonical `TranscriptManager` and starts/forgets handoff state through that owner. This confirms that box-handoff ownership is not merely a Gateway projection rule: send-side production composition must also be free of a parallel handoff owner.

For iOS, the audit result is fail-closed: the current single `FeatureHostController` remains the canonical Host/transcript-adjacent composition owner and Swift/Coordinator do not own box-handoff truth, but the current iOS command surface does not yet expose the Desktop ForeverBox request/help + hand-back state machine. Therefore the affected rows remain non-verified and no placeholder facade is introduced merely to satisfy the ledger. The applicable remote/box handoff product effect must be implemented on a real shipping command/runtime path before those rows can advance. Upgrade/recreate semantics continue to map through the native Coordinator/Host lifecycle and durable Runtime recovery rather than an Electron-style updater.

### 1.4 Exact-HEAD rebaseline: 2026-10-03 / `dbf35f981c18d6eefef6a3fab4b438165c7e3b99`

Desktop PR #20 advanced one commit from `7b41050434d76ac2a18b3b042b4dcb018122b414` to `dbf35f981c18d6eefef6a3fab4b438165c7e3b99`. This commit modifies only `projects/grok-fabu-parity/architecture-manifest.json`; the authoritative `frontend/** + source/**` inventory remains exactly 7,926 paths with unchanged source blobs. Even so, all iOS sourceCommit authorities, manifests, ledger chunks and the strict checker are rebound to the new exact HEAD before further production work.

The upstream manifest now records exact-head Desktop acceptance that `TranscriptManager` is the single shipping composition/delegation owner for Session/runtime/Runner/ack, automation/background wakes, Shared Rooms/group, WidgetResponses, WorkflowCommands, client-side-tool-v2, pending-wake rearm, upgrade/recreate resume and Box handoff, with services bound once and manager dispose settling lifecycle. It also marks Desktop WidgetResponses and WorkflowCommands implemented with focused production contracts. These are upstream evidence changes, not automatic iOS parity: iOS rows retain their independent status until native production composition and same-iOS-HEAD tests prove the corresponding responsibility.

The in-progress iOS Box handoff work remains applicable: the latest Desktop authority still requires one transcript-adjacent owner, no parallel send-side handoff owner, turn cancellation while awaiting the user, exact pending identity, and manager-owned hand-back settlement/resume.

### 1.5 Exact-HEAD rebaseline: 2026-10-03 / `c316a1ccac2d93810224dcfdd1085997a5dab452`

Desktop PR #20 advanced from `dbf35f981c18d6eefef6a3fab4b438165c7e3b99` to `c316a1ccac2d93810224dcfdd1085997a5dab452` with shipping Host changes in `source/host/app/src/main.rs`, `source/host/src/extensions/transcript/turn_runtime.rs`, and the focused recovery contract. The complete selected `frontend/** + source/**` inventory remains exactly 7,926 paths, but the three changed source blobs invalidate prior parity judgments for those responsibilities; every manifest/ledger blob identity, index authority and the strict checker is rebound to this exact HEAD before further migration.

The new normative responsibility is bounded reply delivery recovery. After a successful visible user turn, if no user-facing SendMessage was delivered and no reaction was delivered, the canonical routed-turn owner may run a hidden reply nudge and retry the provider at most three times. A nudge is forbidden for hidden/background/resume turns, after cancellation, while the turn is WaitingUser, after provider failure, after a newer turn epoch supersedes the original turn, or once any message/reaction delivery satisfies the turn. Nudge plain-text deltas are suppressed so only the real delivery channel satisfies the user-visible result obligation.

This state machine is especially coupled to Box handoff: a turn that yielded to `request_box_help` is WaitingUser and must never be nudged while the user owns control. iOS may adapt the mechanism to its native Runtime/provider boundary, but it must retain one canonical delivery-obligation owner, a bounded retry count, stale-generation fencing, cancellation/waiting-user fences, and no renderer-owned retry policy.

The current iOS adaptation is shipping-path owned rather than renderer-owned. `MahayanaRuntime` passes hidden-turn identity into the default `MobileEmbedded` `NativeEngine`; the engine buffers plain model text for visible user turns, treats only a successful `send_message` as message delivery, retries an undelivered visible turn with at most three private nudges, and uses Runtime operation interruption/supersession as the stale-generation fence. `request_box_help` is also a native tool and terminates the current loop after emitting the Host-facing handoff request, so waiting-user work cannot fall into reply nudges. Hidden raw assistant deltas/completions are filtered at the Runtime bridge while successful `send_message` remains a canonical visible message. Focused htch-runtime contracts for bounded delivery recovery and handoff suppression pass; the rows remain `implemented`, not `verified`, until the same committed iOS HEAD passes the full GitHub acceptance set.

### 1.6 Exact-HEAD rebaseline: 2026-10-03 / `4060b1f5a0aaf64029739dc18e9d03e05c1f3838`

Desktop PR #20 advanced from `c316a1ccac2d93810224dcfdd1085997a5dab452` through `9a0c6d1d786aa7c4e97645e94ca69ed5534ed501` to `4060b1f5a0aaf64029739dc18e9d03e05c1f3838`. The selected `frontend/** + source/**` inventory remains exactly 7,926 paths. Four source blobs changed (`source/host/app/src/main.rs`, `source/host/src/extensions/transcript/turn_runtime.rs`, `source/host/tests/box_handoff_resume_contract.rs`, and `source/host/tests/turn_runtime_recovery_contract.rs`), so every Desktop-bound sourceCommit authority and those blob identities are rebound to this exact HEAD before parity claims continue. No row is promoted merely by this source-identity refresh.

The `9a0c6d1` contract-only change confirms the already normative box-handoff owner: shipping Host invokes the TranscriptManager hand-back facade, while the transcript-adjacent manager owns durable handoff settlement/request tracking. The iOS adaptation therefore keeps `FeatureHostController` as the single handoff state/settlement owner and treats SwiftUI/Coordinator as request/projection surfaces only.

The `4060b1f5` shipping change closes a subtler reply-nudge checkpoint bug. A synthetic reply nudge may retain the same inference request lineage, but it must not inherit the original visible user turn's message/reply/fork/attachment checkpoint identity. The iOS native engine now marks successful sends produced by a synthetic reply nudge; the canonical transcript bridge suppresses inherited reply target, fork branch, and attachment-batch identity for those sends, while still honoring an explicit valid `reply_to_message_id` chosen by the SendMessage tool itself. This keeps recovery delivery attached to the current operation without falsely projecting it as the original user's reply/fork/attachment batch. Focused contracts cover both the engine marker and transcript projection; current rows remain `implemented`, not `verified`, until this committed iOS exact HEAD passes the full GitHub acceptance set.

### 1.7 Exact-HEAD rebaseline: 2026-10-03 / `827f22da7c527ab22df0700303f356d589c2a0f4`

Desktop PR #20 advanced one commit from `4060b1f5a0aaf64029739dc18e9d03e05c1f3838` to `827f22da7c527ab22df0700303f356d589c2a0f4`. The only source change restores the `REPLY_NUDGE_PROMPT` import in shipping `source/host/app/src/main.rs`; it repairs the Desktop build after the prior reply-nudge refactor and does not change the normalized ownership or state machine. The selected `frontend/** + source/**` inventory therefore remains 7,926 paths, with only the `main.rs` blob identity changing relative to `4060b1f5`. All sourceCommit authorities and blob identities are rebound to `827f22da` before iOS acceptance continues.

The iOS adaptation from the preceding rebaseline remains the current semantic match: synthetic reply-nudge sends preserve inference-operation lineage but drop inherited user message/reply/fork/attachment checkpoint identity, and Box handoff remains owned by the transcript-adjacent FeatureHostController. No parity status is promoted from this Desktop compile-only fix; same-iOS-HEAD CI remains required.

### 1.8 Exact-HEAD rebaseline: 2026-10-03 / `b5f8855805ec1c0be3a821cf35a4cba047ee9d8b`

Desktop PR #20 advanced one commit from `827f22da7c527ab22df0700303f356d589c2a0f4` to `b5f8855805ec1c0be3a821cf35a4cba047ee9d8b` in `source/host/app/src/main.rs`, `source/host/src/extensions/transcript/roster_emit.rs`, `source/host/src/extensions/transcript/transcript_manager.rs`, and `source/host/tests/transcript_manager_contract.rs`. A fresh recursive Git-tree comparison confirms that the selected `frontend/** + source/**` inventory is still exactly 7,926 blobs with no added, removed, or stale paths. All manifest/ledger chunks, both indexes, the strict checker, and the four changed Desktop blob identities are rebound to this exact HEAD before iOS production work continues. Three pre-existing source-host manifest size fields whose blob identities did not change are also corrected from the current Desktop Git tree; they do not represent new upstream responsibilities.

The new normative responsibility is a success-terminal automation refresh owned entirely by the Host transcript composition. After a routed provider turn has completed its result settlement successfully, shipping Host calls `TranscriptManager.emit_automations(agent_id)`; failed routed turns do not call it. The manager reads the current Agent automation records through its canonical `AutomationRuntime`/session-store owner and passes only the resulting projection to `ProductionRosterEmit`. The roster surface re-reads the canonical active Agent at emission time and emits `automations { agentId, automations }` only when the completed turn's Agent is still active. Therefore a Session/Agent switch between dispatch and terminal settlement suppresses the stale projection. Projection failure does not rewrite the already settled turn result.

This ownership also defines the duplicate/recovery rules that the iOS adaptation must preserve. A successful turn may cause at most one terminal-adjacent automation snapshot for its owned operation identity; duplicate/late terminal observation must not re-project it. Interruption/provider failure must not project a success snapshot. Relaunch/recovery must source the snapshot from durable Host automation state after account/session restoration rather than a renderer cache. The renderer may consume the Agent-tagged projection but may not decide which automation state is canonical or whether the turn qualifies.

Current iOS audit at `1ab164e064183473110194fdd33c1de39e831da4`: `FeatureHostController` is already the single Host automation CRUD owner; account-scoped automation persistence is reloaded on restored authentication, and the Host owns both `operation_agents` identity and `ConversationSessionState.active_conversation_id`. However, the normal `RuntimeEvent::OperationCompleted` path currently removes `operation_agents` and returns `operation.completed` without publishing an Agent-tagged automation snapshot. Failure/interruption paths likewise terminate without such a snapshot, which is correct for failure but exposes the missing success behavior. This is a real shipping-path gap, not a documentation gap. The iOS fix must extend the canonical Rust Host terminal path, carry Agent identity in the projection, gate it against the Host-owned active conversation/Agent mapping at terminal time, and consume the operation-to-Agent identity so duplicate terminal events cannot duplicate the projection. No SwiftUI/renderer fallback or second automation store is permitted.

The four affected ledger rows remain `mapped` through this rebaseline. They may advance only after the iOS production path and focused contracts prove success-only active-Agent projection, inactive/session-switch suppression, duplicate-terminal suppression, failed/interrupted suppression, and persisted automation reload semantics, followed by same-iOS-HEAD required CI.\n\nThe subsequent iOS implementation uses that exact ownership: the existing Rust `FeatureHostController` consumes its operation-to-Agent identity on successful terminal settlement, resolves the current active Agent from Host-owned conversation state, and queues an Agent-tagged `automations` transport projection from the same Host-owned persisted automation map only when the identities still match. Duplicate terminals cannot re-project after the identity is consumed; failed/interrupted turns do not enter this path. The renderer remains a consumer, not an automation or settlement owner. This implementation is `implemented` until the resulting exact iOS HEAD passes the required CI/acceptance gates.

### 1.9 Exact-HEAD rebaseline: 2026-10-03 / 61f8518a5e0b4bead224ec3c081da64523316908

Desktop PR #20 advanced two commits from b5f8855805ec1c0be3a821cf35a4cba047ee9d8b to 61f8518a5e0b4bead224ec3c081da64523316908. The recursive Git tree still contains exactly 7,926 selected frontend/** + source/** blobs, and the Git comparison contains only eight modified source/host/** files: no selected path was added or removed. This rebaseline updates every manifest/ledger chunk authority, both indexes, the strict checker, and the eight changed blob identities before any new iOS parity claim is accepted.

The first upstream commit restores a distinct closing-send nudge for a visible user turn that already emitted an acknowledgement, then completed tool work but ended without a later SendMessage. ProductionTurnAgentOwner records ended_on_silent_tool_calls from the exact latest durable provider checkpoint owned by TurnAgentComposition. Cursor, Codex direct-responses, and OpenRouter checkpoint forms are normalized through turn_shape; a later visible send or final text outside the persisted checkpoint defeats the silent-tail predicate. The hidden closing nudge preserves the original inference request identity while clearing message/reply/fork/attachment identity. It is attempted only for an uncancelled, same-epoch, successful visible user turn that is not WaitingUser.

The second upstream commit reports ordinary empty delivery after reply/closing recovery has settled when a visible user turn still owes delivery. The report is fail-closed unless the run succeeded, was not cancelled, is not WaitingUser, is still on the same turn epoch, and delivered neither a SendMessage nor reaction. It captures bounded reply-nudge attempts, observed tool-call count, stream-output presence, run duration, and outstanding acknowledgement state. Telemetry failure is non-fatal to settlement.

The iOS active-Agent automation projection implemented immediately before this rebaseline remains an applicable Host-owned behavior. However, the broader Desktop main responsibility now includes closing-send recovery and ordinary empty-delivery reporting. Current iOS source has no corresponding closing-send prompt, durable silent-checkpoint terminal fact, or empty-delivery report path, so all eight changed rows are conservatively mapped. The next production slice must preserve Coordinator/Host/Runner ownership: checkpoint observation belongs with Runner/runtime, terminal recovery policy belongs in canonical Host/runtime code, telemetry is a projection of those facts, and SwiftUI/renderer must not synthesize them.

### 1.10 Exact-HEAD rebaseline: 2026-10-03 / ed06d2c96471d5e80b66cd3c4b09116e968a0e56

Desktop PR #20 advanced one evidence-only commit from 61f8518a5e0b4bead224ec3c081da64523316908. The selected frontend/** + source/** inventory remains exactly 7,926 blobs; the only selected source change is source/host/tests/turn_telemetry_production_wiring_contract.rs. The other change is Desktop's architecture manifest, which now marks the broader TurnRuntime responsibility implemented and explicitly documents that it is absorbed by existing canonical Host/Runner owners instead of a parallel TurnRuntime owner.

The focused contract tightens one semantic point for iOS: closing-send delivery and terminal delivery share the same canonical delivery predicate, including successful reaction delivery as satisfying delivery debt. The current iOS NativeEngine already owns real send_message delivery and bounded reply nudges, but has not yet implemented the 61f closing-send/ordinary-empty-delivery slice or proved one shared delivery predicate. This changed evidence row therefore remains mapped. Unchanged automation rows keep their implemented status, and the next production change must extend the existing NativeEngine/Host composition rather than add renderer or duplicate runtime ownership.

### 1.11 iOS production adaptation: closing-send and empty-delivery ownership

The iOS shipping adaptation keeps Mahayana NativeEngine as the Runner/Agent-loop owner. Visible user turns already suppress plain model prose, perform at most three reply nudges, and treat a successful send_message as delivery. The new production path records tool work after a delivery and, if that work would otherwise terminate in hidden plain text, inserts exactly one operation-scoped closing-send nudge into canonical NativeSession.history. The marker carries the operation id and is persisted with the runtime session, so recovery/resume cannot create a second closing nudge for the same operation. Suspension/interruption fence both reply and closing nudges; request_box_help still terminates as waiting-user before closing recovery.

If a visible user turn exhausts bounded reply nudges without a successful send_message, NativeEngine records content-free empty-delivery telemetry through RuntimeTelemetry, including aggregate nudge attempts, tool calls, stream-output presence, and duration. This is canonical runtime telemetry, not renderer state. Focused Rust contracts exercise the production owner. These 61f behavior rows are implemented pending exact-HEAD Actions. The ed06 production-wiring evidence row remains mapped until the separate iOS reaction owner is audited and wired into the same delivery-debt predicate; no reaction capability is fabricated inside NativeEngine merely to satisfy a Desktop string contract.

### 1.12 Exact-HEAD rebaseline: 2026-10-03 / 79a9867ac6cbfecc5af9fcc9ac81225890c20245

Desktop PR #20 adds one shipping lifecycle slice across five source/host files while the selected inventory remains exactly 7,926 blobs. The new resumeAfterRecreate path carries canonical upgrade-resume Agent identities plus durable pending wakes, restores the manager lifecycle state, restores eligible pending wakes, then resumes interrupted upgrade turns. Recreate status exports the same pending-wake carry.

Only CloudAgent and Shell pending-wake kinds may cross recreate, and carry can be disabled. Restore is canonical-Host owned: values are coerced/validated, ineligible kinds ignored, existing durable identities deduplicated, Shell markers flagged interrupted-by-recreate, newly persisted through the pending-wake runtime owner, then rearmed with recreate provenance. No renderer replay owns this state.

Current iOS source has no resumeAfterRecreate/pending-wake carry equivalent by name and must be audited by responsibility rather than copied gateway syntax. The immediately preceding NativeEngine closing-send/empty-delivery implementation remains real production work, but main.rs is mapped again because its upstream responsibility expanded. TranscriptManager's automation projection also remains a valid implemented sub-responsibility while its changed row returns to mapped for the new recreate carry gap. The iOS implementation must locate or add one durable lifecycle/wake owner below SwiftUI, preserve identity/idempotency across scene/app recreation, and resume interrupted work only after durable wake state has been restored.

### 1.13 Exact-HEAD rebaseline: 2026-10-03 / cb2267d52ad816287ad97a357e6e7d4135e79083

Desktop PR #20 adds the shipping quiesce half of the recreate lifecycle across eight source/host files while the selected inventory remains exactly 7,926 blobs. The runner registry now owns one shared upgrade-quiescing signal. Forced upgrade sets the signal and cancels active routed/group tasks; the signal is injected into production turn owners and generated-Agent persistence so a turn that is cancelled while upgrade quiesce is active settles with quiesced_for_upgrade=true. The manager clears this runner quiesce only during resume-after-recreate.

This extends, rather than replaces, the 79a recreate carry contract. Correct ordering is now: request quiesce, stop/settle active shipping runners with durable identity, carry pending durable work, recreate Host, restore/rearm carried work, then clear quiesce/resume. iOS cannot satisfy this by merely refreshing UI state or by restarting a Host generation with no work identity.

The current iOS NativeEngine already has persisted NativeSession state containing active_prompt and operation attempts plus resume_operation, but ordinary run() persists only after execution returns. Therefore an active process recreation can still lose the exact work identity, and there is no canonical shared upgrade-quiesce fence across the current Coordinator/Host generation boundary. All eight changed rows remain mapped. The iOS adaptation must first durably checkpoint the active prompt/operation before inference, then wire one generation-safe quiesce/resume path through existing Host/Coordinator owners; SwiftUI remains a lifecycle trigger only, never the source of runnable work truth.

### 1.14 Exact-HEAD rebaseline: 2026-10-03 / 5ec257a7920478b56a88bdf24b85eb845aacfb46

Desktop PR #20 advanced four commits from `cb2267d52ad816287ad97a357e6e7d4135e79083` to `5ec257a7920478b56a88bdf24b85eb845aacfb46`. The selected `frontend/** + source/**` inventory remains exactly 7,926 paths. Through `75824953`, only five existing Host files change; the final `5ec257a` commit changes only `projects/grok-fabu-parity/architecture-manifest.json`, so it adds upstream Desktop evidence but no new selected source blob. All manifest/ledger sourceCommit authorities, indexes and the strict checker are nevertheless rebound to the current exact HEAD.

The selected-source responsibility is recreate-carry safety ownership. `TranscriptManager` is the lifecycle facade but delegates carried values to canonical `PendingWakeRearm.restore_recreate_carried_pending_wakes`. That owner fails closed when carry is disabled or runtime execution is unavailable, deduplicates identities already durable on the replacement Host, suppresses gone Agents, group sessions and Subagent wakes, and treats session lookup failure as a failed restore rather than guessing. Only CloudAgent and Shell wakes cross recreate. CloudAgent work is rearmed with recreate provenance; Shell wakes are marked `interrupted_by_recreate` and produce an interruption notice instead of blindly re-running shell work. The shipping upgrade contract follows this owner.

The `5ec257a` parity-manifest-only commit records Desktop's own upgrade/recreate/resume row as implemented and expands its production/test evidence across TranscriptManager, Runner quiesce, pending-wake restore, Gateway and source-specific resume. iOS does not inherit that status: it remains independently gated by native production composition and exact-iOS-HEAD evidence.

Current iOS still lacks this complete recreate-carry owner and the preceding active-operation durability guarantee. NativeEngine has `active_prompt`/operation-attempt state and `resume_operation`, but ordinary execution persists the session only after the run returns. `feature.sessionActivity` reaches canonical Rust `FeatureHostController`, yet currently records focus/scene activity rather than a generation-safe quiesce/resume state machine. Therefore all five changed selected-source rows remain only `mapped`. The first production closure is normative: before the first model inference of a newly accepted operation, NativeEngine must assign the canonical `active_prompt`, create the operation attempt carrying the exact operation/prompt identity, advance the session durability timestamp, and atomically persist that complete NativeSession snapshot when a session state path is configured. A process/Host generation that disappears after inference begins must therefore leave enough durable identity for `open_session` + `resume_operation` to recover the same prompt rather than enqueueing a duplicate user turn. This checkpoint is Runtime-owned; SwiftUI and Coordinator cannot synthesize it. After that invariant is proven, native Host/Coordinator quiesce/recreate recovery must add the same safe wake filters, including no blind Shell replay.

### 1.15 Exact-HEAD rebaseline: 2026-10-03 / 9e0b1701d405ebd0b2707620c01781103562b39b

Desktop PR #20 advanced 30 commits from `5ec257a7920478b56a88bdf24b85eb845aacfb46` to `9e0b1701d405ebd0b2707620c01781103562b39b`. The selected `frontend/** + source/**` inventory remains exactly 7,926 paths: no selected path was added or removed. Twelve existing Host source/contract paths changed, plus Desktop's parity architecture manifest outside the selected inventory. The intermediate `8bf608904a9b5a8ec426e4ae1845df4f70280857` is not a valid iOS authority because the final `9e0b170` commit rewires shipping composition again in `source/host/app/src/main.rs` and `source/host/src/host_production_extensions.rs`.

Current Desktop shipping ownership is explicit. `host_gateway_api.rs` is the single frozen gateway method-to-owner registry; `gateway_protocol.rs` consumes that registry and keeps only the Fabushi `resumeAfterRecreate` compatibility command outside it. `ProductionHostExtensions` is now the canonical construction/lifecycle owner for the centralized TurnExecution registry plus Notifications, Session, AutoReview, Transcript, CrossUserSharing, Automations and MCP. Its slots reject duplicate starts and own stop/drop ordering. Shipping `main.rs` consumes those exact owners and wires them into routed provider/MCP, automation, transcript, cross-user and lifecycle paths rather than creating a second set. `TranscriptManager` remains the canonical transcript lifecycle facade: it owns transcript/session/send/Runner registries and observer projection, binds one `PendingWakeRearm`, delegates recreate-carried wake filtering/restoration to that durable owner, and orders quiesce/resume/dispose. Automation execution and completion revival remain subordinate production paths with exact wake/run identity and terminal/interruption settlement.

The iOS shipping topology still preserves the architectural boundary: SwiftUI is projection only; `IOSMainRuntime` forwards native lifecycle; `MahayanaCoordinator` is the only production caller allowed to invoke `MahayanaHostRuntime`; the Rust `FeatureHostController` owns canonical feature/runtime state. No SwiftUI, Coordinator or alternate Swift runtime was found constructing a parallel Host extension owner for the responsibilities above. That does not prove parity: current `feature.sessionActivity` only records scene focus/activity and does not yet implement the Desktop generation-safe quiesce -> durable recreate carry/filter -> resume state machine. Therefore all twelve changed selected-source rows are deliberately reset to `mapped`; no older implemented/verified status or old exact-head CI is inherited.

The current protected Global Dharma acceptance exposed a separate shipping auth restoration defect that must be fixed before any acceptance promotion. GitHub Actions successfully creates a bounded refresh-token-free Fabushi session and the UI test forwards its bytes to the launched Simulator app as `FABUSHI_CI_ACCOUNT_SESSION_BASE64`; the canonical Rust Mahayana product auth loader currently accepts only the host-side `FABUSHI_CI_ACCOUNT_SESSION_FILE` path, which the Simulator application cannot read. The iOS contract is therefore: CI application-session restoration remains owned by Rust product auth, never SwiftUI; it may consume exactly one bounded file or base64 transport only when `GITHUB_ACTIONS=true`; both transports must pass the same provenance, identity, size and lifetime validator; malformed, oversized, ambiguous or non-GitHub-Actions transport must fail closed; the bounded application session contains no refresh token. Swift/XCTest may transport the opaque bytes into launch environment but must not become an authentication or session-persistence owner.

### 1.16 Exact-HEAD rebaseline: 2026-10-03 / 9467112079551dc9ff64d40ba2c0bed0f62a114a

Desktop PR #20 advanced two commits from `9e0b1701d405ebd0b2707620c01781103562b39b` to `9467112079551dc9ff64d40ba2c0bed0f62a114a`. The selected `frontend/** + source/**` inventory remains exactly 7,926 paths; only two existing Host focused-contract files changed. No production source owner moved again, but the acceptance contract tightened, so both affected ledger rows are rebound to the current blob identity and reset to `mapped` rather than inheriting the preceding current-head judgment.

The first contract now proves that AutoReview startup stale-approval sweeping is composed through `ProductionHostExtensions.start_auto_review`, with the expire-sweep failure sink installed before the startup sweep and failures reported only through the Host structured-log owner. Shipping Host must consume this centralized AutoReview owner and must not construct a second extension; Coordinator and Electron telemetry remain non-owners.

The second contract strengthens the centralized production composition invariant: `CURRENT_SHIPPING_PRODUCTION_EXTENSION_IDS` must cover the exact 35 frozen Host extension slots exactly once, and no `NoopProductionExtension`/no-op placeholder may satisfy a frozen slot. The already-audited iOS architecture still has one Coordinator->Host boundary and a single Rust `FeatureHostController` rather than SwiftUI parallel Host owners, but this evidence-only upstream tightening does not promote iOS parity. The two changed rows remain mapped pending native shipping-path proof and same-iOS-HEAD acceptance.

### 1.17 Exact-HEAD rebaseline: 2026-10-03 / f6d4c48d113d02ec2e223c4fcbad1e6417549424

Desktop PR #20 advanced one evidence-only commit from `9467112079551dc9ff64d40ba2c0bed0f62a114a` to `f6d4c48d113d02ec2e223c4fcbad1e6417549424`. The commit changes only `projects/grok-fabu-parity/architecture-manifest.json`; no selected `frontend/** + source/**` path or blob changed, so the authoritative selected inventory remains exactly 7,926 paths and no source responsibility/status is promoted or demoted solely by this move. Nevertheless every manifest/ledger `sourceCommit`, both authority indexes, `MIGRATION_SOURCE.md`, and the strict checker are rebound to the new exact HEAD before further current-head parity claims.

The preceding production-source audit remains semantically current because all selected source blobs are identical. The iOS Rust CI-session transport fix is retained as production code, but acceptance results tied to its pre-rebaseline iOS SHA are not used as proof for the post-rebaseline exact HEAD; that exact HEAD must earn its own architecture, Rust, Swift/UI, archive and protected complete-state evidence.

Rebaseline generation evidence: GitHub Actions run `37076005927` completed successfully; its generation step, `git diff --check`, strict Desktop PR20 architecture checker, and final durable commit/push all succeeded before this provenance note was added. This run proves only the baseline generation/integrity operation, not the downstream product acceptance gates.

### 1.18 Exact-HEAD rebaseline: 2026-10-03 / `c96b56c47c6b2a478370839888124d1703aac518`

Desktop PR #20 advanced two production-source commits from `f6d4c48d113d02ec2e223c4fcbad1e6417549424` to `c96b56c47c6b2a478370839888124d1703aac518`. The selected `frontend/** + source/**` inventory is now **7,927** blobs rather than 7,926 because `source/host/tests/host_runner_composition_production_wiring_contract.rs` is newly selected. `source-host` therefore grows from 956 to 957 rows. The other changed selected paths are `source/host/app/src/main.rs`, `source/host/src/host_runner_composition.rs`, and `source/host/tests/sand_host_production_wiring_contract.rs`. All manifest/ledger `sourceCommit` authorities, indexes, changed blob identities, `MIGRATION_SOURCE.md`, and the strict checker are rebound to this exact HEAD before further product work.

The normative production change centralizes per-turn Runner assembly in `HostRunnerComposition.compose_production_turn(...)`. Shipping `main.rs` supplies the turn-scoped `ProductionRunnerCompositionInput` and `ProductionTurnCompositionHooks`, but the HostRunnerComposition owner alone calls `create_production_runner_composition` and applies agent-management, state-writer, routine auto-review, box-shell review, Subagent, routine-post-write, and multitask decorations in one deterministic order. A new production-wiring contract explicitly forbids shipping `main.rs` from constructing a second Runner composition or attaching those hooks itself. This is a shipping ownership rule, not merely a test refactor.

For iOS the mechanism may differ, but the responsibility does not: one canonical Host/Runner owner must assemble the production turn capability stack before execution; SwiftUI and the Coordinator may supply requests/platform capabilities but must not own a parallel Runner-decoration path. The current iOS tree already keeps SwiftUI outside canonical Runtime truth and routes production work through Mahayana Coordinator -> Host/Runtime, but current evidence does not yet prove a single native Runner-composition owner equivalent to this new Desktop contract. The four affected rows are therefore `mapped`, never inherited as implemented/verified. The next production audit must either identify one existing shipping owner and add focused behavior evidence, or consolidate any real parallel composition path before promotion.

### 1.19 Exact-HEAD rebaseline: 2026-10-03 / `854d4bffde64dee2d7f12ba093207381ef369af4`

Desktop PR #20 advanced two production-source commits from `c96b56c47c6b2a478370839888124d1703aac518` to `854d4bffde64dee2d7f12ba093207381ef369af4` with no selected path additions/removals, so the authoritative selected inventory remains exactly **7,927** blobs and `source-host` remains 957 rows. Three selected Host paths changed: `source/host/app/src/main.rs`, `source/host/src/host_runner_composition.rs`, and `source/host/tests/host_runner_composition_production_wiring_contract.rs`.

The production ownership introduced at `c96b56c` is expanded. `HostRunnerComposition` now owns not only the ordered Runner decorator stack but also production transcript/checkpoint sink construction and turn state-surface construction. It opens the canonical Agent store/blob store, constructs the production transcript mirror and generated-occurrence codec, builds the `ProductionAgentStateCheckpointSink`, and constructs memory-backed Agent state plus optional multitask todo state. Shipping `main.rs` consumes these composition entrypoints and is contractually forbidden from rebuilding the same state/checkpoint/Runner graph in parallel.

The iOS adaptation must preserve this single-owner rule rather than copy Desktop process details. SwiftUI and Mahayana Coordinator may transport lifecycle/request inputs, but canonical production checkpoint/state/Runner composition must remain in one Host/Runtime owner. Current iOS architecture has the correct high-level Coordinator -> Host boundary, yet the newly centralized checkpoint/state/Runner composition has not been proved on the shipping path. All three changed rows remain `mapped` pending focused production evidence and same-iOS-HEAD CI; no previous `implemented`/`verified` result is inherited.

### 1.20 Exact-HEAD rebaseline: 2026-10-03 / `2126a60a388b9bc44545f11eaa22889861314d39`

Desktop PR #20 advanced one production commit from `854d4bffde64dee2d7f12ba093207381ef369af4` to `2126a60a388b9bc44545f11eaa22889861314d39`. The selected inventory remains exactly **7,927** blobs and `source-host` remains 957 rows; only `source/host/app/src/main.rs`, `source/host/src/host_runner_composition.rs`, and `source/host/tests/host_runner_composition_production_wiring_contract.rs` changed.

The single-owner boundary is now complete through Runner facade construction. `HostRunnerComposition.compose_production_runner(...)` constructs `ProductionTurnAgentOwner`, attaches the production checkpoint sink and shared upgrade-quiesce signal, constructs `SandAgentRunner`, and attaches the generated-Agent runtime. Shipping `main.rs` delegates this construction and is contractually forbidden from directly rebuilding `ProductionTurnAgentOwner` or `SandAgentRunner`. This extends the already centralized turn decorators, transcript/checkpoint sink, and turn state surfaces.

The iOS responsibility is the same even though the native mechanism differs: one Host/Runtime owner must construct the complete production Runner graph and lifecycle fences. SwiftUI and Mahayana Coordinator remain request/lifecycle transport layers, not parallel Runner constructors. These three changed rows stay `mapped` until the current iOS shipping path is audited and same-iOS-HEAD focused/CI evidence proves the equivalent ownership.

### 1.21 Exact-HEAD rebaseline: 2026-10-03 / `1d9d9b72d2b6cf2a28d641159d68ffcb3c85dbef`

Desktop PR #20 advanced one production commit from `2126a60a388b9bc44545f11eaa22889861314d39` to `1d9d9b72d2b6cf2a28d641159d68ffcb3c85dbef`; selected inventory remains **7,927** and `source-host` remains 957. The same three Host paths changed.

`HostRunnerComposition` now also owns computer-use session lifecycle: preparation and control-lease acquisition, ready/failed preparation state, ownership checks, model/usage settlement at turn end, lease release, and window cleanup. Shipping `main.rs` delegates these operations and the focused contract forbids direct `ComputerUseCoordination` lifecycle ownership there. This extends the same single composition owner that already owns turn decoration, transcript/checkpoint sinks, state surfaces, and final Runner construction.

On iOS, computer-use or native-control mechanisms may differ, but control/session lifecycle truth must stay with the canonical Host/Runner owner. SwiftUI and Coordinator may transport intent and project state; they cannot own control leases, preparation truth, or terminal cleanup. The three changed rows remain `mapped` until the shipping iOS path and same-head focused/CI evidence prove this ownership.

### 1.22 iOS closure contract: canonical native Runner composition

For Desktop PR #20 `1d9d9b72d2b6cf2a28d641159d68ffcb3c85dbef`, `HostRunnerComposition` is the single shipping owner for production turn decoration, transcript/checkpoint sink construction, turn state surfaces, final `ProductionTurnAgentOwner`/`SandAgentRunner` construction, and computer-use preparation/control/settlement lifecycle.

The iOS production analogue must be explicit in the shipping Rust Host composition, not inferred from presentation code. `MahayanaHost::build_runtime` is the composition root and must construct exactly one native Runner graph whose `NativeEngine` instance is shared by the Runtime engine backend and the `NativeAgentBackend`. NativeEngine remains the sole owner of operation control, prompt/attempt durability, model loop, approvals, tool execution, checkpointable session state, retry/delivery recovery, and terminal settlement. The mobile turn-execution extension may expose a Host-bound adapter for Desktop-derived extension contracts, but it must not construct a second model/tool/session engine or own parallel durable turn truth.

The closure must therefore provide a real Host-owned composition object used by `build_runtime` on the shipping MobileEmbedded path, prove that the engine and Agent surfaces share the same NativeEngine owner, and keep SwiftUI/Coordinator outside Runner construction. Focused Rust tests must exercise that production composition seam. Only after the same iOS exact HEAD passes the required architecture/Rust/Swift/UI/archive/protected-session gates may the affected Desktop rows advance beyond `implemented`.

### 1.23 Exact-HEAD rebaseline: 2026-10-03 / `612a075e7a7de5d845f765ade4eb51c7c318e3c8`

Desktop PR #20 advanced one commit from `1d9d9b72d2b6cf2a28d641159d68ffcb3c85dbef` to `612a075e7a7de5d845f765ade4eb51c7c318e3c8`. The selected inventory remains **7,927** and `source-host` remains 957; only `source/host/app/src/main.rs` changed. The production delta is a worker-closure capture correction: the asynchronous turn worker now calls the cloned `worker_host_runner_composition` for `compose_production_turn` and `compose_production_runner` rather than referencing the outer binding. No Host/Runner ownership, lifecycle state machine, or product effect changed.

The iOS native Runner composition closure defined in Section 1.22 therefore remains the correct adaptation. This rebaseline only refreshes Desktop authority/blob identity; it does not promote the affected row. Same-iOS-HEAD production/test evidence remains required.

### 1.24 Exact-HEAD rebaseline: 2026-10-03 / `9f22a1974f02683f4ad27a2fe856987b7dd1d20f`

Desktop PR #20 advanced one contract-only commit from `612a075e7a7de5d845f765ade4eb51c7c318e3c8` to `9f22a1974f02683f4ad27a2fe856987b7dd1d20f`. The selected inventory remains exactly **7,927** blobs and `source-host` remains 957 rows. The only selected blob change is `source/host/tests/host_runner_composition_production_wiring_contract.rs`.

The strengthened contract now explicitly requires the asynchronous shipping turn worker to consume the same `HostRunnerComposition` owner through `Arc::clone(&host_runner_composition)`, then invoke `worker_host_runner_composition.compose_production_turn(...)`. No shipping source, owner, state machine, or product effect changed. The iOS `MahayanaHost::build_runtime -> NativeRunnerComposition` implementation remains the reviewed platform adaptation because it likewise constructs one shared native Runner graph and passes the same `NativeEngine` ownership into Runtime and NativeAgent. The affected contract row remains `implemented`, never `verified`, until the same iOS exact HEAD completes required CI.

### 1.25 Exact-HEAD rebaseline: 2026-10-03 / `cf0012e6bb5ddcd1669c747994af68a5055bf05d`

Desktop PR #20 advanced two production commits from `9f22a1974f02683f4ad27a2fe856987b7dd1d20f` to `cf0012e6bb5ddcd1669c747994af68a5055bf05d`. The selected inventory remains exactly **7,927** blobs. Five `source-host` blobs changed: shipping `main.rs`, `host_runner_composition.rs`, `generated_agent_turn_stream.rs`, `production_turn_agent_owner.rs`, and the focused production wiring contract.

The new responsibility is behavioral, not merely structural. A group-member turn must still execute through the canonical generated-Agent lifecycle and terminal owner, while private per-Agent state is deliberately absent: no private checkpoint sink, no private turn-state surface, and no private browser/computer image-persistence callback. `HostRunnerComposition` is the sole composition owner that decides those omissions before the Runner is constructed. The shipping Host may supply group-member identity and request-specific hooks but may not create a parallel raw-provider path or private-state fallback.

The current iOS `NativeRunnerComposition` proves a single shared `NativeEngine` owner for Runtime and NativeAgent, but it has no group-member composition input and therefore cannot yet prove the required generated-lifecycle-without-private-state behavior. All five affected ledger rows are intentionally reset to `mapped`; prior `implemented` evidence remains historical and cannot satisfy this upstream identity. The iOS closure must be a native Host/Runner composition rule, not SwiftUI/Coordinator state and not a test-only facade. Same-iOS-HEAD GitHub Actions evidence is required before promotion.

### 1.26 Exact-HEAD rebaseline: 2026-10-03 / `29b10d13f1158c6b070ccabc1cf655317adcf184`

Desktop PR #20 advanced two commits from `cf0012e6bb5ddcd1669c747994af68a5055bf05d` to `29b10d13f1158c6b070ccabc1cf655317adcf184`. The selected inventory remains exactly **7,927** blobs. Three `source-host` blobs changed: shipping `main.rs`, `host_runner_composition.rs`, and the local-permission focused contract.

The new lifecycle rule is normative: after canonical Runner cancellation begins, Host shutdown must explicitly dispose the live local-tool permission projection subscriptions owned by `HostRunnerComposition`. This prevents stale permission surfaces from surviving shutdown/restart and keeps ask/projection ownership inside the Host/Runner composition rather than UI state. iOS must provide the same product effect with a native Host-owned shutdown settlement path; SwiftUI and the Coordinator may project permission state but cannot retain or clean up the canonical subscriptions themselves.

No affected row is promoted by rebaseline. The shipping and composition rows remain `mapped`; the previously unreviewed local-permission contract is now reviewed and mapped only. Exact-head production wiring plus focused behavior evidence is required before `implemented` or `verified`.

### 1.27 Exact-HEAD rebaseline: 2026-10-03 / `1e79bdad87022dcb394b9d4ecaef9508baf7497d`

Desktop PR #20 advanced one contract-only commit from `29b10d13f1158c6b070ccabc1cf655317adcf184` to `1e79bdad87022dcb394b9d4ecaef9508baf7497d`. The selected inventory remains exactly **7,927** blobs and only `source/host/tests/host_runner_composition_production_wiring_contract.rs` changed.

The strengthened contract makes two current responsibilities explicit without changing shipping Desktop source. First, `is_group_member_turn` must be supplied to the canonical `HostRunnerComposition.compose_turn_state_surfaces` path so group-member turns retain generated-Agent execution while omitting private Agent state. Second, shutdown ownership is ordered: the canonical Runner registry interrupts active turns first, then `HostRunnerComposition` disposes only the permission/projection surfaces it owns; composition must not duplicate Runner cancellation ownership.

The affected iOS row remains `mapped`. Existing single-NativeEngine ownership evidence is insufficient until the native mobile shipping path proves group-aware private-state omission and Rust Host-owned shutdown settlement with the same separation of responsibilities.

### 1.28 Exact-HEAD rebaseline: 2026-10-03 / `39e1eeaee2d861b18a5db5864dac4f7fc049530b`

Desktop PR #20 advanced one architecture-manifest-only commit from `1e79bdad87022dcb394b9d4ecaef9508baf7497d` to `39e1eeaee2d861b18a5db5864dac4f7fc049530b`. The selected `frontend/** + source/**` inventory remains exactly **7,927** blobs and every selected blob SHA is unchanged.

The Desktop manifest now declares HostRunnerComposition implemented after shipping-owner audit. Its normative responsibility set is explicit: one Host owner composes per-turn state/checkpoint surfaces, Runner construction and ordered decoration; group-member turns keep the generated-Agent lifecycle while omitting private state/checkpoint/memory/image/local-permission surfaces; TranscriptRunnerRegistry remains the active-run cancellation owner; HostRunnerComposition disposes only its own permission subscriptions after Runner cancellation.

That Desktop status is evidence about Desktop only. iOS does not inherit `implemented` or `verified`. The affected iOS rows remain `mapped` until the native mobile shipping path proves the same product effects and the exact iOS HEAD passes its own required CI/acceptance gates.

### 1.29 Exact-HEAD rebaseline: 2026-10-03 / `f138ffbcff2f6e541899a3f17dc95d25b06b1bcd`

Desktop PR #20 advanced two production commits from `39e1eeaee2d861b18a5db5864dac4f7fc049530b` to `f138ffbcff2f6e541899a3f17dc95d25b06b1bcd`. The selected inventory remains exactly **7,927** blobs. Three `source-host` blobs changed: `production_box_state.rs`, `mcp_state_executor.rs`, and its focused contract.

The normative change removes a duplicate MCP protobuf owner. `mcp_state_executor` now owns the canonical `agent.v1` MCP state argument/result wire contract, exact field/schema projection, provider grouping, tool definitions, and error/rejected normalization. `ProductionBoxMcpStateLoader` is an adapter over that port and may not maintain a second protobuf/tool schema. The focused contract proves semantic state survives the canonical wire round trip.

These rows were previously unreviewed. They are now reviewed and `mapped` only; no Desktop implementation status or old iOS evidence is inherited. iOS must audit its shipping MCP state owner and either reuse one Rust Host port or close any parallel schema before promotion.

### 1.30 Exact-HEAD rebaseline: 2026-10-03 / `ea29726d33da17bae8fbe5e1c145ba3c828081ee`

Desktop PR #20 advanced two production fixes from `f138ffbcff2f6e541899a3f17dc95d25b06b1bcd` to `ea29726d33da17bae8fbe5e1c145ba3c828081ee`. The selected inventory remains exactly **7,927** blobs. Three `source-host` blobs changed: `box_mcp_exec.rs`, `production_box_state.rs`, and `mcp_state_executor.rs`.

The canonical MCP state contract now preserves server `error_message` through encode/decode and both Box adapters project a non-empty value as `status_detail`. This is part of the same single-owner rule introduced at `f138ffbc…`: the Host MCP state executor owns the `agent.v1` state wire schema; Box execution/state layers consume the canonical result and may only adapt the visible projection.

The previously unreviewed `box_mcp_exec.rs` row is now reviewed and `mapped`. All three affected iOS rows remain `mapped`; no Desktop implementation status or historical iOS evidence is inherited.

### 1.31 Exact-HEAD rebaseline: 2026-10-03 / `42ac4967196e33e991e6c4d9e794a7c221ed0c63`

Desktop PR #20 advanced one architecture-manifest-only commit from `ea29726d33da17bae8fbe5e1c145ba3c828081ee` to `42ac4967196e33e991e6c4d9e794a7c221ed0c63`. The selected `frontend/** + source/**` inventory remains exactly **7,927** blobs and every selected blob SHA is unchanged.

The Desktop manifest now records the MCP state executor as implemented: one Rust Host owner controls provider grouping, tool metadata/schema, canonical state/result semantics, status and status-detail preservation, and the shipping Box adapters consume that owner rather than maintaining a parallel state decoder.

This changes Desktop acceptance status only. iOS does not inherit `implemented`. Its current production `RuntimeCommand::McpServers -> NativeAgentBackend::list_mcp_servers` path is the native state surface, but it currently projects only server name/plugin/status/runtime and drops the already-owned MCP tool schemas. The iOS closure must therefore enrich that existing Rust owner and keep `FeatureHostController::McpList` as a projection consumer; it must not introduce an unused Desktop-style protobuf layer merely for mechanism symmetry. If a remote Runner/Box wire is later required, serialization must adapt the same canonical iOS state owner.

The shipping iOS MCP state closure must also preserve the Desktop status/error semantics rather than only the tool schema:

- `NativeAgentBackend` remains the single canonical owner of native MCP server runtime state. SwiftUI, Coordinator, FeatureHost, and shared Swift MCP presentation may request or project that state, but they may not maintain a second startup/error registry.
- After a plugin resolves to an exact MCP server identifier, each open attempt receives a monotonically newer per-server generation and records `loading`. A completion may settle state only while that generation is still current, so an older concurrent attempt cannot overwrite a newer result.
- Successful tool discovery settles the same state to `connected`, clears stale detail, and stores the exact discovered tool definitions/schema. Tool-discovery join/transport/protocol failure settles it to `error` with the non-empty normalized failure message as `statusDetail`; the failed state remains listable even though no runnable MCP session was created.
- `AgentBackend::list_mcp_servers` projects this canonical state and must not manufacture `connected` merely because an MCP session exists. Account/session reset clears runnable MCP sessions and their canonical server-state truth together.
- Focused contracts must exercise the production state owner, including status-detail preservation and stale-generation rejection. A helper-only projection test is not sufficient for parity promotion.


### 1.32 Protected iOS CI account import contract

The protected Global Dharma acceptance must launch the real app with a bounded, refresh-token-free account session produced by `.github/scripts/prepare-ios-ci-session.mjs` and consumed by the Rust product/session owner. The preparation and Rust validation layers are one security contract: a credential accepted and emitted by preparation must not be rejected by a stricter, undocumented length rule at app startup. The account is still constrained by GitHub Actions provenance, `ciRunner=true`, Bearer token type, no refresh token, bounded lifetime, exact CI device/session identity, consistent top-level/nested user identity, maximum session size, and trusted-runner-only transport.

The current preparation contract accepts non-whitespace account access/refresh credentials from the real login response at **24..=16 KiB**. Rust CI-session validation must use the same minimum for the imported access credential; it must continue rejecting values below 24, whitespace/control bytes, oversized credentials, invalid provenance/identity, refresh tokens, and out-of-window lifetime. This alignment is not a relaxation of the protected journey: it removes a contradictory second validator that previously allowed preparation to succeed and then fatally rejected the same bounded session before `auth_status` or the UI could run.

Acceptance remains the unchanged real `GlobalDharmaJourneyUITests/testGlobalDharmaMarketplaceBotWebMcpCommerceJourney` plus the complete-state assertion. Unit/contract evidence must cover the exact 24-character lower boundary and rejection below it, and current-head protected-session evidence must show authenticated home restoration without browser interaction.

### 1.33 Exact-HEAD rebaseline: 2026-10-03 / `0e94c970c63eaadea8b1ea605a807a487a0519e2`

Desktop PR #20 advanced one focused-contract commit from `42ac4967196e33e991e6c4d9e794a7c221ed0c63` to `0e94c970c63eaadea8b1ea605a807a487a0519e2`. The selected inventory remains exactly **7,927** blobs. Only `source/host/tests/auto_review_gate_contract.rs` changed.

The strengthened contract makes Auto Review ownership explicit: shipping `main.rs` supplies live review dependencies into `worker_host_runner_composition.compose_production_turn(...)`, and `HostRunnerComposition` is the canonical owner that decorates routine Auto Review, Box shell review, and Subagent task review on the generated turn. A second direct Host builder path is not acceptable.

The iOS row was previously unreviewed. It is now reviewed and `mapped` only; no Desktop implementation/evidence is inherited. iOS must audit its Rust Host/Runner review composition and prove that SwiftUI/Coordinator do not own execution review decoration before promotion.

### 1.34 Exact-HEAD rebaseline: 2026-10-03 / `c43b246c8cd4b85359acba7850dd64d5a2fececc`

Desktop PR #20 advanced one production commit from `0e94c970c63eaadea8b1ea605a807a487a0519e2` to `c43b246c8cd4b85359acba7850dd64d5a2fececc`. The selected inventory remains exactly **7,927** blobs. Two `source-host` blobs changed: `runner/turn_agent_composition.rs` and `tests/runner_production_bridge_contract.rs`.

The new responsibility closes a production-boundary gap rather than changing the MCP state schema itself. `TurnAgentComposition::execute_mcp_state()` now adapts the exact `RoutedToolBridge` already owned by that generated turn into the canonical `mcp_state_executor`. This preserves first-seen provider grouping and exact tool metadata/schema without constructing a second MCP discovery owner. The focused production-bridge contract executes that shipping composition path. Desktop deliberately reset the manifest responsibility from `implemented` to `existing-needs-parity` until this new wiring receives its own exact-head Host/Runner and Electron evidence.

Accordingly, iOS does not inherit either the prior Desktop status or the prior iOS `implemented` status on the changed turn-composition row. The iOS native equivalent is the existing `RuntimeCommand::McpServers -> NativeAgentBackend::list_mcp_servers` path backed by the generation-safe `NativeMcpServerStateStore`; all native/per-turn consumers must use that one owner. Exact-head behavior evidence must prove this production consumer boundary, including loading/connected/error status, status detail, tool schema, stale-generation rejection, reset fencing, and absence of a parallel Swift/Runner discovery registry.

### 1.35 Exact-HEAD rebaseline: 2026-10-03 / `0b38b53a1a6b7cfe9957cf26732799cd657470a4`

Desktop PR #20 advanced two commits from `c43b246c8cd4b85359acba7850dd64d5a2fececc` to `0b38b53a1a6b7cfe9957cf26732799cd657470a4`. The selected inventory remains exactly **7,927** blobs. The only selected-source blob change is `source/host/tests/generated_subagent_production_cutover_contract.rs`.

The changed contract now proves more than child-session existence. Shipping Task launch must carry the real tool-call identity into Subagent review, a denied review must fence dispatch before the sink runs, and accepted review/sink dependencies must enter generated turns through canonical `HostRunnerComposition` / `TurnToolset`. The shipping Host remains the child Runner/session and live-parent projection owner, and child sessions do not recursively install Task.

For iOS this is `ios-adapted`. The existing `NativeEngine` already owns `subagent_run`, receives the actual model function-call id, and passes every tool through its Rust approval boundary before executing the SubagentScheduler. That overlap is not enough to inherit implementation status for the expanded Desktop responsibility. The row is reviewed to `mapped` until the iOS shipping path proves one child lifecycle owner, parent live projection, no recursive launch owner, and the same real-call identity review fence without a Swift/Coordinator parallel owner.

### 1.36 Exact-HEAD rebaseline: 2026-10-03 / `2d7320aacff322010238a2fca5431adce5c108a3`

Desktop PR #20 advanced one commit from `0b38b53a1a6b7cfe9957cf26732799cd657470a4` to `2d7320aacff322010238a2fca5431adce5c108a3`. The selected inventory remains exactly **7,927** blobs. The only selected-source blob change is `source/host/tests/journal_outcome_production_wiring_contract.rs`.

The changed contract makes checkpoint/transcript journal composition ownership explicit: shipping `app/main.rs` must delegate production checkpoint-sink construction to `HostRunnerComposition::compose_production_checkpoint_sink`; that composition owns the one-time connection to `ProductionTranscriptMirrorProvider::with_reporter`, while journal outcomes continue through the unique Host telemetry owner. Coordinator and renderer/Electron telemetry remain non-owners.

For iOS this is `ios-adapted`, not mechanism-equivalent. NativeEngine/session persistence and Rust telemetry are the platform-owned replacement for Desktop transcript-journal storage and Host structured logging, but the product responsibility still applies: one shipping runtime composition must own persistence outcome truth, one Rust telemetry owner must report it, and SwiftUI/Coordinator may only project that state. The changed ledger row is reviewed from `unreviewed` to `mapped`; existing durable session/checkpoint primitives are not enough to claim implementation until their production outcome/reporting path and unique ownership are demonstrated.

The iOS-adapted production contract for this slice is:

- `NativeEngine::persist_session_state_if_configured` remains the sole durable main-session checkpoint writer. It must emit a content-free checkpoint outcome through the engine-owned `RuntimeTelemetry` on both success and failure, including byte count and elapsed time where available; callers must not recreate that outcome in Swift or Coordinator code.
- persisted-session replay remains owned by `NativeEngine::open_session` / `restore_session`. A present snapshot records replay success or failure through the same `RuntimeTelemetry`; a missing snapshot or a snapshot older than the canonical transcript remains a normal non-replay path and must not be mislabeled as a failure.
- persistence telemetry contains no prompt, transcript, path, token, secret, or user content. It records only operation class (checkpoint/replay), outcome, bounded byte/time aggregates, and counters suitable for diagnostics.
- reporting must not change existing recovery behavior: unreadable/invalid persisted data may continue to fall back exactly where the current shipping path already falls back, while a restore failure that currently propagates remains propagating.
- focused contracts must exercise the real NativeEngine checkpoint/replay boundaries and the single RuntimeTelemetry owner; a helper-only counter test is insufficient for parity promotion.


### 1.37 Exact-HEAD rebaseline: 2026-10-03 / `4186cea169a71a0ebe38849f844dc153de204078`

Desktop PR #20 advanced one commit from `2d7320aacff322010238a2fca5431adce5c108a3` to `4186cea169a71a0ebe38849f844dc153de204078`. The selected inventory remains exactly **7,927** blobs. The only selected-source blob change is `source/host/tests/runner_communicate_listener_contract.rs`.

The strengthened contract binds the routine post-write integration path to the canonical `HostRunnerComposition`: after a routine writes to a target whose listener platform is not connected, shipping Host surfaces the frozen connect card/instruction and arms the lifecycle connection watcher, while the turn still consumes the same production composition hook. This is an ownership/lifecycle requirement, not merely a UI-string contract.

For iOS this is `ios-adapted`. `FeatureHostController` already owns native automations and listener summaries, but the current shipping path does not yet prove the Desktop product effect of post-write listener connect guidance plus Host-owned connection watching and automatic routine resume. The changed row is therefore reviewed to `mapped` only. The eventual iOS implementation must keep watcher/resume truth in the Rust Host/runtime owner and let SwiftUI/Coordinator project state rather than create a parallel listener lifecycle.

The iOS-adapted production contract for this slice is:

- creating or updating an enabled event-triggered automation whose listener platform is not connected must surface one transcript `listenerConnect` card from the canonical Rust Host owner and arm one deduplicated pending listener-resume identity keyed by Agent + platform;
- SwiftUI/Coordinator may render the card or initiate the platform-native connection flow, but may not own the pending-resume registry or decide when the automation setup turn resumes;
- when that exact listener becomes connected, the Host must consume the pending identity at most once, clear it before dispatch, and resume the owning Agent through the normal Rust Host/runtime chat path with hidden/system-style continuation semantics rather than firing the automation itself;
- disconnecting, closing the Host, deleting/disabling the automation, or deleting the owning Agent must not leave a stale resume that can wake a future unrelated turn;
- focused Rust contracts must exercise the production FeatureHostController command path and prove card emission, dedupe, one-shot resume, stale cleanup, and absence of a Swift-side registry.

## 2. Product goal

The goal is not to make an iOS app that separately reinterprets Grok Bot.

The goal is to make the **native iOS edition of the Fabushi product defined by Desktop PR #20**, preserving the same product capabilities, architecture ownership, runtime semantics, state machines, dependency direction, failure behavior, and durable lifecycle wherever they are applicable to iOS.

Desktop remains the upstream product/architecture implementation. iOS is a native platform port.

The iOS implementation may:

- directly reuse source from Desktop PR #20;
- port Rust source into iOS-owned Rust modules;
- translate TypeScript/React behavior into Swift/SwiftUI;
- adapt desktop platform mechanisms to Apple-native equivalents;
- split or merge implementation files when doing so does not collapse an architectural ownership boundary.

The iOS implementation must not:

- depend on a checkout of `fabushi-desktop` at build or runtime;
- depend on `fabushi-platform-core` or another shared Fabushi runtime repository merely to deduplicate Desktop/iOS code;
- move common Desktop/iOS production implementation into a new shared library as part of this migration;
- preserve Grok as a parallel direct iOS source of truth;
- claim parity from file names, stubs, ledger strings, or tests that do not exercise the shipping path.

Reused source becomes **iOS-owned source after migration**.

## 3. Standalone repository ownership

`bhrumom/fabushi-ios` owns the complete iOS product implementation required for a clean checkout to build, test, archive, install, and run:

- SwiftUI renderer;
- iOS platform-main lifecycle;
- trusted iOS bridge;
- Mahayana Coordinator;
- Host;
- Runner and safe local capability execution;
- remote/box execution adapters;
- iOS-local shared contracts/policies;
- iOS-local packages and runtime source;
- persistence/transcript/checkpoint state;
- auth/OAuth/passkeys;
- MCP/connectors/tools;
- messaging/communication capabilities;
- automations/workflows;
- inference/provider routing;
- telemetry/observability;
- StoreKit and Apple platform integration;
- Xcode/SPM/Cargo build integration;
- CI, signing, archive, TestFlight/App Store delivery.

A clean iOS checkout must not require another Fabushi source repository.

## 4. Architecture correspondence

Desktop PR #20 architecture is normative. iOS may substitute platform mechanisms, not ownership.

| Desktop PR #20 | Fabushi iOS |
| --- | --- |
| `frontend/**` | `frontend/**`, native Swift/SwiftUI projection |
| `source/electron-main/**` | `source/ios-main/**` |
| `source/electron-preload/**` | `source/ios-preload/**` |
| `source/electron-dev-controls/**` | `source/ios-dev-controls/**` |
| `source/node-agent-coordinator/**` | `source/mahayana-agent-coordinator/**` |
| `source/host/**` | `source/host/**` |
| `source/local-exec-daemon/**` | `source/local-exec-daemon/**` or safe iOS local-capability runtime |
| `source/box-exec-daemon/**` | `source/box-exec-daemon/**` |
| `source/internal/**` | `source/internal/**` |
| `source/packages/**` | `source/packages/**` |
| `source/shared/**` | `source/shared/**` |
| `source/mahayana/**` | iOS-owned Mahayana/Rust source under the iOS repository |
| `contracts/**` | iOS-owned versioned contracts where applicable |
| `scripts/**` | iOS-specific build/verification scripts |
| `.github/workflows/**` | iOS-specific exact-HEAD CI/release workflows |

The dependency direction is:

```
SwiftUI renderer
      |
      v
iOS trusted bridge
      |
      v
iOS platform-main
      |
      v
Mahayana Coordinator
      |
      v
Host
      |
      v
Runner / Provider / MCP / Capability adapters
```

Renderer/UI state is a projection. It is not the canonical owner of operation/run truth, retry state, transcript truth, provider lifecycle, connector truth, or recovery state.

## 5. Process-boundary adaptation on iOS

Desktop process boundaries are architectural evidence, but iOS must obey the iOS sandbox and lifecycle.

When Desktop uses an independent OS process and iOS cannot or should not do so, iOS may implement the same responsibility as a separate actor/module/runtime boundary.

This adaptation is valid only when it preserves:

- ownership;
- protocol boundaries;
- typed identity;
- cancellation;
- ordering;
- crash/restart or lifecycle settlement semantics;
- persistence ownership;
- failure normalization;
- observability lineage.

It is not valid to collapse Coordinator + Host + Runner + renderer state into one giant Swift object merely because iOS uses a single application process.

## 6. Source reuse policy

This migration intentionally permits source reuse **without a shared source repository**.

Allowed:

```
fabushi-desktop/source/host/x.rs
          |
          | copy / port / adapt
          v
fabushi-ios/source/host/x.rs
```

Allowed:

```
Desktop TypeScript behavior
          |
          | semantic translation
          v
iOS Swift implementation
```

Allowed:

```
Desktop Rust module
          |
          | iOS platform adaptation
          v
iOS-owned Rust module
```

Forbidden for this migration:

```
Desktop ---+
           +--> new/common shared runtime repository
iOS -------+
```

Source copied or ported from Desktop PR #20 must be reviewed for licensing/provenance requirements and then maintained by the iOS repository.

## 7. Desktop-to-iOS parity ledger

The old 2,046-row Grok-direct ledger is historical evidence only. Its `mapped`, `implemented`, `verified`, and N/A statuses are not completion evidence for this Spec.

A new Desktop PR #20 parity ledger is authoritative.

Every source-bearing Desktop file in the selected migration roots must be inventoried with at least:

- `desktop_path`;
- `desktop_blob_sha`;
- `desktop_responsibility`;
- `desktop_owner`;
- `desktop_visible_effect`;
- `ios_disposition`;
- `ios_target_path`;
- `ios_language`;
- `ios_platform_delta`;
- `implementation_status`;
- `production_evidence`;
- `test_evidence`;
- `notes`.

Allowed disposition classes:

- `direct-port`: source/responsibility can be substantially reused in iOS;
- `ios-adapted`: same responsibility/effect, implemented with an iOS-native mechanism;
- `not-applicable-with-replacement`: the desktop mechanism is prohibited/inapplicable, with an explicit replacement for any still-required product effect.

Allowed work statuses:

- `unreviewed`;
- `mapped`;
- `implemented`;
- `verified`;
- `not-applicable`.

Only `verified` and reviewed `not-applicable` may satisfy the final completion gate.

A status from the previous Grok-direct ledger may not be copied forward without re-reading the corresponding Desktop PR #20 implementation and validating the current iOS shipping path against it.

## 8. Inventory roots

The baseline inventory is generated from Desktop PR #20 exact HEAD and includes source-bearing files under:

- `frontend/**`;
- `source/**`.

Additional product-defining Desktop files under `contracts/**`, relevant `scripts/**`, and the authoritative Desktop Spec must be tracked separately when they define iOS-applicable contracts or behavior.

Generated build output, caches, vendored artifacts that do not define source responsibility, and platform packaging artifacts may be excluded only by an explicit inventory rule.

## 9. Platform substitutions

### 9.1 Electron main -> iOS main

Desktop Electron-native mechanisms map to Apple-native owners:

| Desktop mechanism | iOS mechanism |
| --- | --- |
| BrowserWindow/app lifecycle | SwiftUI scene/UIKit application lifecycle |
| desktop OAuth browser flow | AuthenticationServices / ASWebAuthenticationSession |
| desktop secrets | Keychain |
| desktop notifications | UserNotifications |
| desktop downloads | URLSession / background transfer |
| desktop file picker | UIDocumentPicker / PhotosPicker |
| desktop media devices | AVFoundation / Photos / system permission APIs |
| desktop deep links | URL/universal-link routing |
| desktop updates | App Store/TestFlight-compatible update semantics |
| tray/dock/window affordances | native iOS navigation/badge/notification equivalents where applicable |

### 9.2 Local execution

Arbitrary shell execution, unrestricted filesystem access, process spawning, and long-lived desktop daemons are not iOS requirements.

The product effect must be retained when applicable through one of:

- safe in-process iOS capability adapter;
- system framework;
- BGTaskScheduler/background transfer;
- remote Runner;
- box/remote-computer execution.

The ledger must distinguish “desktop mechanism N/A” from “product capability removed”. A product effect cannot be silently removed when an iOS-safe replacement exists.

## 10. Coordinator requirements

`source/mahayana-agent-coordinator/**` is the iOS counterpart of Desktop PR #20 `source/node-agent-coordinator/**`.

### 10.1 iOS carrier adaptation

Desktop `source/node-agent-coordinator/src/carrier.rs` owns more than the desktop process mechanism. Its portable contract includes validated coordinator bootstrap metadata, distinct `coordinator-control` / `coordinator-data` / `coordinator-main-data` channel identity, ordered buffering, fail-closed handling for unknown channels, and deterministic close semantics that reject new posts and discard queued work.

On iOS these responsibilities remain Coordinator-owned even though the transport is in-process. `source/mahayana-agent-coordinator/carrier.swift` is the canonical iOS carrier owner, and the shipping `InProcessCoordinatorPort` must route its frame delivery through that carrier rather than bypassing it. `IOSMainRuntime` supplies validated app-version/package/data-directory bootstrap metadata to `IOSCoordinatorLauncher`; SwiftUI/renderer code never owns or synthesizes carrier truth. The iOS transport may project the existing `CoordinatorPort` API onto the control channel while retaining typed data/main-data channels for Coordinator-internal routing.

### 10.2 Client-side tool v2 relay

Desktop `source/node-agent-coordinator/src/client_side_tool_v2_relay.rs` at blob `a01c69f3eb7aec0e07a8bcc003d251ad9d3d7c33` is the normative Coordinator responsibility for the `client-side-tool-v2` event family. The relay does not invent request IDs or run IDs. Its durable ordering/settlement identity is `(agentId, epoch, sequence, protobuf toolCallId)`: wire version must be `1`, account slot must be `host`, agent/epoch must be non-empty, sequence is strictly increasing within the current agent epoch, retired epochs are rejected, and protobuf/base64 payloads must carry the expected message type plus the matching tool-call-id field (`3` for Call and `35` for Result). A Result without a current Call is dropped. Reset clears current tool-call lifecycles, epoch changes retire the previous epoch, replay exposes only the current epoch's accepted Call/Result pairs in sequence order, and Coordinator shutdown clears relay state.

The iOS platform adaptation may receive Host events through the existing in-process Host/Coordinator boundary rather than Desktop stdio, but ownership may not move into SwiftUI. The shipping Coordinator must own the relay, route accepted events to the renderer event family, and replay accepted current-epoch state only from Coordinator-owned state. Host production of these events is a separate upstream responsibility: until the iOS Host turn-observation path can emit the same versioned `client-side-tool-v2` transport envelope, this Coordinator row may be implemented only for the Coordinator ingress/projection path and must not be represented as end-to-end verified.

### 10.3 Control-port client settlement

Desktop `source/node-agent-coordinator/src/control_port_client.rs` owns the Coordinator control-client handshake, monotonically allocated `c-N` request identity, pending request settlement, cancellation signaling, event posting, protocol-direction enforcement, and deterministic disconnect semantics. iOS may replace Desktop mpsc waiters with `@MainActor` Swift continuations over the in-process carrier, but it must preserve the same protocol effect: exactly one matching-version Ready transitions the client into service; repeated/mismatched Ready or any server-posted client-direction frame is a protocol breach; unknown/late replies are ignored; cancel is emitted only for a still-pending request; local shutdown posts Requested before closing; and no request/event is accepted once settled.

Every terminal control-port path must reject all pending calls with the Desktop `COORDINATOR_DISCONNECTED` failure class while retaining the causal message. Port close uses `control port closed`; local shutdown uses `shutdown requested`; peer shutdown uses its detail when present or the normalized `coordinator shutdown: <reason>` fallback; protocol breach uses the breach detail. Settlement clears pending continuations and closes the carrier exactly once. Generic `port-settled` errors must not erase this failure normalization.

It owns, as applicable:

- renderer port lifecycle;
- request/reply correlation;
- typed operation/run identity;
- ordered event fan-out;
- streaming activity;
- cancellation;
- reconnect/resync;
- transcript routing;
- inference routing;
- local/remote capability routing;
- client-side tool relay;
- MCP routing/OAuth forwarding;
- gateway routing;
- Host supervision;
- crash/lifecycle settlement;
- telemetry lineage.

SwiftUI Views and presentation Models must not reimplement these responsibilities.

## 11. Host / Runner requirements

The iOS Host/Runner implementation is ported from Desktop PR #20 responsibilities, not reconstructed independently from Grok.

It must preserve applicable Desktop behavior for:

- send acceptance and durable identity;
- transcript lifecycle;
- provider streaming;
- first-output/watchdog behavior;
- retry/backoff;
- checkpoint/resume;
- cancellation;
- tool/MCP execution;
- waiting-user states;
- terminal settlement;
- durable persistence;
- agent/group/workflow/automation lifecycle;
- connector state;
- communication/messaging lifecycle;
- failure and recovery semantics.

### 11.1 Send/media shaping platform adaptation

Desktop send_message_shaping.rs remains the normative responsibility boundary even though iOS does not reproduce the Desktop provider call shape literally. The iOS production path must remain FeatureHost ChatSend or AgentSend -> Host-owned selected-image materialization -> existing mahayana.conversation.send contract -> MahayanaRuntime and KernelConversationProvider -> NativeEngine current user turn -> single model boundary.

The canonical image owner is the iOS-owned Rust Host, not SwiftUI or the shared Swift image helper. source/host/selected-image-inputs.rs owns byte loading, MIME classification, and byte-level dimensions; Desktop pitm / ispe / irot behavior for ISO-BMFF HEIC/HEIF/AVIF dimensions must be preserved. FeatureHost may additionally honor an iOS-native picker's declared image MIME when the Desktop extension classifier does not claim that extension, but this is a platform input adaptation, not a second media owner.

mahayana.conversation.send carries an optional image-data channel while existing text-only callers serialize no extra media field. KernelConversationProvider forwards that channel as operation metadata; NativeEngine projects only data:image values into the same current user turn as input_image blocks. The model boundary owns wire-specific projection: Responses keeps input_image, Chat Completions emits image_url, and Anthropic Messages emits a base64 image source. Renderer-owned state must never become the media owner.

For direct Agent-to-Agent delivery, agent.send keeps image URL/alt metadata in durable AgentPeerMessage state. Only local file URLs are materialized into recipient selected-image input; unsupported or remote URLs are not falsely treated as local bytes. The recipient wake reuses schedule_background_agent_turn and mahayana.conversation.send. Group messaging keeps the Desktop product behavior and does not invent a separate image execution owner.

selected_image_inputs.rs and the direct generated-media consumer in agent_to_agent_messaging.rs may be implemented once this production wiring exists, but they remain unverified until exact-HEAD CI runs the production Rust Host graph and focused contracts. For ordinary ChatSend, the Rust FeatureHost must split attachments before Runtime dispatch into three Host-owned channels: images, selected videos, and generic files. Images continue through the typed selected-image data channel. Because the current iOS NativeEngine/model adapters do not expose a native provider video/file content block, selected-video descriptors (path, MIME, filename, fixed 4 fps sampling intent) and generic-file descriptors are consumed by the Host into distinct model-visible local attachment context before the Runtime boundary; images must not be duplicated into that generic context, and videos must not be downgraded to generic files. This is the iOS platform replacement for Desktop Runner argument shaping, not a second media owner. The broader send_message_shaping.rs row remains mapped until this three-way shipping split is wired and the remaining applicable reaction/reply/thread shaping semantics are audited.

### 11.2 Generated SendMessage pipeline rebaseline

Desktop PR #20 advanced from `dcb19a94383833fc1ec5074f10c4bbbd28c09036` to `f1d06ed8ad2a1ce4b5adeeed6df8e97152553661`. Unlike the preceding manifest-only acceptance, this advance changes shipping Host source: `source/host/app/src/main.rs`, `source/host/src/extensions/transcript/production_runtime.rs`, and `source/host/src/extensions/transcript/send_pipeline.rs`, with focused production-composition coverage. Runner-generated `SendMessage` is no longer allowed to append an ad-hoc `runner-send:<tool_call_id>` transcript object directly. It must enter the canonical Host send pipeline, derive a live transcript entry ID, validate explicit reply targets, apply turn reply-context threading, reuse the session attachment batch identity, stamp fork-generated entries as branched, durably append through the canonical Session owner, and then continue through the existing media persistence/delivery/ack/activity owner.

For iOS this is an applicable product responsibility, not a desktop-only mechanism. The in-process iOS Host may replace Desktop process mechanics, but `AgentSend` or any Runner-generated send must not bypass canonical transcript shaping/persistence or create a parallel sender. NativeEngine `send_message` tool output is a generated send, not an ordinary model completion: the tool must return its generated payload to the Kernel, while the canonical KernelConversationProvider/Runtime transcript owner allocates the visible MessageId, persists exactly one assistant entry, and emits the user-visible completion. The tool implementation must not append or simulate a second ordinary completion itself. Generated-send metadata must retain the originating tool-call identity so future reply/thread, attachment-batch, and fork semantics can extend the same owner instead of creating another sender. The affected iOS row remains `mapped` until reply/thread validation, attachment batch identity, fork stamping, and their downstream settlement semantics are applicable and proven with exact-HEAD tests. Existing generated-media materialization does not by itself satisfy this broader pipeline responsibility.

The iOS adaptation for this responsibility is now fixed as follows: `FeatureCommand::ChatSend` may carry `replyToMessageId` plus `isFork`; the canonical Runtime request transports those fields without UI-owned interpretation. `KernelConversationProvider` validates an explicit generated `send_message.reply_to_message_id` only against a live `MessageId` in the same conversation, rejects stale/cross-conversation/self targets, and otherwise falls back to the validated turn reply target. Fork stamping is valid only when the turn reply target is live. Generated attachments use the same per-turn attachment batch identity for every generated attachment in that admitted turn. The canonical persisted `Message.metadata` carries the shaped relation/attachment fields, `FeatureHostController` projects them through the existing `chat.message` event, and SwiftUI renders/uses that projection but never owns the validation or batch/fork truth. Attachment-only generated sends are allowed; text-only generated sends must not receive an attachment batch stamp. This implementation remains `implemented`, not `verified`, until the accepted exact iOS HEAD passes Rust, Swift/UI, architecture, device archive, and protected-session evidence.

### 11.3 Direct-turn context and workflow-reference rebaseline

Desktop PR #20 advanced from `f1d06ed8ad2a1ce4b5adeeed6df8e97152553661` to `bf36916c80e68737f02217ae85be4d22a6a5f928` with source-bearing changes in the shipping Host direct-turn path. `send_turn_dispatch.rs` now shapes direct Runner arguments only after the user turn has been durably admitted: it carries the persisted user `messageId`, projects durable recent user messages (including rich text), expands enabled workflow references, prepends an offline-composed timestamp note when applicable, and injects mentioned-Agent context derived from the live roster. `main.rs` invokes this shaping before the canonical `sendPrompt` Host lane dispatch. `workflow_commands.rs` and `main.rs` also route a visible workflow-reference run-now back through the canonical sendPrompt path instead of a compatibility dispatch, preserving ordinary user-turn history/context ownership.

These are applicable iOS product effects even though iOS may use an in-process Host/Runner boundary. The iOS implementation must preserve one canonical user-turn admission/dispatch owner, durable message identity and recent-history context, workflow-reference expansion, mentioned-Agent context, and offline-composed semantics before Runner execution. No status is inherited from Desktop. The affected rows remain unreviewed until the iOS shipping owners are audited against this exact responsibility; this upstream change does not invalidate the already-audited selected-image/media ownership unless that audit finds an ownership conflict.

### 11.4 Direct user-turn supersession rebaseline

Desktop PR #20 advanced from `bf36916c80e68737f02217ae85be4d22a6a5f928` to `3ad5c76a67408357cfe5647c6d073b36015c8eb5`. The shipping Host now treats a newly durably admitted direct local user turn as a supersession boundary: it cancels the current routed one-to-one Runner task for the same Agent with a stable `superseded by a new user message` reason, also preempts an applicable group-member run, records an acknowledgement interruption when an active run was actually interrupted, and emits turn-interrupt telemetry. `runner_registry.rs` resolves only the current routed stream for the Agent, so a stale/completed stream is not accidentally cancelled.

This behavior is applicable on iOS regardless of process topology. The iOS Host/Runner boundary must preserve current-run identity, targeted same-Agent cancellation, stale-run fencing, cancellation reason, acknowledgement settlement, and observable interruption lineage. A superseded run must settle as an interruption, not a provider failure: the stable supersession reason belongs to the canonical Runtime/Host operation owner, must not create an error tray, and stale/duplicate completion must not erase the replacement operation. Explicit user interruption remains a distinct reason. The changed Runner-registry responsibility remains incomplete until the iOS shipping task registry and direct user-send path are audited; no Desktop status is inherited.

### 11.5 Workflow-reference trace-context rebaseline

Desktop PR #20 advanced from `3ad5c76a67408357cfe5647c6d073b36015c8eb5` to `84dbe458a8f14307bbdeaff469388734e6ce8879` with a focused shipping correction: a visible workflow-reference run-now still re-enters the canonical `sendPrompt` path, but it does so without carrying a synthetic gateway trace context. This preserves the ordinary user-turn execution/telemetry boundary rather than manufacturing a gateway parent span for a locally synthesized workflow-reference turn.

The iOS workflow-reference responsibility remains unreviewed until its shipping owner is audited. No media or direct-turn supersession status changes are inherited from this upstream change.


### 11.6 Await-turn terminal settlement and pre-dispatch supersession rebaseline

Desktop PR #20 advanced from `84dbe458a8f14307bbdeaff469388734e6ce8879` to `3e735e6e5b7253713815ee1d034bd8ec446fb5a7` in two commits. The selected `frontend/**` + `source/**` inventory remains 7,925 files, but seven source-bearing blobs changed and the Desktop architecture manifest changed with them. Every affected ledger row is invalidated until revalidated against the iOS shipping path.

The eight changed Desktop files and their normative responsibility deltas are:

- `source/node-agent-coordinator/src/inference_router.rs`: Coordinator owns explicit `awaitTurn` parsing. Only boolean `true` requests terminal settlement, and workflow-reference run-now now sends `awaitTurn=true` plus `source=workflow-reference`.
- `source/node-agent-coordinator/src/main.rs`: request acceptance and turn completion are distinct. Ordinary sends settle the renderer request after queue admission; `awaitTurn` preserves that same request identity until `execute_local_inference` reaches terminal success or normalized failure/cancel.
- `source/node-agent-coordinator/tests/inference_host_boundary_contract.rs`: focused contract proves the workflow-reference metadata and terminal-request semantics.
- `source/host/src/extensions/transcript/runner_registry.rs`: Host canonical Runner registry now records `dispatched` and `recovery_shaped` per routed stream, and removes that state on finish.
- `source/host/src/runner/turn_run_shell.rs`: the active run records dispatch/recovery shape. Before actual dispatch, supersession is allowed only when the superseding turn carries recovery and the active turn is recovery-shaped; after dispatch normal targeted cancellation applies.
- `source/host/app/src/main.rs`: shipping composition wires recovery-shape registration and dispatch marking into the real routed turn path; this is not helper-only parity.
- `source/host/tests/runner_routed_provider_contract.rs`: focused Host contract proves the pre-dispatch/recovery-shaped fencing and dispatched-state behavior.
- `projects/grok-fabu-parity/architecture-manifest.json`: Desktop's own parity manifest promotes the corresponding send-turn-dispatch responsibility; this file is outside the iOS selected source inventory but is part of the audit evidence.

iOS disposition at this baseline:

- `MahayanaCoordinator.request/dispatchTransport` now implements the explicit terminal-settlement half of this delta: only literal `awaitTurn=true` keeps the same renderer request pending, and workflow-reference execution enters that canonical contract instead of synthesizing a parallel completion path.
- The iOS Runtime/Host implementation for this responsibility lives at the canonical direct-conversation owner, not in SwiftUI. `RuntimeCommand::SendMessage` carries the provider input separately from optional durable `displayText`, and carries an explicit `recoveryEligible` bit from the Host send-shape owner. `KernelConversationProvider` derives incoming `carries_recovery` from visible durable `clientMessageId` identity, while the registered active operation is `recovery_shaped` only when that durable identity is also Host-approved as recoverable and carries no selected image payload. The direct-operation registry stores exact operation identity together with `dispatched`, `recovery_shaped`, and pre-dispatch cancellation state.
- A newly durably admitted direct turn never blindly cancels an undispatched predecessor. If the predecessor is still pre-dispatch, cancellation is permitted only when both the incoming turn carries recovery and the predecessor is recovery-shaped; otherwise the incoming operation waits at the same Runtime admission boundary until the predecessor crosses the dispatch fence, then performs ordinary targeted interruption by exact operation id. Once a predecessor is dispatched, ordinary direct-user supersession remains allowed. A recovery-shaped predecessor cancelled before backend dispatch settles locally as interrupted and never calls the provider backend. Compare-and-clear on exact operation identity prevents the stale predecessor from deleting a replacement operation.
- Durable visible text and provider input are deliberately separate. Memory/MCP/workflow context may expand the provider input, but recovery classification and durable user history are anchored to the visible user turn. Attachments or other non-durable active-turn context fail closed by clearing `recoveryEligible` for that run's own recovery shape; they do not erase the incoming turn's durable-message recovery carry. Hidden/background sends never participate in direct-user supersession.
- The Coordinator terminal-settlement half is now implemented in the iOS shipping path: only literal `awaitTurn=true` on `feature.execute` holds the Coordinator request; `feature.execute` atomically registers the accepted `operationId`; Host-owned `feature.awaitOperation` advances the canonical Runtime event pump in bounded steps, remembers terminal completion/interruption/failure only for registered waiters, and re-enqueues translated Host events so the terminal waiter cannot become a second renderer event owner. Coordinator cancellation removes the waiter registration without inventing a terminal operation state.
- The Coordinator terminal-settlement and Runtime pre-dispatch supersession implementations are not `verified` until their accepted iOS exact HEAD passes focused Rust contracts plus Rust/Swift/architecture/lifecycle acceptance. Focused direct-operation contracts must prove both-sided recovery gating, dispatch transition, local pre-dispatch cancellation, durable visible-text/provider-input separation, and stale-settlement fencing.
- Existing `send_pipeline` reply-target/thread/generated-attachment/fork gaps remain valid candidate work only after the remaining pre-dispatch supersession delta is handled; no old `84dbe458` acceptance evidence proves parity with `3e735e6e`.


### 11.7 Public recovery-shape helper export rebaseline

Desktop PR #20 advanced from `3e735e6e5b7253713815ee1d034bd8ec446fb5a7` to `a8cc75d1917ae8aa8c81d241f17cba57589bb4db` in one commit. The selected inventory remains 7,925 files and only two selected source blobs changed: `source/host/app/src/main.rs` and `source/host/src/runner/mod.rs`.

The change does not alter the recovery/supersession state machine introduced at `3e735e6e`. `runner/mod.rs` now publicly re-exports `is_recovery_shaped_turn`, while `host/app/src/main.rs` imports that helper through the public `runner` surface instead of the private `runner::turn_run_shell` path. Shipping composition still registers recovery shape before dispatch and marks the same routed stream dispatched; request/run identity, terminal settlement, targeted cancellation, recovery classification, and failure semantics are unchanged.

For iOS this export shape is not itself a required mechanism because the iOS Host/Runner boundary is iOS-owned and in-process. The applicable product responsibility remains the pre-dispatch `dispatched + recovery_shaped` supersession fence identified in section 11.6. The two changed Desktop rows are revalidated against `a8cc75d1`; no previous exact-HEAD CI is promoted to current parity evidence.



### 11.8 Deferred windowed Session activation rebaseline

Desktop PR #20 advanced from `a8cc75d1917ae8aa8c81d241f17cba57589bb4db` to `cbed42883dec4dbd12af2d54bd60d5855b3c0327` in two commits. Four selected Host blobs changed: `source/host/app/src/main.rs`, `source/host/src/extensions/transcript/roster_emit.rs`, `source/host/src/extensions/transcript/session_runtime.rs`, and `source/host/tests/transcript_session_runtime_contract.rs`.

The normative product responsibility added by this delta is Session activation after a bounded/windowed transcript read:

- the bounded response settles before a cold Session becomes the canonical active Agent;
- SessionRuntime assigns a monotonically supersedable activation generation and retains the target Agent plus the last transcript entry already shipped in the bounded response;
- only the latest matching generation/Agent may claim activation; explicit Agent switch and Agent deletion invalidate pending activation;
- after the claim, Host switches the canonical active Agent and emits only transcript entries strictly after the retained `shippedThroughId`; if that anchor is missing, it does not guess a catch-up range;
- roster projection is refreshed for the newly active Agent and the previously active Agent when they differ;
- ordinary gateway contact refreshes focus freshness only while the desktop window is focused, preserving the existing focus/staleness state machine.

iOS must preserve this responsibility but need not reproduce a desktop window or background thread. The iOS-native replacement should keep a generation-fenced pending conversation/Agent activation owner at the canonical session/runtime boundary, settle any bounded snapshot first, then apply the latest activation and delta catch-up through structured concurrency. iOS scene activity replaces desktop focus freshness where that product effect applies. The current iOS UI's local `selectedConversation` state and `markRead` call are only presentation state and are not accepted as a replacement canonical Session activation owner.

The iOS production disposition is now implemented in the canonical Rust Host path, but remains pending exact-HEAD verification:

- `mahayana-host-protocol` exposes typed `conversation.openWindowed` / `conversation.openTail` commands plus canonical window, append, activation, and activation-failure events. SwiftUI does not own activation truth.
- `FeatureHostController` owns `ConversationSessionState`: active conversation identity, pending activation generation, shipped-through anchor, explicit-switch/delete/session-reset invalidation, active-only contact freshness, and fail-closed catch-up.
- bounded/tail commands first enqueue the bounded window projection. Only after queued response events are drained does the Host event pump claim the latest pending generation, switch the canonical active conversation, emit entries strictly after the shipped anchor, emit the active-conversation projection, and refresh the canonical conversation roster. A newer bounded request replaces the prior pending claim.
- iOS does not create a desktop-style background thread for activation. The trusted Host event pump is the deferred task boundary, so there is no thread-spawn failure mode. Runtime/history failure after a claim is surfaced as `conversation.activationFailed` carrying both conversation identity and generation rather than silently switching or guessing.
- `IOSMainRuntime` forwards native scene active/inactive/background state through Coordinator → AppHost `feature.sessionActivity`; the freshness timestamp itself remains Rust Host-owned. Ordinary Host feature traffic only refreshes freshness while that canonical scene-active bit is true.
- the ordinary Runtime `ConversationHistory` command keeps its existing 500-message clamp and 200-message explicit-open read-acknowledgement contract. A separate canonical `ConversationHistoryWindow` path now delegates `beforeMessageId` / `afterMessageId` slicing to the owning `ConversationProvider`; the Kernel provider slices its full current canonical transcript, so bounded paging and shipped-through catch-up do not depend on the ordinary-history clamp and do not accidentally mark unread state as read. Missing anchors still fail closed.
- focused Rust contracts cover latest-generation supersession, explicit-switch invalidation, strict-after-anchor catch-up, missing-anchor fail-closed behavior, active-only freshness, stable command/event wire shapes, and provider-owned before/after window boundaries. They are production-focused implementation evidence only until the new iOS exact HEAD passes GitHub Actions.

Accordingly the four changed Host rows are `implemented`, not `verified`. Their known production semantics are now dispositioned; exact-HEAD CI remains required before promotion. Once that exact-head evidence closes, section 11.6's pre-dispatch `dispatched + recovery_shaped` supersession fence is again the earliest production gap. Existing supersession implementation work remains migration material because its Desktop source semantics are unchanged by this delta.


### 11.9 Transcript delegated-owner rebaseline

Desktop PR #20 advanced from `cbed42883dec4dbd12af2d54bd60d5855b3c0327` to `e891086964e08a5747a056d790dfd545b12a1a43` in two commits. The selected inventory remains 7,925 files. Five selected Host blobs changed: `source/host/app/src/main.rs`, `source/host/src/extensions/transcript/session_runtime.rs`, `source/host/src/extensions/transcript/transcript_manager.rs`, `source/host/tests/agent_open_production_wiring_contract.rs`, and `source/host/tests/transcript_manager_contract.rs`. Desktop's `projects/grok-fabu-parity/architecture-manifest.json` also changed but remains outside the selected iOS source inventory.

The first commit (`a5386793`) closes Desktop's own SessionRuntime parity status and aligns comments/focused contract wording with the bounded/deferred activation implementation already introduced at `cbed4288`; it does not introduce a new Session activation state machine. The second commit (`e8910869`) adds a real ownership rule: `TranscriptManager` becomes the single production composition owner for delegated transcript-adjacent services such as group chat, widget/permission responses, and shared rooms, and `main.rs` must reuse those manager-owned instances rather than constructing parallel owners around the same session-worker graph.

iOS preserves the ownership effect without reproducing Desktop's type graph. The native shipping composition has one `AppHost`-owned `FeatureHostController`. The same controller owns `FeatureState` group/group-run/group-operation state and pending approvals, and owns one `MahayanaHost` Runtime used by group member turns and approval resolution. AppHost does not construct an alternate group/widget/session owner. Existing group behavior contracts exercise continuity through this canonical controller. Therefore the changed `transcript_manager.rs` and manager-contract responsibilities are `ios-adapted` / `implemented`, while the existing deferred Session activation rows remain implemented after revalidation against the new upstream blobs.

No CI or acceptance result tied to `cbed4288` is promoted to current parity evidence. PR #26 may currently be based on `e8910869`, but it remains observation-only until merged into PR #20 and never becomes a second formal iOS upstream. Current exact-head GitHub Actions are still required before any affected row can become `verified`.


### 11.10 Transcript delegate routing closure rebaseline

Desktop PR #20 advanced from `e891086964e08a5747a056d790dfd545b12a1a43` to `f55175300404b20c6c50dd815601ef89ec233c49` in one source-bearing commit. The selected inventory remains 7,925 files. Two selected Host blobs changed: `source/host/app/src/main.rs` and `source/host/tests/send_group_fanout_contract.rs`.

This delta does not add a new group, shared-room, or widget-response product state machine. It closes the remaining production composition bypasses: routed Runner dependencies now carry the canonical `TranscriptManager`; local/agent-posted group turns use its stable group-chat delegate; local Agent messages to shared rooms use its stable shared-room delegate; channel inbound/failure revival reads widget-response context through its stable widget delegate; and shared-room/group fanout focused contracts explicitly reject fresh `GroupChatGlue::new(...)` construction.

The iOS adaptation preserves the ownership effect without copying Desktop's Arc graph. `AppHost` owns one `FeatureHostController`, and that controller owns one `FeatureState` plus one production Runtime. Session activation, group lifecycle/run/operation state, approval/event state and Runtime dispatch therefore share one Host composition boundary. A focused behavioral contract must prove cross-flow state continuity on the same controller. This is an ownership contract only: the unchanged Desktop `shared_rooms.rs` and `widget_responses.rs` rows retain their independent review status and are not implicitly promoted by this rebaseline.

The changed `main.rs`, `transcript_manager.rs` ownership responsibility and `send_group_fanout_contract.rs` may remain `implemented` on iOS after that focused contract lands, but none are `verified` until the same accepted iOS exact HEAD passes architecture, Rust Host, Swift Unit/UI, device archive and protected-session acceptance. After this upstream delta is dispositioned, the previously identified pre-dispatch `dispatched + recovery_shaped` supersession fence remains the next production implementation gap.



### 11.11 Transcript lifecycle ownership rebaseline

Desktop PR #20 advanced from `f55175300404b20c6c50dd815601ef89ec233c49` to `6ac2d23df5cdb45004d5c73c771f78de334d6b02` in one source-bearing lifecycle commit. The selected inventory remains 7,925 files; twelve selected Host blobs changed. This delta makes `TranscriptManager` responsible not only for constructing delegated transcript services but also for settling their process-local lifecycle before Session storage closes: deferred activation and runtime observers are invalidated, handoff state is cleared, ack redrive and automation wake/reporting state are disposed, workflow watchers are detached, turn-dispatch/scheduler state is disposed, Runner work is cancelled, and Session owners close with checkpoint semantics. `TranscriptExtension` is explicitly dropped before the Session extension.

The iOS platform adaptation remains the single `AppHost -> FeatureHostController -> MahayanaHost/Runtime` composition rather than cloning Desktop's `TranscriptManager` type graph. Applicable product effects are mandatory. Host close/drop must invalidate deferred conversation activation and generation, settle in-flight operation identities before releasing Runtime ownership, clear process-local approval/await/background/group-operation/session state without deleting durable automations/workflows/transcript history, terminate any active teach capture, and leave no presentation-owned lifecycle truth. iOS workflow access is request-scoped rather than a long-lived file watcher, so Desktop watcher detachment maps to the absence of a second observer owner, not to a new watcher. Durable product state continues to be written by its existing canonical owners; shutdown may not erase it merely to satisfy lifecycle tests.

Affected rows are re-bound to the new blob identities but no prior exact-HEAD verification is inherited. This migration slice reviews all twelve changed Host responsibilities: process-local lifecycle duties map to the canonical `FeatureHostController::close`/Drop path; Desktop-only checkpoint/watcher/redrive/scheduler mechanisms use documented iOS replacements where the product effect is preserved without a second owner. The affected rows are at most `implemented` until exact-HEAD CI proves the production build and focused contracts. The pre-dispatch `dispatched + recovery_shaped` fence remains part of the same iOS Host/Runtime line but requires this new exact-HEAD CI as well. PR #26 is currently rebased onto this PR #20 baseline, but remains observation-only until its work is merged into PR #20; it does not become a second formal iOS upstream.


### 11.12 Durable ack-redrive contract rebaseline

Desktop PR #20 advanced from `6ac2d23df5cdb45004d5c73c771f78de334d6b02` to `5523b564ff16ee8be6f00fce64c4162a6c07b35d` in one focused-contract-only commit. The selected inventory remains 7,925 files and only `source/host/tests/transcript_manager_contract.rs` changed. The production lifecycle implementation is unchanged. The strengthened contract now records a real durable acknowledgement obligation before arming the redrive timer, so TranscriptManager disposal is proven against non-empty durable redrive state rather than an empty scheduler.

This does not create a new iOS transcript-ack owner. The iOS disposition from 11.11 remains: there is no parallel Transcript AckObligations/redrive scheduler; provider/service acknowledgement durability stays with its canonical provider owner, while native Host lifecycle settlement must not create or leave a second redrive callback. The changed Desktop test row is rebound to the new blob and cannot inherit exact-HEAD test verification from `6ac2d23...`; current iOS production and focused-contract evidence still require same-HEAD GitHub Actions before promotion to `verified`.


### 11.13 Client-side-tool-v2 producer ownership rebaseline

Desktop PR #20 advanced from `5523b564ff16ee8be6f00fce64c4162a6c07b35d` to `69401d853cc251666a8d5a65e93d9a18bad27c8d` in one source-bearing ownership commit. The selected inventory remains 7,925 files and three Host blobs changed: `source/host/app/src/main.rs`, `source/host/src/extensions/transcript/transcript_manager.rs`, and `source/host/tests/transcript_manager_contract.rs`. The product change is not cosmetic: the stateful client-side-tool-v2 producer (per-Agent epoch, strictly monotonic sequence, open Call/Result settlement, reset) is no longer permitted as an independent routed-provider producer. `TranscriptManager` now constructs and owns that producer, routed provider observation publishes through the manager, and manager disposal resets the producer together with the rest of process-local transcript state.

iOS already has the downstream Coordinator relay and renderer projection, with `(agentId, epoch, sequence, protobuf toolCallId)` fencing, replay, reset, and shutdown clearing. The remaining applicable Host responsibility is upstream production of the versioned transport envelopes from the canonical Runtime tool-observation path. That producer must be owned by the existing canonical iOS Host/Runtime composition (the single `FeatureHostController`/Runtime line), not SwiftUI and not a second global/static producer. Call and Result must carry the real tool-call identity, Result without a matching open Call must fail closed, sequence must remain monotonic per Agent within one producer epoch, and Host lifecycle settlement must clear/reset producer state.

The three changed Desktop rows are invalidated from prior exact-HEAD evidence until this ownership is wired. The adjacent Desktop `client_side_tool_v2_{inventory,producer,projection}` responsibilities and focused contracts must be reviewed as one behavior slice because the current iOS ledger already records the Coordinator relay as implemented but explicitly blocks end-to-end verification on the missing mobile Rust Host producer. PR #26 is observation-only even when rebased; it is not an iOS migration authority until merged into PR #20.

### 11.14 FBCP/TDRP governance-only exact-HEAD rebaseline

Desktop PR #20 advanced from `69401d853cc251666a8d5a65e93d9a18bad27c8d` to `885f9e0c5ead351bd6b18b52a781633a8862df04` in 13 commits. No selected `frontend/**` or `source/**` file changed, so the 7,925 source-bearing paths and their blob identities are unchanged. The delta adds/updates repository governance and active specifications for FBCP-001 and TDRP-001, including `docs/specs/fabushi-bot-communication-platform.md`, `docs/specs/telegram-desktop-rust-equivalence-migration.md`, project source-of-truth/status files, and root AI instructions.

These documents are product-direction authority but explicitly do **not** claim implemented communication capability at this HEAD: FBCP marks implementation as not implemented/not accepted, and its status still blocks native messaging infrastructure, Human messaging in the existing workspace, Human+Agent unified flow, full feature absorption, packaged acceptance, and release. TDRP likewise remains research/owner-resolution work. Therefore this rebaseline does not import PR #26 production code and does not create a second iOS upstream. iOS records the direction: Telegram remains research-only; current PR #20 owners remain the target; future Human/groups/channels/replies/media/calls/etc. enter iOS only after production implementation lands in PR #20 and the exact HEAD changes again.

All Desktop-bound authority tokens/sourceCommit values are nevertheless rebound to `885f9e0c5ead351bd6b18b52a781633a8862df04` as required. Prior exact-HEAD CI evidence tied to `69401d853cc251666a8d5a65e93d9a18bad27c8d` cannot prove current parity, while unchanged source responsibility judgments may be re-used only after this explicit revalidation. The manager-owned client-side-tool-v2 Host producer implementation remains applicable because none of its Desktop source files changed in this docs-only advance; it still requires current iOS exact-HEAD CI before any row can become `verified`.

### 11.15 Transcript-owned widget-response rebaseline

Desktop PR #20 advanced from `885f9e0c5ead351bd6b18b52a781633a8862df04` to `0941e6b189738d0bfaed9b64c6a186d17e6d33ec` with two source-bearing commits and nine affected Host paths, including one newly selected source row: `source/host/tests/widget_responses_full_contract.rs`. The authoritative selected-source inventory therefore increases from 7,925 to 7,926 rows and the `source-host` group from 955 to 956 rows. Older exact-HEAD acceptance is historical only.

The product delta is not a helper-only change. Desktop now makes `TranscriptManager` the shipping composition owner for `WidgetResponses`, sharing the existing Session/Transcript runtime plus Automation owner and binding exactly once to roster projection, AutoReview, and channel-config signaling. `respondToWidget`, `dismissWidget`, `submitSecret`, and `reactToMessage` enter through that manager-owned lane. Durable transcript mutation, rollback on failed widget send, dismiss-on-move-on branch-aware fencing, stale AutoReview expiry, secret persistence plus hidden resume, reaction toggle/resume semantics, and active-vs-background transcript/roster projection are therefore one canonical responsibility. `AutoReviewService`/`SandAutoReviewController` expose targeted pending-approval expiry to that owner rather than creating a second review state machine.

For iOS this product effect is applicable. The platform replacement may use the existing in-process `FeatureHostController`/Runtime conversation owner instead of Desktop Session workers, but SwiftUI must not own widget/reaction/secret transcript truth. The iOS Host must expose typed actions, resolve them against canonical durable conversation/message identity, mutate exactly once, project the updated canonical entry through the existing Host event lane, and resume the Agent only through the existing Runtime/operation owner when required. Existing `SecretProvide` or communication reaction helpers do not by themselves satisfy this transcript-widget responsibility. The five previously implemented Desktop rows whose blobs changed in this delta are reset to `mapped`; the newly affected WidgetResponses/AutoReview rows remain `unreviewed` until production wiring is ported.

## 12. Frontend requirements

Desktop PR #20 `frontend/**` is the product/UI behavior source. iOS implements it natively in SwiftUI.

The goal is not pixel-identical desktop geometry. The goal is the same information architecture, product capabilities, state semantics, control meaning, and observable lifecycle, adapted to iPhone/iPad.

iOS presentation must consume canonical runtime projections and emit typed intents. It must not infer canonical run state from local booleans such as “busy” when runtime state exists.

## 13. Existing PR #3 implementation

The current PR #3 implementation is retained as migration material, not accepted wholesale.

Existing code must be classified against the pinned Desktop source:

- matches current Desktop responsibility -> may be promoted after evidence;
- derived from Grok but Desktop changed the responsibility -> stale, must be changed;
- implements a Desktop-removed/unauthorized feature -> remove;
- implements an iOS-native substitute for a Desktop responsibility -> retain after mapping/evidence;
- bypasses Desktop ownership boundaries -> refactor/remove.

No previous Grok-ledger count is a completion metric under this Spec.

## 14. Rebaseline protocol

Before any substantial new migration slice:

1. read Desktop PR #20 current exact HEAD;
2. compare it with the pinned SHA in this Spec and reference manifest;
3. if unchanged, continue against the existing baseline;
4. if changed:
   - record the new exact HEAD;
   - regenerate Desktop source inventory/blob identities;
   - diff added/removed/changed upstream paths;
   - invalidate stale row/evidence claims affected by the diff;
   - update this Spec/reference metadata;
   - only then continue implementation.

A green workflow for an older Desktop baseline does not prove parity with a newer PR #20 HEAD.

## 15. Implementation phases

### Phase 0 — Authority cutover

- replace Grok-direct authority with Desktop PR #20;
- pin Desktop exact HEAD;
- supersede the old Grok-direct Spec as an active authority;
- generate Desktop source manifest;
- create Desktop->iOS ledger with all rows initially unreviewed unless revalidated;
- change architecture CI to validate the Desktop-based baseline.

Exit: no active completion gate claims Grok-direct row counts as iOS parity.

### Phase 1 — Re-audit existing iOS architecture

Audit current `frontend/**`, `source/ios-main/**`, `source/ios-preload/**`, Coordinator, Host, Runner, shared/packages, and mobile bootstrap against Desktop PR #20.

Exit: every upstream source responsibility has a disposition and no inherited `implemented` status exists without Desktop comparison.

### Phase 2 — Runtime and contract parity

Port Desktop Coordinator/Host/Runner/shared/packages behavior required by iOS. Reuse Rust source directly when appropriate; otherwise use semantically equivalent iOS-owned implementations.

Exit: canonical send/stream/tool/cancel/retry/recovery paths match Desktop contracts.

### Phase 3 — iOS platform-main parity

Port Electron-main/preload responsibilities into iOS main/preload owners with native adapters.

Exit: no presentation layer owns platform/runtime orchestration.

### Phase 4 — Frontend/product parity

Port Desktop frontend product behavior to native SwiftUI, including iPhone/iPad layout adaptations.

Exit: product flows are driven by canonical Coordinator projections.

### Phase 5 — Communication/connectors/full product closure

Close all applicable Desktop PR #20 communication, MCP/connectors, automations, remote-computer, media, and product responsibilities.

### Phase 6 — Legacy removal

Delete Grok-direct compatibility paths and previous iOS bypass/fallback implementations that are not part of the Desktop-derived architecture.

### Phase 7 — Exact-HEAD acceptance

Run exact-HEAD GitHub Actions and packaged iOS acceptance, including archive/install and lifecycle recovery evidence.

## 16. Verification

All build/test work for this migration runs in GitHub Actions or the designated remote runtime. Do not use a local developer-machine build as acceptance evidence.

Architecture CI must fail when:

- Desktop source manifest and ledger differ;
- duplicate `desktop_path` rows exist;
- a row claims `implemented`/`verified` but its target is missing;
- a verified row lacks production/test evidence;
- a not-applicable row lacks an explicit platform reason/replacement;
- renderer bypasses Coordinator/Host ownership;
- another Fabushi source repository becomes required to build/run iOS;
- a shared Desktop/iOS runtime dependency is introduced contrary to this Spec;
- a stale Desktop exact HEAD is represented as current.

Final completion additionally requires all mandatory rows `verified` or reviewed `not-applicable`.

## 17. Acceptance criteria

- **AC-1**: Desktop PR #20 exact HEAD is explicitly pinned and provenance recorded.
- **AC-2**: 100% of selected Desktop `frontend/**` and `source/**` source-bearing files are present in the Desktop-based inventory and ledger.
- **AC-3**: Grok Bot 0.18 is no longer a direct iOS completion authority.
- **AC-4**: No shared Fabushi runtime repository is required for Desktop/iOS code reuse.
- **AC-5**: A clean iOS checkout owns all source/build inputs required by the iOS product.
- **AC-6**: Desktop -> iOS architectural correspondence in Section 4 is enforced.
- **AC-7**: Coordinator, Host, and Runner remain separate ownership boundaries on iOS even when implemented in one OS process.
- **AC-8**: Renderer does not own canonical operation/run/retry/transcript truth.
- **AC-9**: Electron-specific mechanisms are replaced by documented iOS-native adapters without silently dropping applicable product effects.
- **AC-10**: Existing PR #3 code is revalidated rather than grandfathered from the old Grok ledger.
- **AC-11**: Applicable Desktop Agent/chat lifecycle semantics are ported and verified.
- **AC-12**: Applicable MCP/connectors/tools behavior is ported and verified.
- **AC-13**: Applicable communication/messaging behavior in Desktop PR #20 is ported and verified.
- **AC-14**: iOS lifecycle/background/relaunch recovery preserves canonical durable state without duplicate execution.
- **AC-15**: Old bypass/fallback architectures are removed after cutover.
- **AC-16**: Architecture CI is based on the Desktop PR #20 source manifest/ledger.
- **AC-17**: Exact-HEAD compile/unit/contract/architecture/UI/lifecycle workflows pass on one final iOS SHA.
- **AC-18**: Exact-HEAD archive/export/install acceptance succeeds on the final accepted SHA.
- **AC-19**: Required rights/provenance review for reused source has no unresolved release-blocking item.
- **AC-20**: App Store/TestFlight delivery constraints have no unresolved release-blocking issue.
- **AC-21**: Final compliance table records every requirement/AC as passed, blocked, or not-applicable; mandatory completion requires all mandatory items passed.

## 18. Current compliance record

The authority cutover begins from iOS PR #3 exact HEAD `cdcbfd4344377b592ed882e049dfb1a36a534aa6`.

All implementation counts/statuses from the prior Grok-direct ledger are **historical only** until revalidated against Desktop PR #20 `84dbe458a8f14307bbdeaff469388734e6ce8879`. The current baseline contains 7,925 source-bearing `frontend/**` + `source/**` files. Relative to the immediately previous iOS baseline `95995bdf36a9687788e106c8544d292b2bb0877f`, Desktop advanced to `dcb19a94383833fc1ec5074f10c4bbbd28c09036` with exactly two source-bearing changes under the authoritative roots: `source/host/src/selected_image_inputs.rs` and `source/host/tests/transcript_send_echo_contract.rs`. Desktop then advanced from `bbc7b34a5f6dad46e3d4ca88fe21cc4f7932ce09` to `dcb19a94383833fc1ec5074f10c4bbbd28c09036` only by accepting the send-message-shaping architecture row; no `frontend/**` or `source/**` blob changed. The full 7,925-row source inventory was revalidated against the dcb19a tree with zero missing paths and zero blob mismatches. The newly accepted Desktop responsibility does not transfer status to iOS: send_message_shaping.rs, selected_image_inputs.rs, and agent_to_agent_messaging.rs are explicitly re-audited independently. The iOS port may advance a row only after its own Host-owned production path exists. The production change adds native ISO-BMFF HEIC/HEIF/AVIF primary-image dimension extraction with rotation handling; the test change adds focused contract coverage. All other 7,923 source-bearing blob identities are unchanged. Previously reviewed Coordinator `carrier.rs` and `client_side_tool_v2_relay.rs` blobs are unchanged, so their `implemented` status is preserved, but neither may be promoted to `verified` without exact-HEAD iOS CI and the relay's adjacent Host producer closure described above.

| Item | Status | Evidence / reason |
| --- | --- | --- |
| AC-1 | passed | Desktop PR #20 repository/PR/branch/exact HEAD are pinned in this Spec. |
| AC-2 | passed | All 7,925 selected Desktop source paths/blob identities were revalidated against the pinned dcb19a tree with zero missing paths and zero blob mismatches; manifest and ledger sourceCommit metadata are rebound to this authority. |
| AC-3 | passed | This Spec explicitly demotes Grok to historical provenance and supersedes the old direct authority. |
| AC-4 | passed-by-design | Standalone source ownership and no-shared-runtime rule are normative; repository audit still guards regression. |
| AC-5 | pending | Existing PR #3 is designed standalone; exact-HEAD clean-checkout acceptance must be rerun after rebaseline. |
| AC-6..AC-16 | pending | Existing implementation requires Desktop-based re-audit. |
| AC-17 | pending | New exact-HEAD CI evidence required after authority-cutover commits. |
| AC-18 | pending | Packaged acceptance required after implementation closure. |
| AC-19 | blocked | Reused-source provenance/right review must be updated for Desktop PR #20 source reuse. |
| AC-20 | pending | Final delivery evidence required. |
| AC-21 | pending | Final compliance review not yet complete. |

## 19. Superseded authority

`docs/specs/grok-bot-0.18-ios-architecture-parity.md` is retained only as historical migration context after this Spec lands. Where it conflicts with this document, this document wins.

The old Grok reference manifest and Grok parity ledger may remain temporarily for audit/history, but must not be used as the active architecture-completion gate after the Desktop-based gate is enabled.

### 11.10 Rebaseline: 0941e6b -> c81d95b (current)

Desktop PR #20 advanced from `0941e6b189738d0bfaed9b64c6a186d17e6d33ec` to `c81d95b2399b864d10209d4853231853b7742c50` through four production Host commits. The selected inventory remains 7,926 rows; six existing `source/host/**` blobs changed and no selected source path was added or removed. Older exact-HEAD CI/acceptance remains historical only.

The delta tightens canonical transcript ownership in four places: secret-store failures are projected as deduped error trays instead of escaping secure-input flow; live auto-review approvals are settled against the live approval owner before stale persisted-card fallback; transcript run-lifecycle observer, roster settlement, focus timestamp and shutdown are bound/disposed through the manager; and session group mutation dispatch is routed through the transcript owner rather than directly from shipping Host composition.

For iOS these are applicable ownership/product semantics, but Desktop process mechanics are not copied. The canonical iOS replacement remains the single `FeatureHostController` plus `KernelConversationProvider`/Runtime state: group mutations and group runs are already Host-owned; focus/deferred activation/roster-visible events are owned by `conversation_session` and Host event projection; approvals are keyed by runtime approval identity; and error trays are Host state. No SwiftUI or secondary session/transcript owner may implement these paths. Changed rows are rebound to the new Desktop blob identities and remain at most `mapped`/`implemented` until the same accepted iOS exact HEAD has focused behavior plus required Actions evidence.

### 11.11 Rebaseline: c81d95b -> e4e0c92 (current)

Desktop PR #20 advanced from `c81d95b2399b864d10209d4853231853b7742c50` to `e4e0c92a5312e27004b7eb4a195cece9ca39f165` in one production Host commit. The selected inventory remains 7,926 rows; four existing Host blobs changed. The delta restores automation-configuration and listener-connect observers as state owned/bound by `TranscriptManager` and attached by the transcript extension, while shipping Host composition supplies callbacks rather than owning observer state directly.

This is applicable on iOS as an ownership rule, not an Electron/process mechanism. `FeatureHostController` remains the sole Host product owner for automation/listener state and emits canonical `HostEvent` projections; SwiftUI does not bind or retain a parallel observer registry. The affected rows are rebound to the new blob identities, and older exact-HEAD CI remains historical only.

### 11.12 Rebaseline: e4e0c92 -> 387a5ec (current)

Desktop PR #20 advanced from `e4e0c92a5312e27004b7eb4a195cece9ca39f165` to `387a5ec97a06a9bb5ee1f25242a2f0a75a9c155d` in one production Host commit. The selected inventory remains 7,926 rows; three existing Host blobs changed. Agent lifecycle gateway dispatch is now routed through `TranscriptManager`, so shipping Host composition no longer owns the create/update/delete/list lifecycle dispatch path directly.

The iOS equivalent remains `FeatureHostController`, where bot/agent lifecycle mutation, canonical state, persistence hooks and emitted `HostEvent` projection share one Host owner rather than a SwiftUI/session-side gateway owner. No new platform mechanism is required, but the changed upstream rows and acceptance evidence are rebound to this exact Desktop HEAD.

### 11.13 Rebaseline: 387a5ec -> d3b2477 (current)

Desktop PR #20 advanced from `387a5ec97a06a9bb5ee1f25242a2f0a75a9c155d` to `d3b2477ef7941554946a92d1267e5f6110e9137a` in one production Host commit. The selected inventory remains 7,926 rows; two existing Host blobs changed. `TranscriptManager` now synchronizes the memory subsystem's active-agent identity after successful agent-lifecycle and session gateway mutations, and clears/synchronizes the memory owner as transcript lifecycle settles.

On iOS the equivalent invariant is that account/conversation lifecycle and active-agent memory-facing identity remain canonical Host/Runtime state, never SwiftUI-owned parallel state. The changed source rows are rebound to the new blob identities; older exact-HEAD acceptance remains historical only.


### 11.14 Rebaseline: d3b2477 -> f2a2e181 (current)

Desktop PR #20 advanced from `d3b2477ef7941554946a92d1267e5f6110e9137a` to `f2a2e1811bbd2f1d7677672fd4b88f48e54b4cef` in one production Host commit. The selected inventory remains 7,926 rows; four existing `source/host/**` blobs changed and no selected source path was added or removed. Shipping Host main no longer binds the WidgetResponses channel-config callback directly. `TranscriptManager` now creates and owns the callback proxy, the production Transcript extension binds the manager-owned observer to `transcript.channel-config-changed`, and dispose clears that observer. The focused manager contract rejects a second WidgetResponses channel-config owner and proves the signal is emitted after successful channel-credential persistence.

This is applicable to iOS as an ownership and lifecycle rule, not an Electron mechanism. The current iOS replacement remains the single Host-owned `FeatureHostController`; no SwiftUI or secondary observer registry may become canonical. The affected Desktop rows are rebound to the new blob identities and remain `mapped`: iOS still needs equivalent credential-change projection plus current exact-HEAD focused behavior/Actions evidence before any promotion. Older acceptance bound to d3b2477/cbe85ca is historical only.


## Recovery baseline 2026-10-03: c51bf234

Current Desktop PR #20 authority is `c51bf2344c21e200b6a05dacaa7d3382b02abcc0`, replacing `4186cea169a71a0ebe38849f844dc153de204078`. Earlier rebaseline sections and their acceptance statements are historical. The selected inventory remains 7,927 blobs. The changed responsibilities are TurnAgentComposition MCP discovery projection and its MCP-state/subagent contracts. Shipping Desktop wraps the routed bridge with the canonical MCP-state projection before observation/audit and built-in tool injection, while execution and partial-call observation delegate to the existing bridge. iOS must independently prove the same discovery semantics on its native shipping path; changed rows cannot inherit implemented/verified evidence. No Coordinator/Host/Runner boundary is collapsed.

The listener transcript projection must interpolate the platform display name in pending/connected titles and default detail, preserve the Host-provided entry identity, and use a platform-specific fallback identity when entryId is absent. Swift remains a projection only; resume ownership remains in Rust Host. Existing Swift contract assertions must remain strict, with additional fallback identity/default-detail coverage.

Current iOS 380c6dc8 Actions evidence is partial: Rust Host and unsigned device archive succeeded; Swift listener projection assertions failed, ordinary UI tests were skipped, and protected Global Dharma auto-login failed before complete-state verification. No listener acceptance promotion is authorized. The protected failure requires separate diagnosis of session restoration/preparation and cannot be relabeled as success or a runner cancellation.


### Recovery follow-up authority: 9a748dc4

Desktop advanced to `9a748dc4acb3058815b24b662613e420445857e6` after the recovery commit. The only changed blob is `source/host/tests/system_prompt_shipping_wiring_contract.rs`: assertions now follow HostRunnerComposition computer preparation/lease/finish delegation instead of the superseded direct owner calls. This is a Desktop contract correction, not new iOS shipping behavior. All 7,927 source identities are rebound; the changed row requires independent review and no acceptance status is promoted.


### Recovery rebaseline 2026-10-04: `a59a8fce`

Desktop PR #20 remains open and draft on `refactor/grok-018-architecture-rebuild`, but its exact HEAD advanced from the iOS repository's previously pinned authority `9a748dc4acb3058815b24b662613e420445857e6` to `a59a8fce1495c8d770839ee9ebbc4c1ceba20a92`. All Desktop-bound source identity and acceptance claims for changed rows are therefore invalid until independently revalidated against this exact HEAD.

A direct GitHub compare from `9a748dc4` to `a59a8fce` contains 229 upstream commits and 127 selected source changes, all under `source/host/**`: 117 existing source-bearing blobs changed, 10 source-bearing files were added, and no selected source file was deleted or renamed. The authoritative selected `frontend/** + source/**` inventory is now 7,937 files; `source/host/**` increases from 957 to 967 files.

The rebaseline is intentionally fail-closed. Every one of the 117 changed existing Host rows has its old iOS disposition, implementation status, production evidence, and test evidence invalidated to `unreviewed`; the 10 new Host rows also begin `unreviewed`. Historical target paths and notes may remain only as investigation context. Unchanged Desktop blobs keep their prior review status, but every manifest/ledger chunk is rebound to the current Desktop exact HEAD. No parity status is promoted by this rebaseline.

The new Host source responsibilities include generated-image and web dependency/tool owners, an await-shell tool, turn tool-session reminders, and focused contracts for those owners plus MCP await handling. Existing shipping responsibilities also changed across HostRunnerComposition, turn-agent composition/checkpoint/input projection, system prompt assembly, computer/external-machine routing, MCP state/management, Auto Review, telemetry, transcript-mirror codecs, reply/recovery and subagent execution. Each must be reviewed as responsibility + owner + state machine + product effect; a generic Host facade or ledger-only assertion is not sufficient.

Desktop PR #26 remains observation-only at this baseline. Its unmerged FBCP/Human communication work is not a formal iOS source until that work is merged into PR #20 and PR #20 advances to a new exact HEAD.


Rebaseline recovery note: the generated `source-mahayana` ledger chunk was restored byte-for-byte from its pre-rebaseline Git blob and only its `sourceCommit` authority was rebound to `a59a8fce1495c8d770839ee9ebbc4c1ceba20a92`. The earlier intermediate SHA with a zero-byte chunk is rejected evidence and must not be used for acceptance.


### Recovery rebaseline 2026-10-04: `1fbc34fb`

Desktop PR #20 advanced from `a59a8fce1495c8d770839ee9ebbc4c1ceba20a92` to `1fbc34fba5739edf303bc5fb9ca4cae4d0a08a5c` while remaining open and draft on `refactor/grok-018-architecture-rebuild`. A direct GitHub compare reports three upstream commits and exactly one changed file: `desktop/e2e/openbot-packaged-acceptance.spec.ts`. There are zero changes under the authoritative iOS-selected `frontend/** + source/**` roots, so the selected inventory remains exactly 7,937 files and all selected Desktop blob identities are unchanged.

The upstream change strengthens Desktop signed-packaged acceptance by recognizing both the legacy `app://bundle/` renderer and the shipping `file:.../dist/renderer/index.html` renderer, extending the attachment deadline to 120 seconds, and binding `SAND_USER_DATA_DIR` to the isolated packaged acceptance data directory. This is Desktop packaging-test mechanism, not a new iOS runtime/product responsibility. Therefore existing iOS row dispositions/statuses are not downgraded solely because of this delta, but all Desktop-bound authority metadata is rebound to `1fbc34fba5739edf303bc5fb9ca4cae4d0a08a5c` and all older exact-HEAD CI/acceptance remains historical only.

The iOS acceptance consequence is fail-closed: the current iOS exact HEAD must rerun its own architecture, Rust Host/Runner, simulator UI/unit, and device-archive workflow against this rebased authority before any new acceptance claim is made. No Desktop packaged-acceptance success is treated as evidence for iOS archive/install behavior.

### Recovery rebaseline 2026-10-04: `c92a2cca`

Desktop PR #20 advanced one commit from `1fbc34fba5739edf303bc5fb9ca4cae4d0a08a5c` to `c92a2ccae3e022942475c4d3db00c8f910f8243a` while remaining open and draft on `refactor/grok-018-architecture-rebuild`. A direct GitHub compare reports exactly one changed file, `desktop/e2e/openbot-packaged-acceptance.spec.ts`, and zero changes under the authoritative iOS-selected `frontend/** + source/**` roots. The selected inventory therefore remains exactly 7,937 files and every selected Desktop blob identity is unchanged.

The upstream commit, `test: launch signed macOS candidate through production path`, changes only Desktop signed-packaged acceptance mechanics: it launches the installed signed app as a normal production process, exposes loopback CDP only for readiness discovery/attachment, polls Chromium's real target registry, and binds Playwright only after the shipping renderer target exists. This is Desktop packaging/acceptance behavior rather than a new iOS product responsibility. Existing iOS row dispositions and implementation statuses are therefore preserved, but all Desktop-bound sourceCommit/index/checker authority is rebound to `c92a2ccae3e022942475c4d3db00c8f910f8243a`.

All CI, archive, protected-session, and acceptance evidence tied to the prior Desktop authority remains historical only. The new iOS exact HEAD created by this rebaseline must obtain its own required GitHub Actions and protected-account evidence before any current-head acceptance promotion.



### Recovery rebaseline 2026-10-04: `7c00ea34`

Desktop PR #20 advanced four commits from `c92a2ccae3e022942475c4d3db00c8f910f8243a` to `7c00ea34fd64102cfe71266e646720369f2397e8` while remaining open and draft on `refactor/grok-018-architecture-rebuild`. A direct GitHub compare reports exactly one change under the authoritative iOS-selected `frontend/** + source/**` roots: the new file `source/packaging/offline-asr-engine.json`. No previously selected source file changed, was removed, or was renamed. The selected inventory therefore increases from 7,937 to exactly 7,938 files.

The new source is Desktop packaging provenance/configuration for the mandatory offline ASR engine: whisper.cpp tag/commit plus the default model URL, SHA-256, and byte size. Rebaseline does not assume that Desktop packaging mechanics are directly applicable to iOS. The new `source-packaging` row begins `unreviewed`; its product responsibility, iOS-native replacement/disposition, production wiring, and acceptance evidence must be reviewed before promotion. Existing row dispositions/statuses are preserved because their Desktop blobs are unchanged.

All Desktop-bound manifest and ledger chunks, both indexes, the active authority files, and the strict architecture checker are rebound to `7c00ea34fd64102cfe71266e646720369f2397e8`. CI and acceptance evidence from the prior Desktop authority remain historical only; the resulting iOS exact HEAD must obtain its own same-HEAD Actions evidence before any current-baseline acceptance claim is made. Desktop PR #26 remains observation-only because it is still unmerged into PR #20.


#### iOS adaptation decision: mandatory offline voice transcription

The newly selected Desktop packaging manifest is not treated as desktop-only merely because iOS cannot ship a macOS `whisper-cli`. Its applicable product effect is a voice-input transcription path that can operate locally without a network transcription dependency, with an explicit build/platform provenance boundary. The iOS mechanism is native: the existing AVFoundation `VoiceRecorder` remains the single microphone-capture owner, while a dedicated Speech-framework adapter performs transcription with `requiresOnDeviceRecognition = true`. The adapter must reject unavailable/unsupported on-device recognition and must not silently fall back to server recognition.

For the Agent/Bot composer, recording state remains ephemeral UI input state; canonical conversation/runtime state remains behind the existing iOS trusted bridge and Coordinator/Host/Runner chain. Stopping a voice-input recording transcribes the captured file and places the trimmed text into the composer draft for explicit user review/edit/send. Transcription never auto-sends a turn. Cancellation or account/Agent surface replacement must discard the in-flight recording/transcription result rather than applying stale text to a different composer.

The iOS release contract must include the Speech authorization usage description and focused tests proving the no-network-fallback policy and composer wiring. The Desktop `source/packaging/offline-asr-engine.json` row may move from `unreviewed` to `implemented` only after this shipping path is present; it remains below `verified` until the resulting exact iOS HEAD passes architecture, Swift/unit/UI/lifecycle, archive, and applicable packaged/release acceptance.

### Recovery rebaseline 2026-10-04: `8556281e`

Desktop PR #20 advanced exactly one commit from `7c00ea34fd64102cfe71266e646720369f2397e8` to `8556281e5eb20aeb6dba781bd5ecf8c623206d6e` while remaining open and draft on `refactor/grok-018-architecture-rebuild`. A direct GitHub compare reports one changed file only: `desktop/e2e/openbot-packaged-acceptance.spec.ts`. No file under the authoritative iOS-selected `frontend/** + source/**` roots changed, was added, removed, or renamed. The selected inventory therefore remains exactly **7,938** files and every selected Desktop blob identity remains unchanged.

The upstream change strengthens Desktop packaged acceptance so it follows the canonical ProductionRenderer workspace rather than stale shell/login selectors: acceptance observes the `messenger-workspace` production shell, real account status/onboarding phases, and a usable Mahayana Host/Coordinator `bot.list` path before declaring the signed candidate ready. This is an acceptance-contract change outside the selected source inventory, not a new iOS source responsibility. It therefore does not create a ledger row and does not automatically promote any existing iOS row or packaged/release acceptance status.

All Desktop-bound manifest and ledger chunks, both indexes, `MIGRATION_SOURCE.md`, the active Spec authority, and the strict architecture checker are rebound to `8556281e5eb20aeb6dba781bd5ecf8c623206d6e`. Existing row dispositions/statuses are preserved because their selected Desktop blobs are unchanged. All CI, archive, protected-session, offline-ASR, and other acceptance evidence produced against the previous Desktop authority is historical for current-baseline completion; the resulting iOS exact HEAD must obtain its own same-HEAD GitHub Actions evidence before any current-baseline `verified` promotion. Desktop PR #26 remains observation-only because it is still unmerged into PR #20.

### MCP App Surface bridge lifecycle parity: iOS-adapted

Desktop `frontend/packages/mcp-app-sdk/src/bridge.ts` binds sandboxed MCP App RPC to a private transferred MessagePort plus a `pluginInstanceId`, a per-load nonce, explicit grants, and deterministic disposal that rejects pending RPC. Desktop `webmcp.ts` aborts native registration on disposal while retaining the local Fabushi registry as the compatibility fallback when the draft native registration API is unavailable or rejects registration.

iOS does not emulate a browser-to-parent MessagePort. Its platform replacement is the private `WKScriptMessageHandler` channel restricted to the local Mini App origin. The same security/lifecycle responsibility still applies: each local Mini App source load receives a fresh instance identity and nonce; native calls must present both and name a tool included in that load's explicit grants; duplicate request identities are rejected; pending native tasks are owned by the bridge session and cancelled when the WebView is torn down, replaced by a hosted source, or the page sends its one-shot disposal on `pagehide`; responses are delivered only while the exact originating bridge session remains active. The page must reject its own pending promises on disposal and abort every native WebMCP registration signal.

The existing `window.__fabushiWebMcp` registry remains the iOS compatibility path. If `document.modelContext.registerTool` exists, the same granted tools are additionally registered through that draft native API; registration failure does not remove the local registry or create a second runtime owner. Per-tool approval continues to be enforced by the native Coordinator before a mutating runtime call. This bridge/session work changes lifecycle and capability fencing only; canonical Mini App runtime/tool truth remains in the existing model/runtime owner rather than the WebView.
