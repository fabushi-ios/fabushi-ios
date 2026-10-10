import Foundation

struct SandErrorDefinition: Equatable, Sendable {
    let name: String
    let domain: String
    let retryable: Bool
    let summary: String
    let payload: [String]
    let seededFrom: String
}

let SAND_ERROR_DEFINITIONS: [String: SandErrorDefinition] = [
    "SAND-E0001": .init(
        name: "unregistered",
        domain: "registry",
        retryable: false,
        summary: "A value that is not a registered SandError reached the emit boundary and was reclassified; the original code and payload never ship.",
        payload: [],
        seededFrom: "sandErrorTags emit-boundary validation in registry.ts"
    ),
    "SAND-E0101": .init(
        name: "gatewayRefused",
        domain: "transport",
        retryable: true,
        summary: "Gateway connection refused: nothing accepted the desktop-to-box connect.",
        payload: ["errno"],
        seededFrom: "classifyGatewayFetchFailure `refused` in gateway-reachability.ts"
    ),
    "SAND-E0102": .init(
        name: "gatewayTimeout",
        domain: "transport",
        retryable: true,
        summary: "Gateway attempt timed out: the deadline fired with no reply.",
        payload: ["errno"],
        seededFrom: "classifyGatewayFetchFailure `timeout` in gateway-reachability.ts"
    ),
    "SAND-E0103": .init(
        name: "gatewayHttp5xx",
        domain: "transport",
        retryable: true,
        summary: "Gateway replied 5xx: the pod proxy routed but the gateway or host errored.",
        payload: ["httpStatus"],
        seededFrom: "outcomeForHttpStatus `http_5xx` in gateway-reachability.ts"
    ),
    "SAND-E0104": .init(
        name: "gatewayDns",
        domain: "transport",
        retryable: true,
        summary: "Gateway host name did not resolve.",
        payload: ["errno"],
        seededFrom: "classifyGatewayFetchFailure `dns` in gateway-reachability.ts"
    ),
    "SAND-E0105": .init(
        name: "gatewayNetwork",
        domain: "transport",
        retryable: true,
        summary: "Gateway transport failed on a live-but-broken route.",
        payload: ["errno"],
        seededFrom: "classifyGatewayFetchFailure `network` in gateway-reachability.ts"
    ),
    "SAND-E0106": .init(
        name: "backendHttpStatus",
        domain: "transport",
        retryable: true,
        summary: "Cursor backend replied with a non-ok HTTP status.",
        payload: ["httpStatus"],
        seededFrom: "classifyDeliveryError BackendStatusError in sand-automation-fire-consumer.ts"
    ),
    "SAND-E0107": .init(
        name: "backendUnreachable",
        domain: "transport",
        retryable: true,
        summary: "Cursor backend was unreachable over the transport.",
        payload: ["errno"],
        seededFrom: "classifyDeliveryError errno branch in sand-automation-fire-consumer.ts"
    ),
    "SAND-E0108": .init(
        name: "backendDeliveryFailed",
        domain: "transport",
        retryable: true,
        summary: "Backend delivery attempt failed outside the status and errno shapes; the caller retries with backoff.",
        payload: [],
        seededFrom: "classifyDeliveryError fallback in sand-automation-fire-consumer.ts"
    ),
    "SAND-E0109": .init(
        name: "logShipTimeout",
        domain: "transport",
        retryable: true,
        summary: "A structured-log submitLogs attempt hit the transport submit deadline; the batch requeues and redelivers.",
        payload: ["batchEntries"],
        seededFrom: "isDeadlineExpiry in structured-log-transport.ts shipLogs"
    ),
    "SAND-E0110": .init(
        name: "streamStalled",
        domain: "transport",
        retryable: true,
        summary: "Established gateway event stream went silent past the stall watchdog and was torn down; the loop reconnects.",
        payload: [],
        seededFrom: "stallWatchdog downReason `stall-timeout` in gateway-client.ts"
    ),
    "SAND-E0111": .init(
        name: "localExecNoProviders",
        domain: "transport",
        retryable: true,
        summary: "Local-machine action refused: no desktop is registered on the reverse local-exec channel.",
        payload: [],
        seededFrom: "requireProvider providers.size === 0 in local-exec-bridge.ts"
    ),
    "SAND-E0112": .init(
        name: "localExecProvidersStale",
        domain: "transport",
        retryable: true,
        summary: "Local-machine action refused: desktops are registered on the reverse local-exec channel but none heartbeated inside the liveness window.",
        payload: [],
        seededFrom: "requireProvider live-resolve miss in local-exec-bridge.ts"
    ),
    "SAND-E0113": .init(
        name: "localExecComputerUnknown",
        domain: "transport",
        retryable: true,
        summary: "Local-machine action refused: the addressed computer matches no registered desktop connection.",
        payload: [],
        seededFrom: "requireProvider unknown-computerId branch in local-exec-bridge.ts"
    ),
    "SAND-E0201": .init(
        name: "boxAccessDenied",
        domain: "auth",
        retryable: false,
        summary: "Backend refused box access for a NO_STORAGE (privacy-mode) account.",
        payload: [],
        seededFrom: "GATEWAY_NO_STORAGE_MESSAGE_MARKER `no_storage` in gateway-reachability.ts"
    ),
    "SAND-E0202": .init(
        name: "gatewayAccessDenied",
        domain: "auth",
        retryable: false,
        summary: "Gateway attempt was refused by an auth/entitlement gate (401/403); a retry cannot fix it.",
        payload: ["httpStatus"],
        seededFrom: "outcomeForHttpStatus `access_denied` in gateway-reachability.ts"
    ),
    "SAND-E0203": .init(
        name: "connectorAuthStartRefused",
        domain: "auth",
        retryable: false,
        summary: "Connector OAuth flow could not start: the connector cannot mint a sign-in link in its current state.",
        payload: ["reason"],
        seededFrom: "authenticateServer refusal branches in mcp-auth-watch-lifecycle.ts"
    ),
    "SAND-E0204": .init(
        name: "connectorAuthStartFailed",
        domain: "auth",
        retryable: true,
        summary: "Connector OAuth flow failed to start: the backend OAuth status probe failed.",
        payload: ["reason"],
        seededFrom: "checkAuthStatus throw in mcp-auth-watch-lifecycle.ts authenticateServer"
    ),
    "SAND-E0205": .init(
        name: "connectorOauthCallbackFailed",
        domain: "auth",
        retryable: true,
        summary: "Connector OAuth callback did not complete: the provider redirected an error or no code, or the backend rejected the exchange.",
        payload: ["reason"],
        seededFrom: "loopback handleRequest failure branches in mcp-oauth-loopback.ts"
    ),
    "SAND-E0206": .init(
        name: "connectorAuthAbandoned",
        domain: "auth",
        retryable: true,
        summary: "Connector OAuth flow expired: the started flow's token never landed within the watch window.",
        payload: [],
        seededFrom: "pollPendingAuthWatch expiry in mcp-auth-watch-lifecycle.ts"
    ),
    "SAND-E0207": .init(
        name: "webauthnNoProvider",
        domain: "auth",
        retryable: true,
        summary: "WebAuthn proxy ceremony refused: no desktop is registered on the reverse WebAuthn channel.",
        payload: [],
        seededFrom: "selectProvider empty registry in webauthn-proxy-bridge.ts"
    ),
    "SAND-E0208": .init(
        name: "webauthnProviderStale",
        domain: "auth",
        retryable: true,
        summary: "WebAuthn proxy ceremony refused: desktops are registered but none heartbeated inside the liveness window.",
        payload: [],
        seededFrom: "selectProvider live-resolve miss in webauthn-proxy-bridge.ts"
    ),
    "SAND-E0209": .init(
        name: "webauthnCeremonyTimedOut",
        domain: "auth",
        retryable: true,
        summary: "WebAuthn proxy ceremony timed out: the desktop never settled it inside the ceremony deadline.",
        payload: [],
        seededFrom: "DeadlineExceededError branch in webauthn-proxy-bridge.ts requestCeremony"
    ),
    "SAND-E0210": .init(
        name: "webauthnConsentDeclined",
        domain: "auth",
        retryable: false,
        summary: "WebAuthn proxy ceremony declined by the user at the desktop consent prompt.",
        payload: [],
        seededFrom: "consent decline branch in node-agent-coordinator/webauthn/provider.ts"
    ),
    "SAND-E0211": .init(
        name: "webauthnSignFailed",
        domain: "auth",
        retryable: true,
        summary: "WebAuthn proxy signing leg failed on the desktop: the signer reported a DOMException instead of an assertion.",
        payload: ["domError", "signErrorClass"],
        seededFrom: "signer error frames in node-agent-coordinator/webauthn/provider.ts"
    ),
    "SAND-E0212": .init(
        name: "webauthnDesktopFailed",
        domain: "auth",
        retryable: true,
        summary: "WebAuthn proxy ceremony failed on the desktop outside the declined and signing shapes, or an older desktop reported no stage attribution.",
        payload: ["domError", "signErrorClass"],
        seededFrom: "unattributed error settlement in webauthn-proxy-bridge.ts"
    ),
    "SAND-E0213": .init(
        name: "webauthnDispatchFailed",
        domain: "auth",
        retryable: true,
        summary: "WebAuthn proxy ceremony dispatch failed: the selected desktop's request stream refused the ceremony write.",
        payload: [],
        seededFrom: "provider.send throw in webauthn-proxy-bridge.ts requestCeremony"
    ),
    "SAND-E0214": .init(
        name: "sessionRefreshHttpStatus",
        domain: "auth",
        retryable: true,
        summary: "Cursor session token refresh got a non-ok HTTP status; the session is kept and a later refresh may succeed.",
        payload: ["httpStatus"],
        seededFrom: "non-ok /oauth/token response in cursor-auth.ts runRefreshAccessToken"
    ),
    "SAND-E0215": .init(
        name: "sessionRefreshNetwork",
        domain: "auth",
        retryable: true,
        summary: "Cursor session token refresh failed on the transport before any backend verdict; the session is kept.",
        payload: ["errno"],
        seededFrom: "fetch throw in cursor-auth.ts runRefreshAccessToken"
    ),
    "SAND-E0216": .init(
        name: "sessionRefreshBadPayload",
        domain: "auth",
        retryable: true,
        summary: "Cursor session token refresh returned an ok status without a usable token payload; the session is kept.",
        payload: [],
        seededFrom: "unreadable body / empty access_token branches in cursor-auth.ts runRefreshAccessToken"
    ),
    "SAND-E0217": .init(
        name: "sessionRefreshRejected",
        domain: "auth",
        retryable: false,
        summary: "Cursor session refresh was terminally rejected (backend shouldLogout verdict or an unparseable token response) with no rotation-race rescue; the user was signed out.",
        payload: [],
        seededFrom: "shouldLogout / parse-failure sign-out in cursor-auth.ts runRefreshAccessToken"
    ),
    "SAND-E0218": .init(
        name: "sessionPolicyRefused",
        domain: "auth",
        retryable: false,
        summary: "Cursor session refresh was refused by the device's MDM sign-in policy; the user was signed out.",
        payload: [],
        seededFrom: "MDM policy verdict in cursor-auth.ts runRefreshAccessToken"
    ),
    "SAND-E0219": .init(
        name: "sessionSecretsUnavailable",
        domain: "auth",
        retryable: false,
        summary: "OS secure storage is unavailable, so the signed-in session's Cursor tokens are held in memory only and will not survive a restart.",
        payload: [],
        seededFrom: "noteSecretsUnavailableSession in cursor-auth.ts storeAuthentication callers"
    ),
    "SAND-E0301": .init(
        name: "bootStageStalled",
        domain: "rebuild",
        retryable: true,
        summary: "Box rebuild stalled before completing a boot stage.",
        payload: ["stage"],
        seededFrom: "SAND_BOX_BOOT_STAGES in host/ports/telemetry.ts"
    ),
    "SAND-E0302": .init(
        name: "hostLifecycleStalled",
        domain: "rebuild",
        retryable: true,
        summary: "In-box host startup stalled inside a lifecycle phase.",
        payload: [],
        seededFrom: "HostLifecycleProgress watchdog in host-lifecycle-progress.ts"
    ),
    "SAND-E0303": .init(
        name: "hostLifecycleFailed",
        domain: "rebuild",
        retryable: true,
        summary: "In-box host startup failed inside a lifecycle phase.",
        payload: [],
        seededFrom: "HostLifecycleProgress.fail in host-lifecycle-progress.ts"
    ),
    "SAND-E0304": .init(
        name: "boxImageCheckTimedOut",
        domain: "rebuild",
        retryable: true,
        summary: "Box image-update check hit its deadline before the backend answered.",
        payload: [],
        seededFrom: "DeadlineExceededError branch in forever-box-service.ts"
    ),
    "SAND-E0305": .init(
        name: "boxImageCheckFailed",
        domain: "rebuild",
        retryable: true,
        summary: "Box image-update check failed before its deadline.",
        payload: [],
        seededFrom: "Image-check catch fallback in forever-box-service.ts"
    ),
    "SAND-E0401": .init(
        name: "providerOverloaded",
        domain: "agent",
        retryable: true,
        summary: "Model provider is overloaded (capacity or rate limit).",
        payload: ["connectCode"],
        seededFrom: "isProviderCapacityError in transient-stream-error.ts"
    ),
    "SAND-E0402": .init(
        name: "firstTokenStall",
        domain: "agent",
        retryable: true,
        summary: "Model provider streamed nothing within the first-token deadline.",
        payload: [],
        seededFrom: "isFirstTokenStallError in transient-stream-error.ts"
    ),
    "SAND-E0403": .init(
        name: "streamReset",
        domain: "agent",
        retryable: true,
        summary: "Turn stream dropped on a transient transport reset.",
        payload: ["connectCode", "errno"],
        seededFrom: "isTransientStreamError in transient-stream-error.ts"
    ),
    "SAND-E0404": .init(
        name: "contextWindowOverflow",
        domain: "agent",
        retryable: false,
        summary: "Turn hit a context-window overflow dead end a retry would repeat.",
        payload: [],
        seededFrom: "isContextOverflowDeadEnd in transient-stream-error.ts"
    ),
    "SAND-E0405": .init(
        name: "backendRejected",
        domain: "agent",
        retryable: false,
        summary: "Backend rejected the turn with a terminal structured error.",
        payload: ["connectCode"],
        seededFrom: "isRetryableProviderError false + findBackendConnectError in turn-runtime.ts"
    ),
    "SAND-E0406": .init(
        name: "turnRetryable",
        domain: "agent",
        retryable: true,
        summary: "Turn failed retryably outside every more specific shape.",
        payload: ["connectCode"],
        seededFrom: "isRetryableProviderError true in turn-runtime.ts"
    ),
    "SAND-E0407": .init(
        name: "agentUnclassified",
        domain: "agent",
        retryable: false,
        summary: "Agent-path operation failed outside every classified shape.",
        payload: [],
        seededFrom: "classifyAgentError fallback in turn-runtime.ts"
    ),
    "SAND-E0408": .init(
        name: "backendCapacityDeferred",
        domain: "agent",
        retryable: true,
        summary: "Backend deferred the turn at capacity with a server-paced retry-after; the runner sleeps the paced delay and retries.",
        payload: ["connectCode", "retryAfterMs"],
        seededFrom: "isProviderCapacityError + serverRetryAfterMsFromError in classifyAgentError (turn-runtime.ts)"
    ),
    "SAND-E0409": .init(
        name: "memorySynthesisInvalidOutput",
        domain: "agent",
        retryable: false,
        summary: "Memory synthesis produced an invalid proposal: unparseable output, an unknown evidence citation, or a change the memory state rejected.",
        payload: [],
        seededFrom: "MemorySynthesisAttemptError invalid-output + applySynthesis invalid in memory-synthesis-service.ts"
    ),
    "SAND-E0410": .init(
        name: "memorySynthesisRejected",
        domain: "agent",
        retryable: false,
        summary: "Memory synthesis verification did not approve the proposed changes.",
        payload: [],
        seededFrom: "MemorySynthesisAttemptError rejected in memory-synthesis-service.ts"
    ),
    "SAND-E0411": .init(
        name: "memorySynthesisStale",
        domain: "agent",
        retryable: true,
        summary: "Memory files changed under a synthesis run; the pending evidence re-queues against the fresh state.",
        payload: [],
        seededFrom: "applySynthesis stale in memory-synthesis-service.ts"
    ),
    "SAND-E0412": .init(
        name: "memorySynthesisEvidenceDropped",
        domain: "agent",
        retryable: false,
        summary: "Pending synthesis evidence was shed by a capacity cap before any run consumed it.",
        payload: [],
        seededFrom: "recordTurn overflow shedding in memory-synthesis-service.ts"
    ),
    "SAND-E0413": .init(
        name: "memorySynthesisFailed",
        domain: "agent",
        retryable: true,
        summary: "Memory synthesis run failed outside the classified shapes (inference transport or deadline).",
        payload: [],
        seededFrom: "runAgent catch fallback in memory-synthesis-service.ts"
    ),
    "SAND-E0414": .init(
        name: "conversationTooLarge",
        domain: "agent",
        retryable: false,
        summary: "Turn refused at the conversation-size hard cap: the live conversation tree is over the limit and GC could not shrink it, so the refusal repeats until GC succeeds or the user starts a new conversation.",
        payload: [],
        seededFrom: "SandConversationTooLargeError turn gate in conversation-size-limits.ts"
    ),
    "SAND-E0501": .init(
        name: "updateFeedFetchFailed",
        domain: "update",
        retryable: true,
        summary: "Update-feed fetch threw before any HTTP status arrived (network, DNS, or TLS transport).",
        payload: [],
        seededFrom: "classifyUpdateCheckError `fetch_throw` in update-telemetry.ts"
    ),
    "SAND-E0502": .init(
        name: "updateFeedHttpStatus",
        domain: "update",
        retryable: true,
        summary: "Update server replied with a non-ok HTTP status.",
        payload: ["httpStatus"],
        seededFrom: "classifyUpdateCheckError `http_non_ok` in update-telemetry.ts"
    ),
    "SAND-E0503": .init(
        name: "updateFeedMalformed",
        domain: "update",
        retryable: false,
        summary: "Update server's response body failed the feed schema.",
        payload: [],
        seededFrom: "classifyUpdateCheckError `malformed_body` in update-telemetry.ts"
    ),
    "SAND-E0504": .init(
        name: "updateCheckAborted",
        domain: "update",
        retryable: true,
        summary: "Update check aborted before a result (dispose, track change, or gate change); the next scheduled check retries.",
        payload: [],
        seededFrom: "checkForUpdates abort closure in sand-update-service.ts"
    ),
    "SAND-E0505": .init(
        name: "updateDisabledByEnv",
        domain: "update",
        retryable: false,
        summary: "Updater is inert: updates disabled by environment override.",
        payload: [],
        seededFrom: "computeUpdateDisabledReason `disabled-by-env` in update-gate.ts"
    ),
    "SAND-E0506": .init(
        name: "updateDisabledLabBuild",
        domain: "update",
        retryable: false,
        summary: "Updater is inert: a Sand Lab one-off build outside every track never self-updates.",
        payload: [],
        seededFrom: "computeUpdateDisabledReason `lab-build` in update-gate.ts"
    ),
    "SAND-E0507": .init(
        name: "updateDisabledNotPackaged",
        domain: "update",
        retryable: false,
        summary: "Updater is inert: an unpackaged development run has nothing to update.",
        payload: [],
        seededFrom: "computeUpdateDisabledReason `not-packaged` in update-gate.ts"
    ),
    "SAND-E0508": .init(
        name: "updateDisabledUnsupportedPlatform",
        domain: "update",
        retryable: false,
        summary: "Updater is inert: the platform has no supported update path.",
        payload: [],
        seededFrom: "computeUpdateDisabledReason `unsupported-platform` in update-gate.ts"
    ),
    "SAND-E0509": .init(
        name: "updateStagedNotAdopted",
        domain: "update",
        retryable: true,
        summary: "A Squirrel-staged desktop build was not the one running on the next launch: ShipIt did not swap it in (an apply failure, or an unclean termination that skipped the swap). The updater re-stages on its next check.",
        payload: [],
        seededFrom: "decideUpdateApplySettlement staged/squirrel branch in apply-marker.ts"
    ),
    "SAND-E0510": .init(
        name: "updateApplyIncomplete",
        domain: "update",
        retryable: true,
        summary: "An explicit restart-to-update was requested but the next launch still ran the old build: the handoff (ShipIt swap, or the parked Windows installer) never completed.",
        payload: [],
        seededFrom: "decideUpdateApplySettlement requested/spawned branches in apply-marker.ts"
    ),
    "SAND-E0511": .init(
        name: "updateInstallerSpawnFailed",
        domain: "update",
        retryable: true,
        summary: "The parked Windows installer failed to spawn as the app quit for restart-to-update; the user relaunches into the old build.",
        payload: ["errno"],
        seededFrom: "installOnQuit spawn-error callback recorded by applyStagedOnQuit"
    ),
    "SAND-E0512": .init(
        name: "updateAutoRelaunchFailed",
        domain: "update",
        retryable: false,
        summary: "The update applied but the post-install auto-relaunch never brought Sand back; the confirm arrived late, from a manual launch (SAND-1269).",
        payload: [],
        seededFrom: "decideUpdateApplySettlement confirmed_late branch in apply-marker.ts"
    ),
    "SAND-E0601": .init(
        name: "desktopStartupFailed",
        domain: "desktop",
        retryable: false,
        summary: "Desktop bootstrap threw before the app finished starting; the payload names the phase that was in progress.",
        payload: ["phase"],
        seededFrom: "app.whenReady bootstrap catch in electron-main/main.cts"
    ),
    "SAND-E0602": .init(
        name: "desktopStartupStuck",
        domain: "desktop",
        retryable: true,
        summary: "Desktop bootstrap sat in one phase past the stuck watchdog without finishing, failing, or quitting.",
        payload: ["phase"],
        seededFrom: "desktop-startup-telemetry.ts stuck watchdog"
    ),
    "SAND-E0603": .init(
        name: "desktopUncaughtException",
        domain: "desktop",
        retryable: false,
        summary: "Desktop main process uncaught exception, kept alive by the process crash guard.",
        payload: [],
        seededFrom: "installProcessCrashGuards reporter in electron-main/main.cts"
    ),
    "SAND-E0604": .init(
        name: "desktopUnhandledRejection",
        domain: "desktop",
        retryable: false,
        summary: "Desktop main process unhandled promise rejection, kept alive by the process crash guard.",
        payload: [],
        seededFrom: "installProcessCrashGuards reporter in electron-main/main.cts"
    ),
    "SAND-E0605": .init(
        name: "desktopChildProcessGone",
        domain: "desktop",
        retryable: true,
        summary: "An Electron child process (renderer, GPU, or utility) died abnormally under the desktop main process.",
        payload: ["process", "reason"],
        seededFrom: "render-process-gone / child-process-gone wiring in renderer-lifecycle-telemetry.ts"
    ),
    "SAND-E0606": .init(
        name: "desktopCoordinatorHandoffFailed",
        domain: "desktop",
        retryable: true,
        summary: "A coordinator port handoff leg threw while transferring a data port; the next request or relaunch re-serves.",
        payload: ["leg"],
        seededFrom: "coordinator port sinks in electron-main/main.cts"
    ),
    "SAND-E0607": .init(
        name: "desktopCoordinatorExitTimeout",
        domain: "desktop",
        retryable: true,
        summary: "A departing coordinator did not confirm exit within the account-handoff deadline; the handoff blocked on a zombie.",
        payload: ["timeoutMs"],
        seededFrom: "stop() deadline in coordinator-account-runtime.ts"
    ),
    "SAND-E0608": .init(
        name: "desktopLocalExecSpawnFailed",
        domain: "desktop",
        retryable: true,
        summary: "The detached local-exec daemon failed to spawn from the desktop bundle.",
        payload: ["errno"],
        seededFrom: "spawnLocalExecDaemon error observer in local-exec-native.ts"
    ),
    "SAND-E0609": .init(
        name: "desktopVncLivenessStall",
        domain: "desktop",
        retryable: true,
        summary: "The interactive box-desktop viewer forwarded repeated key/click input while the framebuffer drew nothing and the wire stayed silent for the rolling window: the connected-but-frozen fingerprint the RFB lifecycle cannot see.",
        payload: [],
        seededFrom: "createVncLivenessDetector emission in box-vnc-liveness.ts"
    ),
    "SAND-E0610": .init(
        name: "desktopUncleanExit",
        domain: "desktop",
        retryable: false,
        summary: "A prior desktop session's alive marker was never settled by a clean quit: the main process died out from under the app (native crash, OS OOM kill, force-quit, or power loss), observed and reported at the next boot.",
        payload: [],
        seededFrom: "decideUncleanExitSettlement in desktop-unclean-exit-telemetry.ts"
    ),
    "SAND-E0700": .init(
        name: "clientSliceCorrupt",
        domain: "storage",
        retryable: false,
        summary: "A persisted Client slice failed its envelope parse or its owner's value codec at load; the owner resets the slice.",
        payload: [],
        seededFrom: "parseEnvelope corrupt + noteValueRejected in client/persistence.ts"
    ),
    "SAND-E0701": .init(
        name: "clientSliceIoError",
        domain: "storage",
        retryable: true,
        summary: "A Client persistence port operation (read, write, remove, or list) threw.",
        payload: ["errno"],
        seededFrom: "PersistenceRegistry port-operation catch in client/persistence.ts"
    ),
    "SAND-E0702": .init(
        name: "clientSliceQuotaExceeded",
        domain: "storage",
        retryable: false,
        summary: "A Client slice write was refused for storage quota; retrying cannot free space.",
        payload: [],
        seededFrom: "isQuotaFailure branch of reportThrown in client/persistence.ts"
    ),
    "SAND-E0703": .init(
        name: "clientQueuedFlushNonceMismatch",
        domain: "storage",
        retryable: false,
        summary: "A queued send reached the Host with a nonce whose accepted digest did not match.",
        payload: [],
        seededFrom: "NONCE_DIGEST_MISMATCH in client/send/send-journal.ts queuedFlushFailureCause"
    ),
    "SAND-E0704": .init(
        name: "clientQueuedFlushCapabilityUnavailable",
        domain: "storage",
        retryable: false,
        summary: "A queued send could not flush because the Host lacks the required send capability.",
        payload: [],
        seededFrom: "SAND_SOURCE_CAPABILITY_UNAVAILABLE in client/send/send-journal.ts queuedFlushFailureCause"
    ),
    "SAND-E0705": .init(
        name: "clientQueuedFlushHostRejected",
        domain: "storage",
        retryable: false,
        summary: "Acceptance-status resolution proved that the Host rejected a queued send.",
        payload: [],
        seededFrom: "rejected acceptance status in client/send/send-journal.ts resolveRecord"
    ),
    "SAND-E0706": .init(
        name: "clientQueuedSendSuperseded",
        domain: "storage",
        retryable: false,
        summary: "An explicit recovery action retired an offline-queued send without delivery proof.",
        payload: [],
        seededFrom: "retireCanceled in client/send/send-journal.ts"
    ),
    "SAND-E0707": .init(
        name: "clientQueuedSendAckExpired",
        domain: "storage",
        retryable: false,
        summary: "A flushed queued send received no authoritative echo before its online ACK deadline.",
        payload: [],
        seededFrom: "markAckTimedOut in client/send/send-journal.ts"
    ),
    "SAND-E0720": .init(
        name: "journalAppendFailed",
        domain: "storage",
        retryable: true,
        summary: "Committing the prepared transcript WAL into the canonical journal failed; the checkpoint stays durable and the turn path surfaces the error.",
        payload: ["errno"],
        seededFrom: "commitCheckpoint in transcript-mirror.ts"
    ),
    "SAND-E0721": .init(
        name: "journalCheckpointFailed",
        domain: "storage",
        retryable: true,
        summary: "Deriving or writing the pending transcript WAL checkpoint failed before anything was committed.",
        payload: ["errno"],
        seededFrom: "prepareCheckpoint in transcript-mirror.ts"
    ),
    "SAND-E0722": .init(
        name: "journalCorruptTail",
        domain: "storage",
        retryable: false,
        summary: "The transcript journal was missing, truncated, or torn at open for a conversation with durable history; it was discarded and rebuilt from the checkpoint.",
        payload: ["tail", "errno"],
        seededFrom: "initialize rebuild branch in transcript-mirror.ts"
    ),
    "SAND-E0723": .init(
        name: "journalReplayFailed",
        domain: "storage",
        retryable: true,
        summary: "Reconciling the pending transcript WAL against the durable checkpoint at recover failed; the journal cannot advance until the mismatch resolves.",
        payload: ["errno"],
        seededFrom: "recover in transcript-mirror.ts"
    ),
    "SAND-E0724": .init(
        name: "journalRebuildFailed",
        domain: "storage",
        retryable: true,
        summary: "Re-deriving the transcript journal from the durable checkpoint failed after a corrupt or absent journal was discarded; the next recover retries the rebuild.",
        payload: ["tail", "errno"],
        seededFrom: "initialize rebuild branch in transcript-mirror.ts"
    )
]

