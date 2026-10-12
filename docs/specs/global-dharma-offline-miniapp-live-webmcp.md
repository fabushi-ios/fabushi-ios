# Global Dharma Offline Mini App + Live WebMCP — iOS Implementation and Acceptance Specification

Status: active; implementation and exact-HEAD acceptance required  
Owner: fabushi-ios/fabushi-ios native Marketplace / Mini App / Host integration  
Last updated: 2026-10-11  
Related: PR #3; Desktop FPM-001; Desktop OMA-001; iOS full-parity spec

## 1. Context / problem

Fabushi iOS already contains a native Marketplace, MCP/Skills surfaces, a strict .fabapp verifier, local-author signing, local package loading, and a WebMCP bridge. However the current Global Dharma path is not an accepted offline Mini App vertical.

At discovery source `fabushi-ios/fabushi-ios` PR #3 `1f1deb151cb2c137ba5f7f1470a14be263661b28`:
- `GlobalDharmaMiniAppView.swift` still builds an inline document and uses `loadHTMLString`;
- its document unconditionally calls a remote `status` path and `refreshEntitlement()` on load;
- `GlobalDharmaMiniAppBridge.callOfficialMcpTool` opens a remote MCP session at `/api/mcp/apps/global-dharma`;
- `MarketplaceModel` projects a separate account-scoped Global Dharma execution record in UserDefaults;
- `MiniAppWebMcpSurface` already has the generic installed-package loader, per-account WebKit data store, per-load instance/nonce/grants, cancellation, and local `loadFileURL` path, but it only has a page-to-Host execution direction and Global Dharma is still special-cased to remote MCP / projected Host state.

Therefore existing strings such as “WebMCP 已连接” or “Bot / Web UI 同一共享状态” do not prove Bot -> current WKWebView execution.

Live authorities re-read for this task:
- iOS main: `8b0e7236d75f8e16b868332e51cde41b6ee415fc`
- iOS PR #3 pre-evidence HEAD for the 2026-10-12 rebaseline: `49c566a9c37373f4040c8624db2ef07e2171ce75` (the evidence/source commit produced by this review is newer; only its exact-head CI may be current evidence)
- Desktop main: `371b72f1cd20de03dc4dff9a15ed761054968093`
- Backend main: `9c56a77cbe02dfceebf574236a703b53cd257cb9`
- Platform Core main: `c301820b93274bda586fbb82be982afc3c2ee4d4`
- Canonical Fabushi server/payment main: `c4fd9ca7aa1346fe8e4a07e2461a70f4b9f16b52`

The original discovery HEAD was `1f1deb151cb2c137ba5f7f1470a14be263661b28`; it and all earlier workflow runs are historical diagnostics only. Final acceptance must bind to the latest exact PR HEAD.

## 2. Goal

Make Global Dharma the first production reference implementation of the existing canonical iOS Mini App architecture:
1. one Marketplace / Host install truth;
2. one verified local-package execution path;
3. free offline behavior that never depends on entitlement or Marketplace availability;
4. protected capability checks only at protected execution boundaries;
5. one live WebMCP session bound to the current WKWebView, account, installed package, load nonce and generation;
6. Bot/Host can discover and execute tools registered by the active page and observe the same durable state/UI;
7. the same platform primitives remain reusable by Marketplace and AI-created Mini Apps.

## 3. Non-goals / out of scope

- No second Mini App store, runtime, chat client, database truth, entitlement service, or Agent runtime.
- No claim that remote-only Global Dharma operations are offline merely because their names appear in the page.
- No arbitrary native code, downloaded iOS framework, shell or unrestricted filesystem bridge.
- No StoreKit/App Review/device acceptance claim without real configured evidence.
- Android/Desktop UI implementation is not part of this iOS-first slice.
- Do not change SKU, amount, payment rail, entitlement scope or publisher trust without current canonical service evidence.

## 4. Requirements

### Architecture and package

- **GD-ARCH-001** Global Dharma opens through `MiniAppWebMcpSurface` and the same Host-owned install/source APIs used by other Mini Apps. The old special `GlobalDharmaMiniAppView` must not remain a shipping parallel runtime.
- **GD-PKG-001** Stable app ID is `global-dharma`. Its offline production document/assets are represented by the canonical installed package descriptor including version, digest and entrypoint.
- **GD-PKG-002** Installed resources execute only after normal Host/package validation. Local package loading uses the generic restricted WebKit loader and cannot fall back to a network-hosted Global Dharma page for an app advertised as offline-ready.
- **GD-PKG-003** Per-account WebKit storage is preserved; sign-out/account switch/version replacement fence old sessions.
- **GD-PKG-004** Install/update is atomic and digest/version-bound. Invalid or interrupted updates must not replace the last-good active package.

### Free/offline boundary

- **GD-OFF-01** After a valid installation, Global Dharma cold-starts with network disabled, renders all declared free/offline resources, performs at least one real local free mutation, persists it, and restores it after app restart.
- **GD-OFF-02** Opening the app and exercising free/offline tools performs no entitlement preflight. Entitlement/commerce failure cannot block free UI or free state.
- **GD-OFF-03** Remote-only functions are explicitly classified as network services and are not used as proof of offline execution.

### Live WebMCP

- **GD-WEBMCP-01** The active page publishes its actual tool definitions/schema into the native session; discovery evidence comes from the current loaded page, not only catalog hard-coding.
- **GD-WEBMCP-02** Native Host/Agent execution targets an active session by account scope + app ID + installed version/digest + app instance + load nonce + session generation.
- **GD-WEBMCP-03** A Host-originated tool call executes inside the current WKWebView's page runtime, returns the page's actual result, mutates the page's real local business state where applicable, and updates visible UI without reload.
- **GD-WEBMCP-04** Page-originated and Host/Bot-originated operations read/write one state store and strictly monotonic revision. Manual page changes must be visible to subsequent Host/Bot reads and vice versa.
- **GD-WEBMCP-05** Free local tools remain executable through the native Host test entrypoint while network is disabled. Cloud-model availability is tested separately.
- **GD-WEBMCP-06** Wrong account/app/version/digest/origin/nonce/generation/tool/schema, duplicate request, closed page, crash, cancellation, timeout, disabled tool and stale result fail closed.
- **GD-WEBMCP-07** Ordered trace records app/account scope (privacy-safe), version/digest, instance ID, nonce hash or non-secret identifier, generation, call ID, tool, approval, state revision and outcome so same-instance execution can be proven without logging secrets.

### Permissions and entitlement

- **GD-PAY-01** `local.prayer-wheel.start` remains protected. The trusted Host checks current canonical server entitlement immediately before executing that capability. Network/service failure fails only that protected operation.
- **GD-PAY-02** Current server-authoritative commerce contract, re-read from `bhrumom/fabushi@c4fd9ca7aa1346fe8e4a07e2461a70f4b9f16b52`:
  - mini app `global-dharma`
  - protected capability `local.prayer-wheel.start`
  - monthly SKU `local-prayer-wheel.monthly`, product `prod.global-dharma.local-prayer-wheel.monthly`, CNY 3000 minor units, 30-day subscription
  - lifetime SKU `local-prayer-wheel.lifetime`, product `prod.global-dharma.local-prayer-wheel.lifetime`, CNY 108000 minor units, durable entitlement
  - Apple/Google purchase rails are exposed only when their server provider bindings are actually active; unknown subscription expiry, cancellation, revocation and expiration fail closed.
