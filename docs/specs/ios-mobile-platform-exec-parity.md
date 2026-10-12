# iOS mobile platform execution parity

Status: active / implementation incomplete  
Spec ID: IOS-MPX-001  
Owner: Fabushi iOS existing Coordinator -> Rust Host -> Runner graph  
Desktop authority: bhrumom/fabushi-desktop@371b72f1cd20de03dc4dff9a15ed761054968093  
Desktop tree: 3116c54a1e36da95189c740bdc789ad543ce2d20  
Target: fabushi-ios/fabushi-ios PR #3  
Related source ledger: docs/parity/desktop-main/source-local-exec-daemon.json

## 1. Context

Desktop current main ships local-exec daemon responsibilities through `source/local-exec-daemon/main.ts` and `source/local-exec-daemon/production-executor.ts`: generation-bound process identity, controlled execution, cancellation, resource cleanup, typed/protobuf execution messages, lifecycle shutdown and observability. iOS must preserve the product responsibility and security semantics without starting a localhost daemon or spawning arbitrary local child processes.

The iOS tree already owns `IOSProductionLocalExecutor`, `RemoteRunner`, `IOSGenerationFencedRemoteRunnerTransport`, typed box-exec operations and local allowlisted native capabilities. The shipping Coordinator currently reaches that executor only for `openExternal -> openExternalURL`. Rust MobileEmbedded intentionally configures `ProcessExecution::Unavailable`, so a real Agent `process_exec` request cannot reach the platform Runner. The current RemoteRunner dispatch closure also routes back to `MahayanaCoordinator.request`, which is the same local Rust Host. It must not become a synchronous Host -> Swift -> Host re-entrant execution loop.

The current remote-computer service is not a generic command relay: its worker only accepts the reviewed WebRTC signaling kinds `offer`, `answer`, `ice`, `ready` and `close`. No unverified `/agent-boxes/.../exec` or other arbitrary execution endpoint may be invented.

## 2. Goal

Close the real shipping caller gap while preserving one canonical runtime:

`Agent/Host request -> Host-owned mobile platform-exec broker -> Coordinator -> IOSProductionLocalExecutor -> IOSGenerationFencedRemoteRunnerTransport -> reviewed remote execution provider`.

Local Apple capabilities remain on the existing allowlist. Shell/process/box execution never runs as an iPhone child process. If no reviewed remote execution provider is configured, the request fails closed after reaching the production Runner boundary; absence of a provider is a release blocker, never fake success.

## 3. Non-goals

- No second Agent runtime, scheduler, Host, state store or Swift-owned conversation runtime.
- No localhost shell/daemon on iOS.
- No `Process`, `NSTask`, embedded Docker, arbitrary dynamic library or hidden local process substitute.
- No reuse of Remote Computer WebRTC signaling rows as a command channel.
- No fabricated backend route, test-only production bypass or mock success in acceptance evidence.
- No promotion of global migration or local-exec ledger rows merely because source/tests compile.

## 4. Requirements

### IOS-MPX-R1 — Host-owned broker

MobileEmbedded Rust Host owns a bounded platform-exec broker. Native Agent process execution submits a typed request to this broker and awaits exactly one terminal result. Request identity is collision-resistant and stable across Coordinator polling. The broker owns queued/leased/terminal state; duplicate polling must not execute the same request twice.

### IOS-MPX-R2 — Mobile process semantics

`ProcessExecution::Host` and `LocalDocker` remain impossible for MobileEmbedded/WebWasm. Mobile may expose process tools only when a platform broker is installed. `process_exec`, `git_status` and `git_diff` use the broker asynchronously. A missing broker fails closed with `CapabilityUnavailable`.

### IOS-MPX-R3 — Shell/process/box namespace

The platform boundary accepts only exact `shell`, `process` and `box` roots plus their dotted descendants. Unknown or lookalike names fail closed. Broker envelopes preserve method, typed JSON parameters and request id. Existing `IOSBoxExecOperation` continues to use the same RemoteRunner rather than a second box runtime.

### IOS-MPX-R4 — Coordinator service loop

While waiting for a canonical Host operation, Coordinator services pending platform-exec requests through the existing `IOSProductionLocalExecutor`. It resolves or rejects the exact broker request before polling the next one. Coordinator shutdown/cancellation stops servicing new work and rejects/cancels owned pending requests rather than leaving a late writeback.

### IOS-MPX-R5 — Account/revision/generation fencing

Every remote execution uses the existing `IOSRemoteRunnerExecutionIdentity`: account scope, account revision and Host generation are captured before dispatch and revalidated after dispatch. Account switch, logout, revision advance, Host failure/recovery or generation replacement makes the result stale and prevents commit into the old Host request.