enum SandErrorPayloadValue: Equatable, Sendable {
    case string(String)
    case int(Int)
    case double(Double)
    case bool(Bool)

    fileprivate var telemetryString: String? {
        switch self {
        case .string(let value):
            guard value.count <= 64,
                  !value.isEmpty,
                  value.allSatisfy({
                      $0.isASCII && ($0.isLetter || $0.isNumber || "._|:-".contains($0))
                  }) else { return nil }
            return value
        case .int(let value):
            return String(value)
        case .double(let value):
            return value.isFinite ? String(value) : nil
        case .bool(let value):
            return String(value)
        }
    }
}

struct SandErrorValue: Equatable, Sendable {
    let code: String
    var payload: [String: SandErrorPayloadValue] = [:]
}

let UNREGISTERED_CODE = "SAND-E0001"

func isRegisteredCode(_ code: String?) -> Bool {
    guard let code else { return false }
    return SAND_ERROR_DEFINITIONS[code] != nil
}

func sandErrorWireCode(_ error: SandErrorValue) -> String {
    isRegisteredCode(error.code) ? error.code : UNREGISTERED_CODE
}

private func sandErrorTagName(_ field: String) -> String {
    var output = ""
    for character in field {
        if character.isUppercase {
            output.append("_")
            output.append(contentsOf: character.lowercased())
        } else {
            output.append(character)
        }
    }
    return output
}