- **GD-PAY-03** Purchase/restore must use real authorized StoreKit/canonical service flows in production. CI synthetic commerce stays explicitly test-only and cannot be cited as production payment acceptance.
- **GD-PERM-01** Read-only tools may use no-prompt grants only when explicitly declared. Mutations require the existing approval owner unless an equivalent durable per-tool user grant exists.
- **GD-PERM-02** Web content never receives account bearer tokens, OAuth secrets, Keychain, arbitrary MCP servers, shell, Computer, filesystem or entitlement-signing authority.

### Marketplace / Skills / Connectors / AI-created apps

- **GD-MKT-01** Global Dharma install/remove/update uses the one Marketplace/Host owner and normal signed package/install state; no test-only bypass.
- **FPM-IOS-01** Existing native Marketplace Discover/Installed/Created/Team, search/detail, compatibility/version, install/enable/disable/update/remove, account/team policy and data lifecycle remain canonical and are expanded rather than forked.
- **FPM-IOS-02** MCP Connectors preserve OAuth/PKCE, account-scoped credentials, initialize, tools/list, tools/call, revoke/refresh/error recovery and Host approval.
- **FPM-IOS-03** Skills preserve create/edit/install/enable/disable/update/Agent use/private/team publish/sync/revoke in existing owners.
- **FPM-IOS-04** AI-created private Mini Apps use existing draft -> review -> zero-privilege preview -> exact digest approval -> local signing -> normal install flow. Offline execution after install is distinct from cloud/offline model generation.

## 5. Current feature responsibility matrix

| Product responsibility | Current owner at discovery HEAD | Classification | Offline promise | Entitlement |
| --- | --- | --- | --- | --- |
| Marketplace package install/source/lifecycle | MarketplaceModel + existing Coordinator/Host `feature.marketplace.*` / `feature.plugin.*` | platform | installed metadata and packages remain usable offline where declared; discovery/update may require network | no |
| MCP Connectors install/OAuth/tool execution | MarketplaceModel + CoordinatorMcpSurface + canonical native MCP client/Host permission owner | connector service | no; external connector execution depends on provider/network | per-tool Host approval; OAuth/account scope where required |
| Skills create/edit/install/enable/disable/update/sync/Agent use | shipping Plugins/Yours UI -> MarketplaceModel.refreshPrivateSkills -> `coordinator.skill.pluginFactsSync` -> Coordinator/Host plugin facts -> canonical `workflow.*` owner | workflow/Agent platform | installed declarative skill state is local/account-scoped; external actions inherit their connector/network needs | capability/tool-specific only |
| Local package render/storage | MiniAppWebMcpSurface + WKWebView | free platform | yes after install | no |
| Global Dharma `home/status/logs/validate_config/start/stop/loop/chat/send/deploy_latest` | remote `/api/mcp/apps/global-dharma` via GlobalDharmaMiniAppBridge | remote service | no | only when returned protected Host request requires it; iOS preserves this route only when no active local-page tool fence exists |
| Local WebMCP session identity/permission | MiniAppWebMcpSurface + ActiveMiniAppWebMcpSessionRegistry | free platform | yes | per-tool grant/approval |
| Local prayer wheel start | Global Dharma + trusted Host boundary | protected | no by default because fresh online entitlement required | yes |
| Purchase/restore | GlobalDharmaCommerceModel + StoreKit + service | paid service | no | yes |
| AI-created private Mini App draft/review/sign/install | Agent artifact -> MarketplaceModel `feature.plugin.localDraft.*` -> exact-digest review -> LocalMiniAppSigningAuthority -> `feature.plugin.installLocalFabapp` -> `feature.plugin.uiDocument` -> MiniAppWebMcpSurface | platform | installed app yes; cloud model generation is separately network-dependent | requested capabilities only after exact-digest user approval |
| Mobile/Web native process execution boundary | mahayana-app-host + mahayana-host + mahayana-native-engine; canonical Remote Runner remains the platform execution path | security/runtime | local Mini App/Skill operation must not gain arbitrary app-sandbox process execution | process execution is unavailable for MobileEmbedded/WebWasm; Host/Runner policy applies to remote execution |

This table must be updated from code/service evidence if new actual Global Dharma product responsibilities are found. No function is reclassified by name alone.

## 6. Target state and ownership

```text
Marketplace / Bot / native shell
        |
        v
MarketplaceModel + existing Coordinator/Host
        |
        +---- Host package/install truth ---- verified local .fabapp
        |
        +---- ActiveMiniAppWebMcpSessionRegistry (thin iOS bridge owner)
                  key: account + plugin + version/digest
                  value: current instance + nonce + generation + weak WKWebView endpoint
                  |
                  v
             MiniAppWebMcpSurface
                  |
                  v
             current WKWebView
                  |
                  +---- page registered WebMCP tools
                  +---- app-scoped durable Web storage
                  +---- visible UI
```

The registry is a narrow lifecycle/transport adapter inside the existing Mini App WebMCP owner, not a second runtime or data store. It does not own business state. It must remove/fence sessions at navigation, teardown, account change and source/version change.

Global Dharma's free local business state lives in app-scoped Web storage exposed by its installed page. The Host keeps only trace/session metadata needed for routing/audit. The old `globalDharmaExecution` UserDefaults projection is migration-only and must not be the post-cutover state owner.

## 7. Interfaces / contracts / data flow

### Page registration
Page -> bridge:
`ready { pluginInstanceId, nonce, tools:[{name,title,description,inputSchema,annotations}] }`

Native validates current trusted main frame, expected plugin/version/digest, nonce/session generation and schema before publishing tools to the active-session registry.

### Host-to-page call
Native -> page:
`invoke(callId, pluginInstanceId, nonce, generation, tool, arguments)`

Page runtime resolves against the page's own registered tool implementation. Completion returns:
`result { callId, pluginInstanceId, nonce, generation, ok, result|error, stateRevision? }`.

### Protected capability
A page tool may request a typed protected Host capability. Host validates ownership and entitlement. WebMCP arguments never carry an “entitled=true” escape hatch.

### Local state
Global Dharma package exposes at least:
- `offline.capabilities` read-only;
- `offline.state.read` read-only;
- one real free local mutation tool defined in the package and UI, with monotonic revision.

Exact names may be adjusted during implementation if an existing product owner provides canonical names; tests and this spec must then be updated before implementation diverges.

## 8. Constraints / non-functional requirements

