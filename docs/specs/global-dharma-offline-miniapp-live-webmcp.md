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
- iOS main: `f098639ab178a839690b9dece60fa0d11c5d2e25`
- iOS PR #3 pre-document implementation HEAD: `5feaa90ae769a114d0194c9114601aeabe3a8d5b` (final evidence must use the resulting newer exact HEAD)
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
| Marketplace package install/source | MarketplaceModel + Host `feature.plugin.*` | platform | install/update network may be needed | no |
| Local package render/storage | MiniAppWebMcpSurface + WKWebView | free platform | yes after install | no |
| Global Dharma `home/status/logs/validate_config/start/stop/loop/chat/send/deploy_latest` | remote `/api/mcp/apps/global-dharma` via GlobalDharmaMiniAppBridge | remote service | no | only when returned protected Host request requires it; iOS preserves this route only when no active local-page tool fence exists |
| Local WebMCP session identity/permission | MiniAppWebMcpSurface | free platform | yes | no |
| Local prayer wheel start | Global Dharma + trusted Host boundary | protected | no by default because fresh online entitlement required | yes |
| Purchase/restore | GlobalDharmaCommerceModel + StoreKit + service | paid service | no | yes |
| AI private draft/sign/install | MarketplaceModel + LocalMiniAppSigningAuthority + Host localDraft/installLocalFabapp | platform | installed app yes; generation depends on model route | per requested capability only |

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

## 16. Spec compliance record

| Requirement / AC | Status | Evidence / reason |
| --- | --- | --- |
| GD-ARCH-001 | implemented; exact-head CI pending | Shipping entry uses generic `MiniAppWebMcpSurface`; the legacy `GlobalDharmaMiniAppView` parallel `loadHTMLString` runtime has been removed from PR #3. |
| GD-PKG-001..004 | implementation closed; production signer rollout blocked | Global Dharma 1.1.0 source is owned by `bhrumom/fabushi-marketplace` and uses the strict catalog `.fabapp` manifest/finalizer contract. The iOS production install path now preserves `platform=ios`, selects the iOS ZIP-container `.fabapp`, requires release `fabappTrust`, verifies the outer digest/size and Ed25519 package manifest through `mahayana-plugin-runtime::verify_fabapp_bytes`, reconciles app/version/digest/requested-capabilities, and only then journals/activates the install. Backend PR #28 keeps this release fail-closed until immutable artifact URL/source SHA/digest/size plus managed signer key ID/public key are all present. Marketplace source-contract CI is already green; production finalization still requires the managed signing Worker configuration. |
| GD-OFF-01..03 | implementation present; journey evidence pending | The 1.1.0 package uses only local `index.html/style.css/app.js`, CSP `connect-src 'none'`, localStorage-backed revision state, and no startup entitlement request. Cold-start/restart/network-off UI evidence must still complete on the current exact iOS HEAD. |
| GD-WEBMCP-01..07 | implementation present; latest exact-head UI/security evidence pending | Active page tools register through `registerLocalTool`; native Host invokes the current page via `invokeLocalFromHost` fenced by account/app/version/digest/instance/nonce/generation. Page tool schemas must match reviewed catalog schemas, Host validates arguments before execution, and page annotations cannot weaken Host approval class. Host-originated page calls are bounded to 15 seconds with abort-on-pagehide; WebKit process/navigation failure tears down the active session. With an active Global Dharma page, remote MCP fallback is explicitly rejected; without an active page, the pre-existing governed remote-service route remains available and is never cited as local WebMCP evidence. The registry records bounded ordered invocation traces with call ID, account/app/package/version/instance, privacy-safe nonce identifier, generation, tool, permission decision, state revision and outcome; the protected journey covers denial-before-approval and same-page bidirectional revision changes. |
| GD-PAY-01 | implementation present; protected journey pending | `local.prayer-wheel.start` is the only current protected local capability; `callActiveWebMcpTool` performs fresh Host-side entitlement refresh only for that protected tool. Free tools bypass entitlement preflight. |
| GD-PAY-02 | implementation aligned; production purchase evidence blocked | Canonical server main defines both CNY 30 / 30-day monthly (`local-prayer-wheel.monthly`) and CNY 1080 lifetime (`local-prayer-wheel.lifetime`) offers for `local.prayer-wheel.start`. iOS now parses and validates both server offers, exposes both choices, refuses client-side price/kind/SKU drift, requires the server `activeRails` projection before StoreKit, and restores either SKU before re-reading canonical entitlement. Real Apple purchase/restore still requires provider/App Store configuration. |
| GD-PAY-03 / GD-PAY-02 acceptance | blocked | Real StoreKit/device/payment acceptance depends on external Apple/product/account configuration; CI test rail is not production evidence. |
| GD-MKT-01 | implementation closed; production artifact rollout blocked | Global Dharma discovery and release selection preserve native iOS instead of collapsing to generic `mobile`; the catalog emits the 1.1.0 package only when immutable release metadata and signer trust are complete, otherwise iOS release is unavailable rather than falling back to legacy 1.0.0/Hosted MCP. The normal Host `feature.plugin.install` path consumes the exact `releaseManifest` and its `fabappTrust`. Production-signed 1.1.0 artifact/catalog rollout remains blocked by managed signer configuration. |
| FPM-IOS-01..04 | pending exact-head regression | Existing Marketplace/Connectors/Skills/private Mini App production owners remain in place; current-head regression and any remaining product gaps still require Actions evidence. |
| GD-REL-01 | blocked | Real signed-device/TestFlight/App Review acceptance requires configured Apple distribution infrastructure and policy evidence. |