func sandErrorTags(_ error: SandErrorValue) -> [String: String] {
    let code = sandErrorWireCode(error)
    guard let definition = SAND_ERROR_DEFINITIONS[code] else { return [:] }
    var tags = [
        "error_code": code,
        "error_domain": definition.domain,
        "error_retryable": String(definition.retryable),
    ]
    guard code == error.code else { return tags }
    let declared = Set(definition.payload)
    for (field, value) in error.payload where declared.contains(field) {
        if let rendered = value.telemetryString {
            tags[sandErrorTagName(field)] = rendered
        }
    }
    return tags
}

enum SandError {
    static func unregistered(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0001", payload: payload)
    }

    static func gatewayRefused(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0101", payload: payload)
    }

    static func gatewayTimeout(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0102", payload: payload)
    }

    static func gatewayHttp5xx(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0103", payload: payload)
    }

    static func gatewayDns(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0104", payload: payload)
    }

    static func gatewayNetwork(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0105", payload: payload)
    }

    static func backendHttpStatus(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0106", payload: payload)
    }

    static func backendUnreachable(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0107", payload: payload)
    }

    static func backendDeliveryFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0108", payload: payload)
    }

    static func logShipTimeout(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0109", payload: payload)
    }

    static func streamStalled(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0110", payload: payload)
    }

    static func localExecNoProviders(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0111", payload: payload)
    }

    static func localExecProvidersStale(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0112", payload: payload)
    }

    static func localExecComputerUnknown(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0113", payload: payload)
    }

    static func boxAccessDenied(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0201", payload: payload)
    }

    static func gatewayAccessDenied(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0202", payload: payload)
    }

    static func connectorAuthStartRefused(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0203", payload: payload)
    }

    static func connectorAuthStartFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0204", payload: payload)
    }

    static func connectorOauthCallbackFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0205", payload: payload)
    }

    static func connectorAuthAbandoned(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0206", payload: payload)
    }

    static func webauthnNoProvider(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0207", payload: payload)
    }

    static func webauthnProviderStale(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0208", payload: payload)
    }

    static func webauthnCeremonyTimedOut(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0209", payload: payload)
    }

    static func webauthnConsentDeclined(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0210", payload: payload)
    }

    static func webauthnSignFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0211", payload: payload)
    }

    static func webauthnDesktopFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0212", payload: payload)
    }

    static func webauthnDispatchFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0213", payload: payload)
    }

    static func sessionRefreshHttpStatus(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0214", payload: payload)
    }

    static func sessionRefreshNetwork(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0215", payload: payload)
    }

    static func sessionRefreshBadPayload(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0216", payload: payload)
    }

    static func sessionRefreshRejected(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0217", payload: payload)
    }

    static func sessionPolicyRefused(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0218", payload: payload)
    }

    static func sessionSecretsUnavailable(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0219", payload: payload)
    }

    static func bootStageStalled(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0301", payload: payload)
    }

    static func hostLifecycleStalled(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0302", payload: payload)
    }

    static func hostLifecycleFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0303", payload: payload)
    }

    static func boxImageCheckTimedOut(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0304", payload: payload)
    }

    static func boxImageCheckFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0305", payload: payload)
    }

    static func providerOverloaded(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0401", payload: payload)
    }

    static func firstTokenStall(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0402", payload: payload)
    }

    static func streamReset(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0403", payload: payload)
    }

    static func contextWindowOverflow(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0404", payload: payload)
    }

    static func backendRejected(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0405", payload: payload)
    }

    static func turnRetryable(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0406", payload: payload)
    }

    static func agentUnclassified(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0407", payload: payload)
    }

    static func backendCapacityDeferred(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0408", payload: payload)
    }

    static func memorySynthesisInvalidOutput(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0409", payload: payload)
    }

    static func memorySynthesisRejected(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0410", payload: payload)
    }

    static func memorySynthesisStale(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0411", payload: payload)
    }

    static func memorySynthesisEvidenceDropped(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0412", payload: payload)
    }

    static func memorySynthesisFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0413", payload: payload)
    }

    static func conversationTooLarge(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0414", payload: payload)
    }

    static func updateFeedFetchFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0501", payload: payload)
    }

    static func updateFeedHttpStatus(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0502", payload: payload)
    }

    static func updateFeedMalformed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0503", payload: payload)
    }

    static func updateCheckAborted(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0504", payload: payload)
    }

    static func updateDisabledByEnv(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0505", payload: payload)
    }

    static func updateDisabledLabBuild(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0506", payload: payload)
    }

    static func updateDisabledNotPackaged(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0507", payload: payload)
    }

    static func updateDisabledUnsupportedPlatform(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0508", payload: payload)
    }

    static func updateStagedNotAdopted(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0509", payload: payload)
    }

    static func updateApplyIncomplete(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0510", payload: payload)
    }

    static func updateInstallerSpawnFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0511", payload: payload)
    }

    static func updateAutoRelaunchFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0512", payload: payload)
    }

    static func desktopStartupFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0601", payload: payload)
    }

    static func desktopStartupStuck(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0602", payload: payload)
    }

    static func desktopUncaughtException(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0603", payload: payload)
    }

    static func desktopUnhandledRejection(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0604", payload: payload)
    }

    static func desktopChildProcessGone(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0605", payload: payload)
    }

    static func desktopCoordinatorHandoffFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0606", payload: payload)
    }

    static func desktopCoordinatorExitTimeout(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0607", payload: payload)
    }

    static func desktopLocalExecSpawnFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0608", payload: payload)
    }

    static func desktopVncLivenessStall(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0609", payload: payload)
    }

    static func desktopUncleanExit(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0610", payload: payload)
    }

    static func clientSliceCorrupt(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0700", payload: payload)
    }

    static func clientSliceIoError(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0701", payload: payload)
    }

    static func clientSliceQuotaExceeded(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0702", payload: payload)
    }

    static func clientQueuedFlushNonceMismatch(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0703", payload: payload)
    }

    static func clientQueuedFlushCapabilityUnavailable(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0704", payload: payload)
    }

    static func clientQueuedFlushHostRejected(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0705", payload: payload)
    }

    static func clientQueuedSendSuperseded(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0706", payload: payload)
    }

    static func clientQueuedSendAckExpired(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0707", payload: payload)
    }

    static func journalAppendFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0720", payload: payload)
    }

    static func journalCheckpointFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0721", payload: payload)
    }

    static func journalCorruptTail(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0722", payload: payload)
    }

    static func journalReplayFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0723", payload: payload)
    }

    static func journalRebuildFailed(_ payload: [String: SandErrorPayloadValue] = [:]) -> SandErrorValue {
        .init(code: "SAND-E0724", payload: payload)
    }
}