- GitHub Actions only for executable build/test/package/automation acceptance.
- No local/container/device builds.
- Existing PR #3 must be updated without overwriting unrelated parallel migration work.
- WebMCP call timeout is bounded; cancellation/teardown is deterministic.
- All JSON/schema inputs are bounded and validated.
- No secret/token logging.
- Offline package must not require CDN/font/script/image/Wasm download at launch.
- iOS policy/device/store acceptance is independent and may remain blocked while code/CI phases continue.

## 9. Failure modes and edge cases

Cover network loss, entitlement outage, malformed/unsigned/tampered package, stale package pointer, old version still open during update, navigation away, WebView crash, duplicate/late calls, account switch, logout, app disable/remove, permission deny, unregistered tool, schema mismatch, wrong origin/frame/nonce, protected-capability prompt injection, interrupted update, storage restoration and rollback.

## 10. Implementation strategy

1. Keep Global Dharma on the generic Marketplace -> Host install -> `feature.plugin.uiDocument` -> `MiniAppWebMcpSurface` path; remove/deprecate the special inline-WebView runtime.
2. Extend the generic WebMCP bridge with active-session registration and reverse Host-to-page invocation.
3. Make page-provided definitions authoritative for live-session discovery while intersecting them with Host/catalog grants.
4. Implement Global Dharma offline package/page behavior using app-scoped Web storage and observable UI/revision.
5. Remove unconditional entitlement/status network work from free app startup.
6. Route protected prayer-wheel execution through trusted Host entitlement checks only at invocation.
7. Route Bot/Agent calls to the active page when a matching live session exists; remote MCP remains only for explicitly remote service tools, never as a substitute for local live WebMCP.
8. Expand unit/UI/protected workflows and evidence state.
9. Preserve Connectors/Skills/AI-created Mini App owners and add regression acceptance rather than parallel implementations.

## 11. Verification / test strategy

All executable verification runs in GitHub Actions on the exact tested SHA.

Required gates:
- Swift unit tests, including strict package verifier and local-author signing.
- Rust Host/runtime tests relevant to plugin install, UI document/resource, account fencing and MCP routing.
- Global Dharma parity tests for startup network behavior, state revision, schema/session fencing and entitlement boundary.
- UI journey that keeps the Mini App live while invoking a real Host-to-page tool and observes an in-place UI/state change.
- native Host entrypoint offline WebMCP call with network disabled.
- stale-session/permission-deny/account-switch/version-change/teardown tests.
- Simulator clean-install and restart persistence.
- archive/device signing paths where configured.
- real StoreKit/device/App Review acceptance only when infrastructure is genuinely available.

## 12. Acceptance criteria / Definition of Done

- **GD-OFF-01 / GD-OFF-02**: exactly as requirements above, with network audit and restart evidence.
- **GD-PAY-01**: protected operation performs online entitlement at execution; free operation remains usable after failure.
- **GD-PAY-02**: real purchase/restore evidence when configured; otherwise blocked, never simulated passed.
- **GD-WEBMCP-01..07**: page-discovered live tools, current-instance read/write, same-page UI mutation, bidirectional revision consistency, offline local Host call, denial/stale/cancel coverage, ordered trace evidence.
- **GD-MKT-01**: install/open/update/remove through the normal Marketplace/Host path.
- **GD-REL-01**: release policy/signing/device evidence for the actual intended iOS channel; missing external prerequisites are blocked.
- **FPM-IOS-01..04**: no regression of Marketplace/Connectors/Skills/private Mini App flows and exact-head CI evidence.

No completion claim until every applicable row is passed or explicitly blocked/not-applicable with reason.

## 13. Release / migration / rollback

- Existing hosted Global Dharma may remain only as an explicitly online compatibility source while the offline package is being introduced; once Global Dharma is advertised offline-ready, launch must fail clearly rather than silently downgrade to hosted Web.
- Migrate only data that is proven to be user business state. Do not convert the old Host execution projection into app business truth.
- Package update uses immutable digest/version and last-good rollback.
- Disabling/removing the app tears down the live session and prevents future calls immediately.
- Rollback to the previous package must preserve compatible app-scoped data per package schema/migration policy.

## 14. Observability / evidence

For each accepted call record privacy-safe account scope identifier, app ID, installed version/digest, app instance, load/session generation, call ID, tool, permission outcome, state revision, execution outcome and timing. Never log access/refresh/OAuth tokens or StoreKit secrets.

Acceptance evidence must record repository, exact SHA, workflow/run/job IDs, command/exit status, tests/counts when available, package digest/version, network state, and relevant screenshots/video/trace.

## 15. References / provenance

- Desktop `docs/specs/fabushi-plugins-marketplace-open-ecosystem.md` (FPM-001)
- Desktop `docs/specs/offline-first-installable-mini-app-platform.md` (OMA-001)
- iOS `docs/specs/fabushi-desktop-main-ios-parity.md`
- iOS `docs/specs/ios-marketplace-action-isolation.md`
- `mobile/ios/Fabushi/MarketplaceModel.swift`
- `mobile/ios/Fabushi/MiniAppWebMcpSurface.swift`
- `mobile/ios/Fabushi/OfflineMiniAppPackageVerifier.swift`
- `mobile/ios/Fabushi/LocalMiniAppSigningAuthority.swift`
- `mobile/ios/Fabushi/GlobalDharmaMiniAppBridge.swift`
- `mobile/ios/Fabushi/GlobalDharmaCommerceModel.swift`
- `.github/workflows/ios-protected-global-dharma.yml`


### 2026-10-12 execution evidence refresh

This execution round is intentionally fail-closed. It does **not** convert pending, cancelled, skipped, neutral, or externally blocked evidence into Passed.