### IOS-MPX-R6 — Remote provider boundary

`IOSGenerationFencedRemoteRunnerTransport` dispatches to a dedicated remote-execution provider, not back into the same Host request path. The provider is an injected existing-owner service. Until a reviewed production provider contract is available, the shipping provider fails closed with an explicit unavailable error. Tests may inject deterministic transports but release evidence must identify the real provider and service contract.

### IOS-MPX-R7 — Cancellation and resource cleanup

Task cancellation propagates from Coordinator operation wait to the platform request and Agent operation. Broker terminal settlement removes request state. Host close/recovery rejects outstanding requests and clears queued/leased resources. Late results are ignored. No unbounded completed-result cache is permitted.

### IOS-MPX-R8 — Idempotency

A broker request id may transition queued -> leased -> terminal once. Repeated `next` calls do not return a leased request. Repeated resolve/reject for the same terminal id is idempotent only when the terminal payload matches; conflicting duplicate settlement fails closed. Unknown ids never create state.

### IOS-MPX-R9 — Native allowlist regression

`openExternalURL`, background transfer, share item and clipboard allowlisted local capabilities continue to execute only through `IOSNativeLocalCapabilityBackend`. Adding Agent platform execution must not route those capabilities remotely or broaden permissions.

### IOS-MPX-R10 — Evidence

All executable validation runs only in GitHub Actions. Focused tests must exercise the real Rust Host broker and shipping Coordinator owner, not source-string-only reachability. Exact-head evidence records commit/tree, run/job/step and artifacts. Old-head success is predecessor evidence only.

## 5. Interfaces

The Rust Host exposes typed host methods with one owner:

- `platformExec.next` -> null or `{ requestId, method, params }`; leasing is atomic.
- `platformExec.resolve` -> `{ requestId, result }`.
- `platformExec.reject` -> `{ requestId, code, message }`.
- `platformExec.cancel` -> `{ requestId }`, used only for owned in-flight work.

These methods are control-plane operations on the existing Host handle. They are not user-facing tools and do not create a second runtime.

The Swift Coordinator may service only broker methods returned by the Host. It calls `IOSProductionLocalExecutor.execute(method:params:)`; that executor remains the one namespace and allowlist gate.

## 6. Failure modes

Cover: missing account, logout/account switch during execution, account revision change, Host generation change, Host recovery, Coordinator task cancellation, Host close, missing remote provider, unknown method, duplicate lease/settlement, conflicting terminal settlement, malformed payload, provider failure, late result after cancel, and resource cleanup.

Any ambiguous remote side effect with unknown completion state must not be automatically replayed. A retry requires a fresh broker request or a provider-specific idempotency contract proving safe reuse.

## 7. Verification

Focused Rust tests:
- MobileEmbedded exposes process tools only with the broker.
- Real NativeEngine `process_exec` submits a broker request and receives its resolved result.
- missing broker/provider fails closed;
- lease is single-consumer;
- resolve/reject/cancel and duplicate settlement are idempotent/fail-closed as specified;
- close rejects outstanding work and leaves zero pending state.

Focused Swift tests:
- shipping Coordinator services Host broker requests through `IOSProductionLocalExecutor`;
- shell/process/box namespaces reach the RemoteRunner;
- unknown names fail closed;
- Task cancellation prevents late commit;
- account switch/revision and Host generation changes fence results;
- Host failure/recovery fences old results;
- duplicate broker request is executed at most once;
- resources are cleaned;
- `openExternal` and other local allowlisted capabilities remain local.

CI:
- architecture;
- canonical rust-host including native engine, Host and mobile app-host;
- Swift build/unit and UI;
- protected-account and offline gates when the new HEAD reaches those workflows.

## 8. Acceptance / ledger policy

The `main.ts` and `production-executor.ts` rows in `docs/parity/desktop-main/source-local-exec-daemon.json` remain `mapped` until:
1. the real Host/Agent caller reaches the broker and shipping Coordinator/Executor graph;
2. exact-head focused Rust and Swift tests succeed;
3. account/revision/generation, cancellation, duplicate and cleanup behavior is evidenced;
4. a reviewed production remote execution provider exists for the required remote effect.

If 1-3 pass but 4 remains unavailable, record the implementation progress and external-provider blocker without marking the rows `verified`. Global `migration_complete` remains false until IOS-MAIN-AC-01..21 close.

## 9. Rollback and observability

Rollback must leave MobileEmbedded process spawning disabled. Broker request/terminal telemetry may include request id, method root, account-safe generation markers and error code, never credentials, shell secrets or unsanitized environment values. Host recreation drops old-generation broker work rather than replaying it.