- Live refs were re-read directly through Git refs: iOS main `f098639ab178a839690b9dece60fa0d11c5d2e25`; Desktop main `371b72f1cd20de03dc4dff9a15ed761054968093` / tree `3116c54a1e36da95189c740bdc789ad543ce2d20`; Backend main `9c56a77cbe02dfceebf574236a703b53cd257cb9`; Platform Core main `c301820b93274bda586fbb82be982afc3c2ee4d4`; canonical Fabushi server main `c4fd9ca7aa1346fe8e4a07e2461a70f4b9f16b52`; Marketplace main `1971f2a93197cfb79d1e8f0856345f36776f3ca3`.
- Historical iOS evidence head `ba067cd838496c847715acda9b22c8d39c908a6f` and runs `38154970619` / `38154970497` are observation-only because PR #3 advanced.
- New Actions-only offline acceptance is owned by `.github/workflows/ios-protected-session.yml` job `global-dharma-offline`. It performs canonical online Marketplace preparation, proves a real public HTTPS probe works before isolation, disables enabled macOS runner network services, requires the same real public probe to fail, then cold-starts the installed app and invokes `/offline.state.read` plus approved `/offline.practice.record` through the native Host panel against the active page. It terminates/relaunches Fabushi again while isolated and requires the same persisted revision, restores network, then requires the real public probe to recover. No invalid URL or synthetic network mock is accepted as isolation evidence.
- Cold app initialization no longer preflights Marketplace catalog or remote MCP discovery. Catalog/MCP refresh remains surface-driven; protected `local.prayer-wheel.start` retains its fresh Host-side entitlement boundary.
- Active WebMCP pending-result fencing now includes package version and artifact digest in addition to account/instance/generation. Focused tests cover package identity replacement and immediate removal of a disabled page tool, complementing existing wrong-account, wrong instance/nonce/generation/tool, schema additionalProperties/maxLength, approval-denial, origin trust, and timeout/page-teardown contracts.
- Live-page approval reconciliation is enforced end-to-end: `registerPageTools` computes the strongest catalog/page approval and the Host-to-page invocation path now carries that effective `MiniAppToolContract` into `requestApproval`. A page may strengthen but never weaken the Host approval class, and invocation traces can no longer record an approved mutation when the effective page contract required a prompt that was never shown. Exact-head Actions remain pending.
- Backend production signer preflight run `38156689037`, job `114521016801`, exact Backend `9c56a77cbe02dfceebf574236a703b53cd257cb9`, failed closed with exit code 1 because all five managed Worker secrets remain absent: `FABUSHI_MARKETPLACE_SIGNING_PRIVATE_KEY`, `FABUSHI_MARKETPLACE_SIGNING_PUBLIC_KEY_RAW`, `FABUSHI_MARKETPLACE_SIGNING_KEY_ID`, `FABUSHI_MARKETPLACE_TRUST_EPOCH`, and `FABUSHI_MARKETPLACE_REVIEW_TOKEN`. No production package/signature is fabricated.
- Signer support thread `1a12b70942632c11` and Apple/provider thread `1a12bc3ea0b5880b` were re-read; neither contains a provider/configuration-complete reply. Duplicate reminder mail was not sent. Consequently production `.fabapp` finalization and GD-PAY-02 remain externally blocked.
- Protected UI acceptance now separates the core real Marketplace/WebMCP journey from the externally gated Apple purchase journey. Synthetic commerce remains disabled. Apple purchase/restore becomes a hard gate only when `FABUSHI_E2E_REQUIRE_REAL_APPLE_COMMERCE=1`; otherwise the evidence records `commerceExternalBlock=true` and cannot be cited as GD-PAY-02 Passed.
- Exact-head Actions in this round have repeatedly been invalidated by unrelated/concurrent PR #3 commits. Only a terminal run whose `head_sha` equals the final PR head is release evidence. Architecture-only success followed by cancelled/skipped rust-host/build/archive jobs is not Passed.
- Predecessor head `edb314b350f059d175adb13b2e0ccdb814a2eb00` run `38174750828` is diagnostic only. Its `architecture` job `114574730349` succeeded, but `rust-host` job `114574783892` failed in required step `Test iOS-owned Rust Host modules and canonical messaging core` because the newly added `InstalledPluginSkillSnapshot` used `#[serde(default = "default_true")]` without defining `default_true`; generated-Mini-App, Skills/Agent and native-MCP required groups plus `build-and-test` and `device-archive` were consequently skipped. This is a deterministic production compile defect, not acceptable evidence.
- The compile defect was repaired in `98f8e04cd53cbbce53d7196f473f18a0d21c0d8a`; `eab873fa633bc1639a75b500ec4f73e1304f8fa2` adds coverage that installed plugin skills default to Agent-enabled when the wire snapshot omits the optional enablement field.
- FPM-IOS-03 was also tightened before promotion: Marketplace-installed Skill materialization now requires the effective plugin `versionRef` to normalize to an exact 40-hex commit, rewrites a GitHub `blob/<ref>/.../SKILL.md` source to that immutable commit before fetching, stores the normalized commit as `pluginVersion`, and records the pinned source URL. Mutable refs such as `main`, non-GitHub sources, or non-exact effective versions are not materialized as installed executable Skills. The Rust Host sync boundary independently revalidates the normalized 40-hex `pluginVersion` and requires `sourceRef` to identify the same GitHub `blob/<commit>/...` source, so a forged or stale projection cannot bypass Coordinator pinning. The implementation/test ancestry is `4017e1b9f01b03f24bf2be1ecc639c64c00f513f` -> `b9160c1a43cd04cebbd60bdd63454c506fb773dc` -> `53722176152047e27da95430b4fb18c5c3de50b9`; final acceptance still requires fresh exact-head Actions after this evidence update.
- Predecessor head `ef652ded4a6b623eedb6c41c334384c2fd23f94b` exposed a deterministic Swift 6 test-compile defect in `SharedExperimentServiceParityTests`: an actor-isolated `script.calls()` was awaited inside an XCTest autoclosure. This was repaired by awaiting before assertion; the resulting `9caf6f5261497c69365fa80724c33717208a0f38` rust-host job `114558878273` in run `38169447549` then executed and passed all four required Host groups (canonical messaging, generated Mini App install/recovery, Skills/Agent lifecycle, native MCP/persistence/runtime/protected-account) plus the end-of-job Desktop authority check.
- The same `9caf6f5261497c69365fa80724c33717208a0f38` remained non-acceptable because protected-session run `38169447408` job `114558979913` failed at `Build app and UI tests once while online`; therefore Marketplace preparation, runner network isolation, offline cold start, local WebMCP read/mutation, persistence recovery and evidence-state gates were skipped. The log identified further Swift 6 actor-isolation/API-drift compile failures in Gateway, Feedback, HumanCall, Global Dharma Commerce and generated-Mini-App hardening regression tests. Those defects were repaired without disabling tests or weakening strict concurrency; the code-fix ancestor is `c7a9454671d7d2041b5399910eb5fccaff85276b`. Any runs on `9caf6f5...` or intermediate repair commits are predecessor evidence only. Final promotion requires a new terminal exact-head architecture/rust-host/build/archive plus offline/protected acceptance after this evidence update.
- Acceptance evidence now closes the gap between in-memory WebMCP fencing and the real protected journey. `ActiveMiniAppWebMcpSessionRegistry` emits privacy-safe OSLog records for the live session and ordered tool calls (SHA-256 account identifier plus non-replayable nonce identifier; app/version/digest/instance/generation/tool/permission/revision/outcome, with no raw account/token/nonce). The protected workflow extracts only those records from the real Fabushi process and hard-fails unless one four-tool Global Dharma session identity is shared by a denied mutation, an approved mutation at `botMutationRevision`, and a no-approval read-back at `manualMutationRevision`, with strictly ordered trace sequence. The gate is also bound to the expected business tools and causal order: denied `offline.practice.record` -> approved `offline.practice.record` -> `offline.state.read`; every matched trace must carry a numeric unique sequence, the three call IDs must be distinct, and their ordered sequence/tool identity is persisted in `webmcp-live-trace.json`.
- FPM-IOS-01..04 focused regression is widened beyond the previous six suites. The exact-head gate now directly selects Marketplace lifecycle/client, MCP OAuth/auth-watch/account lifecycle/manager/tools discovery, Coordinator MCP surface, live Mini App session registry, Global Dharma Mini App parity, strict offline package verification, shared Workflow/Skill contracts, typed Skill publishing transport, and Agent surface authorization/fencing. Full `FabushiTests`, all UI tests, Rust Skills lifecycle/Agent-integration tests, generated Mini App install/recovery tests, and archive remain independent required gates.
- Native MCP transport is now an explicit exact-head Rust gate rather than an inferred capability: `mahayana-mcp-runtime --all-targets` exercises the shipping `NativeMcpClient` over a real loopback HTTP server and requires the protocol sequence `initialize -> notifications/initialized -> tools/list` and `initialize -> notifications/initialized -> tools/call`, including session-id propagation and exact call parameters.

### 2026-10-12 current-head rebaseline after 9537fdb3

- PR #3 advanced from the requested checkpoint `9537fdb3e717959cc751bb1adb8038a023478d23` to pre-evidence-update HEAD `f5b75151342bd09c18a584ecf2acce312cf4f574`, tree `576bf945a4123c83fc2dde4cc083eea073ffcd1e`; base main remains `f098639ab178a839690b9dece60fa0d11c5d2e25`. Desktop, Backend, Platform Core, canonical Fabushi server and Marketplace authorities remain respectively `371b72f1cd20de03dc4dff9a15ed761054968093`, `9c56a77cbe02dfceebf574236a703b53cd257cb9`, `c301820b93274bda586fbb82be982afc3c2ee4d4`, `c4fd9ca7aa1346fe8e4a07e2461a70f4b9f16b52`, and `1971f2a93197cfb79d1e8f0856345f36776f3ca3`.
- Runs `38178756048` and `38178756051` are predecessor evidence only. On `9537fdb3...`, architecture `114586623575` and rust-host `114586665615` succeeded, but build-and-test `114588885686`, device-archive `114588885798`, protected-account-ui `114587126111`, and global-dharma-offline `114587126292` were cancelled, so that checkpoint never satisfied current-head acceptance.
- Pre-evidence-update current-head runs are `38179810400` and `38179810398`. Architecture `114589864397` has succeeded; rust-host `114589906003` has passed canonical Runner composition and iOS-owned canonical messaging and is proceeding through generated Mini App / Skills / native MCP groups. Protected-account-ui `114589949737` and global-dharma-offline `114589949940` have passed exact-source/authority/account/source-boundary setup and clean Simulator boot and are still executing. These results become predecessor diagnostics as soon as this evidence update creates a new exact HEAD; no pending step is promoted to Passed.
- The `f5b7515...` production change closes a mobile/web process-execution safety gap: iOS/Android use `BuildProfile::MobileEmbedded`, Web uses `WebWasm`, and both profiles force `ProcessExecution::Unavailable` inside the native engine even if a caller asks for Host/Docker execution. Electron/Mock desktop retains the desktop process policy. User-visible shell/process work on mobile must therefore route through the existing governed platform Remote Runner rather than spawning arbitrary app-sandbox processes. This boundary applies to Agent/Skills and AI-created Mini App flows as well as the generic Host.
- The shipping responsibility chain is re-recorded above: Marketplace lifecycle remains in MarketplaceModel + existing Host; MCP Connectors remain in CoordinatorMcpSurface/native MCP + Host approval; Skills refresh flows from the real Plugins/Yours UI through `MarketplaceModel.refreshPrivateSkills()`, `coordinator.skill.pluginFactsSync`, Host plugin facts and then `workflow.*`; AI-created Mini Apps flow from Agent artifact through Host local-draft materialization/review, native local-author signature, normal `installLocalFabapp`, `uiDocument`, and the same MiniAppWebMcpSurface.
- Signer thread `1a12b70942632c11` and Apple/provider thread `1a12bc3ea0b5880b` were re-read after the drift and still contain no configuration-complete reply. The five managed Cloudflare signer settings remain externally blocked; Apple product/provider binding and real StoreKit eligibility remain externally blocked. No duplicate reminder was sent and no synthetic payment/package result is promoted.

### 2026-10-12 rebaseline after App Store update-policy wiring

- PR #3 advanced from evidence candidate `292cee1bb1e38f4b8dce78cba16504df209b905d` to pre-evidence-update `73ee80da13988206552db2586a1c89c87e472ec3`, tree `d3214cbfa8cf8ad93f12c7ef0f45df7b5c8cc92d`, via `fix(ios): wire App Store update policy`. Base main remains `f098639ab178a839690b9dece60fa0d11c5d2e25`.
- The five external authorities remain unchanged: Desktop `371b72f1cd20de03dc4dff9a15ed761054968093`, Backend `9c56a77cbe02dfceebf574236a703b53cd257cb9`, Platform Core `c301820b93274bda586fbb82be982afc3c2ee4d4`, Fabushi server `c4fd9ca7aa1346fe8e4a07e2461a70f4b9f16b52`, Marketplace `1971f2a93197cfb79d1e8f0856345f36776f3ca3`.
- Runs `38180270382` / `38180270380` are predecessor evidence only after this drift. On `292cee1...`, architecture `114591200642` and rust-host `114591247331` completed successfully; rust-host passed canonical Runner, iOS-owned messaging, generated Mini App install/offline recovery, Skills/Agent, native MCP/persistence/runtime/protected-account, including `codex-core-skills --all-targets` and `mahayana-mcp-runtime --all-targets`. At drift time, build-and-test `114593092247`, device-archive `114593092235`, protected-account-ui `114591302526`, and global-dharma-offline `114591302793` were still in progress and therefore cannot be promoted as current-head acceptance.
- The new production change is release/update plumbing, not a second Marketplace/Mini App/WebMCP owner. It makes `checkForUpdates` fetch App Store version policy asynchronously, retains a prior offer on refresh error, enforces stable App Store/TestFlight track semantics, and fails closed for desktop-only nightly/idle-apply controls. It does not alter the single Host/Marketplace install state, Local WebMCP registry, Global Dharma local state, entitlement boundary, Skills lifecycle, MCP Connector owner, or AI-created Mini App pipeline recorded above.
- New exact-head workflows are required after this evidence update. No GD-OFF, GD-WEBMCP, FPM-IOS, archive, payment, signer, or release requirement is promoted by this documentation commit.

### 2026-10-12 exact-head rebaseline after 54832f40

- The requested checkpoint `54832f40583009aa963d31573e7c502cc7700ce1` is predecessor evidence only. PR #3 advanced to pre-evidence-update HEAD `b6ae32fed4ea822ab82864a8858d2137cbc69c31`, tree `b68ac57e5b7ccc82e38be419648c4845efe8135f`; base main remains `f098639ab178a839690b9dece60fa0d11c5d2e25`.
- Since that checkpoint, the production delta is bounded to Marketplace/plugin account-generation and native-engine/runtime fencing; a concurrent documentation commit also closes current-main review for macOS application placement and Windows user-data migration as iOS platform replacements. This does not create a second Marketplace, Mini App, WebMCP, Skills, Connector, or Global Dharma state owner.
- Desktop authority remains `371b72f1cd20de03dc4dff9a15ed761054968093` / tree `3116c54a1e36da95189c740bdc789ad543ce2d20`; Backend remains `9c56a77cbe02dfceebf574236a703b53cd257cb9`, Platform Core `c301820b93274bda586fbb82be982afc3c2ee4d4`, canonical Fabushi server `c4fd9ca7aa1346fe8e4a07e2461a70f4b9f16b52`, and Marketplace `1971f2a93197cfb79d1e8f0856345f36776f3ca3`. The 7,951-row Desktop source index and direct-impact denominator therefore stay unchanged.
- Runs `38188168213`, `38188168239`, `38189392211`, and `38189392169` are predecessor diagnostics only. This evidence update creates a newer exact iOS HEAD, so no step from those runs is promoted to GD/FPM/IOS-MAIN acceptance; replacement exact-head architecture, Rust Host, Swift/UI, archive, offline/protected and completeness/release evidence is mandatory.
- Signer thread `1a12b70942632c11` and Apple/provider thread `1a12bc3ea0b5880b` were re-read and still contain no configuration-complete reply. No duplicate reminder was sent. The five managed signer settings and real Apple product/provider/test eligibility remain external blockers.
- No GD-OFF, GD-WEBMCP, GD-MKT, GD-PAY, FPM-IOS, signed-device, TestFlight/App Store, IOS-MAIN or final acceptance status is promoted by this rebaseline.

## 16. Spec compliance record

| Requirement / AC | Status | Evidence / reason |
| --- | --- | --- |
| GD-ARCH-001 | implemented; exact-head CI pending | Shipping entry uses generic `MiniAppWebMcpSurface`; the legacy `GlobalDharmaMiniAppView` parallel `loadHTMLString` runtime has been removed from PR #3. |
| GD-PKG-001..004 | implementation closed; production signer rollout blocked | Global Dharma 1.1.0 source is owned by `bhrumom/fabushi-marketplace` and uses the strict catalog `.fabapp` manifest/finalizer contract. The iOS production install path now preserves `platform=ios`, selects the iOS ZIP-container `.fabapp`, requires release `fabappTrust`, verifies the outer digest/size and Ed25519 package manifest through `mahayana-plugin-runtime::verify_fabapp_bytes`, reconciles app/version/digest/requested-capabilities, and only then journals/activates the install. Backend PR #28 keeps this release fail-closed until immutable artifact URL/source SHA/digest/size plus managed signer key ID/public key are all present. Marketplace source-contract CI is already green; production finalization still requires the managed signing Worker configuration. |
| GD-OFF-01..03 | implementation present; journey evidence pending | The 1.1.0 package uses only local `index.html/style.css/app.js`, CSP `connect-src 'none'`, localStorage-backed revision state, and no startup entitlement request. Cold-start/restart/network-off UI evidence must still complete on the current exact iOS HEAD. |
| GD-WEBMCP-01..07 | implementation present; latest exact-head UI/security evidence pending | Active page tools register through `registerLocalTool`; native Host invokes the current page via `invokeLocalFromHost` fenced by account/app/version/digest/instance/nonce/generation. Page tool schemas must match reviewed catalog schemas, Host validates arguments before execution, and the strongest catalog/page approval is carried into the actual Host prompt so page annotations can strengthen but never weaken Host approval. Host-originated page calls are bounded to 15 seconds with abort-on-pagehide; WebKit process/navigation failure tears down the active session. With an active Global Dharma page, remote MCP fallback is explicitly rejected; without an active page, the pre-existing governed remote-service route remains available and is never cited as local WebMCP evidence. The registry records bounded ordered invocation traces with call ID, account/app/package/version/instance, privacy-safe nonce identifier, generation, tool, permission decision, state revision and outcome; the protected journey covers denial-before-approval and same-page bidirectional revision changes. |
| GD-PAY-01 | implementation present; protected journey pending | `local.prayer-wheel.start` is the only current protected local capability; `callActiveWebMcpTool` performs fresh Host-side entitlement refresh only for that protected tool. Free tools bypass entitlement preflight. |
| GD-PAY-02 | implementation aligned; production purchase evidence blocked | Canonical server main defines both CNY 30 / 30-day monthly (`local-prayer-wheel.monthly`) and CNY 1080 lifetime (`local-prayer-wheel.lifetime`) offers for `local.prayer-wheel.start`. iOS now parses and validates both server offers, exposes both choices, refuses client-side price/kind/SKU drift, requires the server `activeRails` projection before StoreKit, and restores either SKU before re-reading canonical entitlement. Real Apple purchase/restore still requires provider/App Store configuration. |
| GD-PAY-03 / GD-PAY-02 acceptance | blocked | Real StoreKit/device/payment acceptance depends on external Apple/product/account configuration; CI test rail is not production evidence. |
| GD-MKT-01 | implementation closed; production artifact rollout blocked | Global Dharma discovery and release selection preserve native iOS instead of collapsing to generic `mobile`; the catalog emits the 1.1.0 package only when immutable release metadata and signer trust are complete, otherwise iOS release is unavailable rather than falling back to legacy 1.0.0/Hosted MCP. The normal Host `feature.plugin.install` path consumes the exact `releaseManifest` and its `fabappTrust`. Production-signed 1.1.0 artifact/catalog rollout remains blocked by managed signer configuration. |
| FPM-IOS-01..04 | pending exact-head regression | Focused Actions now directly runs MarketplaceModelLifecycleTests, SharedMarketplaceClientParityTests, SharedMcpOAuthLifecycleParityTests, SharedMcpAuthWatchLifecycleParityTests, SharedMcpAccountLifecycleParityTests, SharedMcpManagerParityTests, SharedMcpToolsDiscoveryParityTests, CoordinatorMcpSurfaceTests, ActiveMiniAppWebMcpSessionRegistryTests, GlobalDharmaMiniAppParityTests, OfflineMiniAppPackageVerifierTests, SharedWorkflowModelParityTests, SharedAccountMcpParityTests and FabushiAppAgentSurfaceTests. Rust-host independently runs codex-core-skills lifecycle/Agent-integration tests plus generated `.fabapp` install/recovery contracts; full FabushiTests and all UI tests remain required. Current exact-head success is still required before promotion. |
| GD-REL-01 | blocked | Real signed-device/TestFlight/App Review acceptance requires configured Apple distribution infrastructure and policy evidence. |


## 2026-10-12 shipping local-exec final-boundary rebaseline

- PR #3 requested checkpoint `a93626a8f60dcc36dc229f0980a3cee9ea552a82` and runs `38190289874` / `38190289840` are predecessor evidence only. The later `948f60d0` runs `38192173184` / `38192173175` are also predecessor after production `process.exec` completion at pre-evidence HEAD `5dda071eb22a1a8c197fb8e9c8755579968e7814` (tree `f57aa160e7ced7e0397d37235c1db9e2e5c15d25`).
- Desktop authority remains `371b72f1cd20de03dc4dff9a15ed761054968093` / tree `3116c54a1e36da95189c740bdc789ad543ce2d20`; Backend `9c56a77cbe02dfceebf574236a703b53cd257cb9`, Platform Core `c301820b93274bda586fbb82be982afc3c2ee4d4`, Fabushi server `c4fd9ca7aa1346fe8e4a07e2461a70f4b9f16b52`, Marketplace `1971f2a93197cfb79d1e8f0856345f36776f3ca3`.
- Shipping local-exec now reuses the existing account-scoped Cursor sandbox exec-daemon: `PlatformExecBroker -> MahayanaCoordinator -> IOSProductionLocalExecutor -> IOSGenerationFencedRemoteRunnerTransport -> CoordinatorMcpSurface.dispatchRemoteExecution -> dashboard.ensureSandBox() -> IOSRemoteExecDaemonClient -> agent.v1.ExecService/Exec`. The provider is not a second runtime and does not call back into the same Host request.
- `ExternalShell -> shell.exec`, `process_exec -> process.exec`, and `ExternalRead -> box.exec` preserve structured method identity through the broker/Coordinator. Only the final exec-daemon provider converts structured argv for `process.exec` into an authenticated ShellStream request with POSIX-safe argument quoting. Unknown methods remain fail-closed.
- Account scope/account revision/Host generation and routed scope-token fencing remain mandatory before and after dispatch; cancellation and late results cannot promote stale state. This implementation statement does **not** promote any GD-OFF/GD-WEBMCP/GD-MKT/GD-PAY/IOS-MAIN release gate. Fresh exact-head GitHub Actions and the artifact field audit remain required.

- Focused remote-provider acceptance additionally requires independent provider-failure, Task-cancellation, account-switch late-result, Host-generation late-result and unknown-method rejection tests; these are present in the post-c9a3a8e3 evidence/test commit and still require exact-head Actions before promotion.

- Strict architecture evidence note: predecessor run `38192567443` rejected a multi-path `ios_target_path` in the implemented local-exec ledger. The canonical owner is now singularly `source/mahayana-agent-coordinator/local-exec/supervisor.swift`; the production executor and process-identity files remain supporting composition evidence. The validator was not relaxed.

- Current-head rebaseline after `b13b769f`: runs `38190289874`, `38190289840`, `38192173184`, `38192173175`, `38192487666`, `38192487704`, `38192567443`, `38192567432`, `38192646437`, `38192646349`, `38192850135`, `38192850173`, `38193218500`, `38193218520` are predecessor only. Strict architecture run `38193218500` found stale current-main review metadata for `source/shared/os-notification.ts`; Desktop `371b72f1cd20de03dc4dff9a15ed761054968093` retains exact blob `ffaf599dada09e938f362d75aa202327eed3a4ad`, so the row is rebound to current-main-reviewed without any verified/release promotion. Fresh exact-head Actions are mandatory.


## Live rebaseline — 2026-10-12 current PR #3

- Requested checkpoint `c408673139a8ebb92a6e94aeb73727de01124ac1` is predecessor-only.
- Desktop authority remains `371b72f1cd20de03dc4dff9a15ed761054968093` / tree `3116c54a1e36da95189c740bdc789ad543ce2d20`; Backend `9c56a77cbe02dfceebf574236a703b53cd257cb9`; Platform Core `c301820b93274bda586fbb82be982afc3c2ee4d4`; Fabushi server `c4fd9ca7aa1346fe8e4a07e2461a70f4b9f16b52`; Marketplace `1971f2a93197cfb79d1e8f0856345f36776f3ca3`.
- Runs `38193384215`, `38193384217`, `38193596582`, and later `38194466221` / `38194466227` are predecessor evidence only after PR-head advancement. Their green steps and artifacts cannot promote GD/FPM/IOS-MAIN acceptance.
- Current responsibility work re-read four platform-only Desktop rows at exact current-main blobs and retained them as concrete iOS platform replacements; no implemented row was mechanically promoted to verified.
- Production signer, real StoreKit/provider binding, signed Global Dharma 1.1.0 .fabapp, physical-device install/upgrade, TestFlight/App Store, strict completeness, and independent acceptance remain blocked/pending until same-final-SHA evidence exists.


## 2026-10-12 current-head responsibility-completeness rebaseline after d705013a

- Requested checkpoint `d705013a1ae4be5c36057b7ff1444f1a5b1aeb19` is predecessor-only. PR #3 advanced through `e8e22c1f0098ef502742bec47d2b7050de8a0e2e` to pre-evidence HEAD `8a53722d0b2c8f5681781d0ce2e82f8b562b3243`; iOS main/base remains `8b0e7236d75f8e16b868332e51cde41b6ee415fc`.
- External authorities were re-read and remain unchanged: Desktop `371b72f1cd20de03dc4dff9a15ed761054968093` / tree `3116c54a1e36da95189c740bdc789ad543ce2d20`; Backend `9c56a77cbe02dfceebf574236a703b53cd257cb9`; Platform Core `c301820b93274bda586fbb82be982afc3c2ee4d4`; Fabushi server `c4fd9ca7aa1346fe8e4a07e2461a70f4b9f16b52`; Marketplace `1971f2a93197cfb79d1e8f0856345f36776f3ca3`.
- Runs `38194840503`, `38194840499`, and strict-completeness `38194881441` are predecessor evidence only. The latter failed closed with about 1.6e4 responsibility errors dominated by mandatory `mapped/implemented` rows that still lack final owner/review closure and stale historical `verified` rows that lack current-head responsibility/evidence/reviewer binding. Its validator was not weakened.
- Later runs `38195149103` / `38195149107` and `38195278502` / `38195278457` are likewise predecessor diagnostics once this evidence commit lands. The intervening production delta at `8a53722d...` adds generation-fenced Coordinator failure-episode telemetry and focused tests; it does not replace the Marketplace, Mini App, WebMCP, Skills, Connector or Global Dharma canonical owners.
- The requested Desktop batch was re-read at exact current-main blobs: MCP App SDK app-surface WebMCP, app-surface contract, bridge, commands, types, WebMCP registry; shared app-experience; renderer bootstrap; Agent delete/name/row-actions; CommandPalette; and ProductionRenderer. Current iOS shipping owners were independently inspected.
- No production gap was found in that bounded batch requiring a second runtime. `commands.ts` is deliberately demoted from historical `verified` to `implemented` because its old evidence was bound to an older iOS/Desktop head. `ProductionRenderer.tsx` is promoted from `mapped` to `implemented` because the current shipping chain `FabushiSceneRoot -> FabushiRuntime -> ProductionRenderer -> GrokMobileShell` is real and singular; it remains below `verified` until descendant completeness, exact-head Actions, artifacts and independent review close.
- Signer thread `1a12b70942632c11` still has only the two prior blocker messages; Apple/provider thread `1a12bc3ea0b5880b` still has only the original blocker notice. No configuration-complete reply exists, so no duplicate email was sent. Production-signed Global Dharma 1.1.0 and real StoreKit GD-PAY-02/03 remain blocked.
- `dependenciesReviewed`, `completenessAccepted`, `fullReviewAccepted`, `newAndChangedResponsibilitiesReviewed`, `finalAcceptance` remain fail-closed; `finalIosCommit` remains null.


### 2026-10-12 command-palette responsibility batch after b12c1016

- Exact-input strict completeness run `38195693072`, job `114637704813`, successfully checked out iOS `b12c1016e80fc937f6a3181a381983cc329c9559` and Desktop `371b72f1cd20de03dc4dff9a15ed761054968093`, then failed closed in `--strict --complete`; the log reports `15,938 additional errors (truncated)`. Authority-only recheck passed.
- Current-main source owners were re-read for `command-palette-link-provider.ts`, `command-palette-message-provider.ts`, `command-palette-model.ts`, `command-palette-provider.ts`, `command-palette-root-commands.ts`, and `command-palette-search-provider.ts`, together with shipping `GrokMobileCommandPaletteModel.swift`, `GrokMobileShell+CommandPalette.swift`, and focused tests.
- The four historical provider rows are demoted from stale `verified` to `implemented`; their old Actions/artifacts are predecessor evidence and are not rebound synthetically. `command-palette-model.ts` and `command-palette-root-commands.ts` are corrected from `mapped` to `implemented` because current shipping owners and tests exist. No row is promoted to `verified`.
- This commit supersedes runs `38195681012` / `38195680859` and `38195693072`; replacement exact-head Actions and strict completeness are required.


## Current exact-head rebaseline — 2026-10-12 local-tool permission scope

- Requested checkpoint `e9070d368902c65f669464ccdcd9ea1bcbfe3d91`, runs `38200123560` / `38200123558`, and jobs `114651452123`, `114651495111`, `114651618109`, `114651618294` are predecessor evidence after PR #3 advanced to pre-evidence head `49c566a9c37373f4040c8624db2ef07e2171ce75`. The pending `38200626640` / `38200626642` runs are likewise superseded by this source/evidence change.
- Desktop `frontend/src/production/local-tool-permission-scope.ts@4bfbc19e23c8b4b4eacd049d38ab47e021d732cf` was re-read with its shipping `ProductionRenderer` caller. Its security effect is account/revision fencing: same-scope stability, strictly newer revision on account re-entry, stale revision rejection, reset on account/logout replacement, and permanent disposal.
- The native iOS replacement is the single canonical `MahayanaCoordinator` permission lifecycle, not a second renderer store. `IOSLocalToolApprovalScopeFence` now owns normalized account scope and monotonic revision; persistent and temporary decisions remain in the existing account-scoped settings owner. Scope+revision is checked before persistence, before Host permission dispatch, and after dispatch; RemoteRunner additionally binds Host generation.
- Focused unit coverage exercises account A → B → A re-entry, stale token rejection, reset, post-reset newer revision, and dispose inertness. This moves only the source row to implemented/current-main-reviewed; it does not promote verified, strict completeness, GD-OFF/GD-WEBMCP, payment, release, device, TestFlight/App Store, independent acceptance, or final acceptance.


## Current exact-head rebaseline — 2026-10-12 Avatar Editor and Channels

- PR #3 advanced through `27dd89bb5c6f5e2f6099b31ba36971dd4274589b` and `2eb2fcf6554a9da49e461ddfd262f7a8aaa54dc1` after the local-tool fence repair. Their workflow runs/jobs are predecessor evidence only; no successful or in-progress step is inherited by the resulting exact HEAD.
- Avatar Editor was re-read end-to-end: Desktop production adapter scopes the controller to account + Agent and disposes/rebuilds on scope replacement; the native sheet binds account + Agent + reconnect generation, cancels outstanding work on scope replacement/disappear, fences completion by generation, and preserves the 25 MiB / 1024 px / 256 px / 1x–5x crop contract plus canonical Host avatar mutation.
- Channels was re-read end-to-end: Desktop controller strictly decodes channels, keeps a prior snapshot, fences lifecycle/request generations and owns retry/connect/disconnect/refresh. Native SwiftUI binds its task to Agent + account + reconnect, uses secure credential input and stale-completion fencing, while the Rust Host stores account-scoped metadata apart from encrypted credentials and never exposes credential payloads. Built-in Discord/Slack remain fail-closed coming-soon until actual connector runtime support exists.
- These rows are only current-main-reviewed/implemented. Global Dharma offline/WebMCP artifacts, strict completeness, production signer, StoreKit, signed archive/device/distribution, independent acceptance and final acceptance remain open.


## Current exact-head rebaseline — 2026-10-12 Computer update confirmation

- Strict inventory review of Desktop `frontend/src/recovered/features/computer/update/confirmation.ts` found a real native gap: the iOS account Computer menu directly invoked update without Desktop's running-Agent confirmation, queue-until-idle path, explicit destructive force override, or image-update availability gate.
- The canonical iOS Computer surface now hides update unless the Host reports an image update, strictly reads current individual Agent running state from canonical `bot.list`, fails closed when that state cannot be established, distinguishes ready from busy override, queues `force=false` until all Agents are idle, and uses `force=true` only after explicit destructive confirmation. The queue is surface-lifecycle owned and cancels with the SwiftUI task/disposal rather than creating a second durable state owner.
- Desktop rebuild-banner semantics were also re-read against the native reducer/presentation: update/reset/recover step ordering, migration progression and reconnect checking/network/restarting states remain native equivalents.
- Pre-repair head `28d252ccd74ee70bd75e3a34dceb4dbc25be6957`, runs `38201247586` / `38201247593`, and jobs `114654989020` / `114654989193` / `114654987413` / `114655027482` are predecessor evidence only after this source change. No verified, GD-OFF/GD-WEBMCP, strict-completeness, release, payment or final-acceptance promotion is made by this audit.


## Current exact-head rebaseline — 2026-10-12 conversation-card audit

- Exact-head drift superseded `2e36130fe670abcd8be15123e1eb40b44d18488e` and `bc534f90023c33a9a0f6e91e9e59f2fc8e6079e5`; their runs/jobs are predecessor evidence only. Pre-evidence head `b55bfb679b4c0b2dc2dc99798ff6ff95b26b93e9` is also superseded by this required ledger/evidence commit.
- The Computer busy-update guard and started-untrackable recovery remain in the descendant.
- Strict inventory review advanced through notice, retired permission-request, timeline event automation/registry/resolver/card, attachment-data, auto-review approval actions, cloud-agent provider and connector actions against Desktop current main `371b72f1cd20de03dc4dff9a15ed761054968093`. All are current-main-reviewed/implemented only; none is verified.
- Next strict inventory position is the emoji catalog/picker family. Global Dharma artifact provenance, strict completeness, signer, StoreKit, signed archive/device/distribution, independent acceptance and final acceptance remain open.
