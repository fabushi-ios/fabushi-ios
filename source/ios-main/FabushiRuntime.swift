import Foundation
import Observation
import SwiftUI

private let IOS_CLOUD_AGENT_POLL_INTERVAL_NS: UInt64 = 10_000_000_000
private let IOS_CLOUD_AGENT_MAX_WAIT_MS: Int64 = 5 * 60 * 60 * 1_000
private let IOS_CLOUD_AGENT_RESTART_GRACE_MS: Int64 = 3 * 60 * 1_000
let IOS_AGENT_STORE_RECONCILE_INTERVAL_NS: UInt64 = 120_000_000_000
let IOS_AGENT_STORE_ERROR_BACKOFF_NS: UInt64 = 30_000_000_000

@inline(__always)
func iosAgentStoreReconcileResultIsCurrent(
    expectedGeneration: UInt64,
    currentGeneration: UInt64,
    loggedIn: Bool,
    taskCancelled: Bool
) -> Bool {
    expectedGeneration == currentGeneration && loggedIn && !taskCancelled
}

private struct IOSPendingCloudAgentWake: Equatable {
    let agentId: String
    let workId: String
    let operationId: String
    let title: String
    let startedAtMs: Int64

    init?(_ raw: Any) {
        guard let object = raw as? [String: Any],
              let agentId = object["agentId"] as? String,
              !agentId.isEmpty,
              let workId = object["workId"] as? String,
              !workId.isEmpty,
              let operationId = object["operationId"] as? String,
              !operationId.isEmpty,
              let title = object["title"] as? String
        else { return nil }
        self.agentId = agentId
        self.workId = workId
        self.operationId = operationId
        self.title = title
        self.startedAtMs = (object["startedAtMs"] as? NSNumber)?.int64Value ?? 0
    }

    var identity: String { "\(agentId)\u{1f}\(workId)" }
}

@MainActor
private final class IOSCloudAgentWakeWatcher {
    private let coordinator: MahayanaCoordinator
    private var scanTask: Task<Void, Never>?
    private var watches: [String: Task<Void, Never>] = [:]

    init(coordinator: MahayanaCoordinator) {
        self.coordinator = coordinator
    }

    func start() {
        guard scanTask == nil else { return }
        scanTask = Task { [weak self] in
            guard let self else { return }
            await self.refresh(waitForRestart: true)
            while !Task.isCancelled {
                do {
                    try await Task.sleep(nanoseconds: IOS_CLOUD_AGENT_POLL_INTERVAL_NS)
                } catch {
                    return
                }
                await self.refresh(waitForRestart: false)
            }
        }
    }

    func stop() {
        scanTask?.cancel()
        scanTask = nil
        for task in watches.values {
            task.cancel()
        }
        watches.removeAll()
    }

    func refreshNow() async {
        await refresh(waitForRestart: false)
    }

    private func refresh(waitForRestart: Bool) async {
        let raw: Any
        do {
            raw = try await coordinator.request(method: "native.cloudAgent.pendingWakes").value
        } catch {
            return
        }
        guard let values = raw as? [Any] else { return }
        let pending = values.compactMap(IOSPendingCloudAgentWake.init)
        let liveKeys = Set(pending.map(\.identity))
        let staleKeys = watches.keys.filter { !liveKeys.contains($0) }
        for key in staleKeys {
            watches.removeValue(forKey: key)?.cancel()
        }
        for wake in pending where watches[wake.identity] == nil {
            arm(wake, waitForRestart: waitForRestart)
        }
    }

    private func arm(_ wake: IOSPendingCloudAgentWake, waitForRestart: Bool) {
        let key = wake.identity
        watches[key] = Task { [weak self] in
            guard let self else { return }
            await self.watch(wake, waitForRestart: waitForRestart)
            self.watches.removeValue(forKey: key)
        }
    }

    private func watch(_ wake: IOSPendingCloudAgentWake, waitForRestart: Bool) async {
        let startedAt = nowMs()
        let deadline = startedAt.saturatingAdding(IOS_CLOUD_AGENT_MAX_WAIT_MS)
        let restartDeadline = startedAt.saturatingAdding(IOS_CLOUD_AGENT_RESTART_GRACE_MS)
        var awaitingRestart = waitForRestart

        while !Task.isCancelled {
            do {
                try await Task.sleep(nanoseconds: IOS_CLOUD_AGENT_POLL_INTERVAL_NS)
            } catch {
                return
            }
            let now = nowMs()
            if now >= deadline {
                let text = "The Cursor agent (\(wake.workId)) is still running after 300 minutes. It keeps running remotely; check the cloud agents dashboard for the result."
                if await settle(wake, status: "error", result: text) { return }
                continue
            }

            let info: IOSCloudAgentComposerInfo
            do {
                info = try await coordinator.cloudAgentInfo(bcId: wake.workId)
            } catch {
                continue
            }

            if awaitingRestart {
                if info.isActive || now >= restartDeadline {
                    awaitingRestart = false
                } else {
                    continue
                }
            }

            switch info.status {
            case 1, 4:
                continue
            case 2:
                let summary = info.summary?.trimmingCharacters(in: .whitespacesAndNewlines)
                let result = (summary?.isEmpty == false) ? summary! : "The Cursor agent finished."
                if await settle(wake, status: "completed", result: await augmentWithTranscriptDump(wake, result: result)) { return }
            case 3, 5:
                let errorText = info.permanentError?.trimmingCharacters(in: .whitespacesAndNewlines)
                    ?? info.summary?.trimmingCharacters(in: .whitespacesAndNewlines)
                let result = (errorText?.isEmpty == false)
                    ? errorText!
                    : "The Cursor agent (\(wake.workId)) ended before finishing."
                if await settle(wake, status: "error", result: await augmentWithTranscriptDump(wake, result: result)) { return }
            default:
                // Desktop normalizes unknown numeric statuses to Unspecified and keeps polling.
                continue
            }
        }
    }

    private func augmentWithTranscriptDump(
        _ wake: IOSPendingCloudAgentWake,
        result: String
    ) async -> String {
        do {
            guard let dump = try await coordinator.cloudAgentTranscriptDump(bcId: wake.workId),
                  let path = dump["path"] as? String,
                  let sizeBytes = dump["sizeBytes"] as? Int,
                  let lineCount = dump["lineCount"] as? Int
            else {
                return result
            }
            return result + "\n\nFull transcript dumped to \(path) (\(sizeBytes) bytes, \(lineCount) message lines). Read it with your shell tools; tail -n 1 \(path) gives the final assistant report."
        } catch {
            // Desktop semantics are best-effort: a transcript dump failure must
            // never prevent the durable completion wake from settling.
            return result
        }
    }

    private func settle(
        _ wake: IOSPendingCloudAgentWake,
        status: String,
        result: String
    ) async -> Bool {
        do {
            let reply = try await coordinator.request(
                method: "native.cloudAgent.settleWake",
                params: [
                    "agentId": wake.agentId,
                    "workId": wake.workId,
                    "status": status,
                    "result": result,
                ]
            ).value
            guard let object = reply as? [String: Any] else { return false }
            return (object["settled"] as? Bool) == true
        } catch {
            return false
        }
    }

    private func nowMs() -> Int64 {
        Int64(Date().timeIntervalSince1970 * 1_000)
    }
}

private extension Int64 {
    func saturatingAdding(_ other: Int64) -> Int64 {
        let (value, overflow) = addingReportingOverflow(other)
        return overflow ? Int64.max : value
    }
}

/// Production composition root. FabushiApp only owns Scene/App lifecycle and
/// forwards events here.
@MainActor
@Observable
final class FabushiRuntime {
    let main: IOSMainRuntime
    let bridge: IOSPreloadBridge
    let appAgentSurface: FabushiAppAgentSurface
    let remoteDeviceGateway: FabushiRemoteDeviceGateway
    let marketplace: MarketplaceModel
    let messaging: MessagingModel
    let uiPreferencesStore: MobileUiPreferencesStore
    let humanCallMediaPort: HumanCallMediaPort
    let authCallbackRegistration: IOSAuthCallbackRegistration
    private(set) var reconnectGeneration = 0
    private(set) var appVersionPolicyState: IOSAppVersionPolicyLoadState = .idle

    @ObservationIgnored private var deepLinkController: IOSDeepLinkController?
    @ObservationIgnored private let cloudAgentWakeWatcher: IOSCloudAgentWakeWatcher
    @ObservationIgnored private var wasBackgrounded = false
    @ObservationIgnored private var resumeTask: Task<Void, Never>?
    @ObservationIgnored private var connectionRetryTask: Task<Void, Never>?
    @ObservationIgnored private var appVersionPolicyTask: Task<Void, Never>?
    @ObservationIgnored private var agentStoreReconcileTask: Task<Void, Never>?
    @ObservationIgnored private var agentStoreReconcileInFlight: Task<Bool, Never>?
    @ObservationIgnored private var agentStoreReconcileGeneration: UInt64 = 0
    @ObservationIgnored private var agentStorePeriodicTask: Task<Void, Never>?
    #if DEBUG
    @ObservationIgnored private var devControlsPreload: IOSDevControlsPreload?
    #endif

    init() throws {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
            .appendingPathComponent("com.ombhrum.fabushi", isDirectory: true)
        #if DEBUG
        let featureHostTest = ProcessInfo.processInfo.environment["FABUSHI_FEATURE_HOST_SMOKE"] == "1"
            || ProcessInfo.processInfo.environment["FABUSHI_FEATURE_HOST_TEST"] == "1"
        #else
        let featureHostTest = false
        #endif

        let authCallbackRegistration = try IOSAuthCallbackRegistrar.requireShippingRegistration()
        let main = try IOSMainRuntime(appDataDirectory: base, featureHostTest: featureHostTest)
        let bridge = IOSPrimaryPreloadEntrypoint.install(main: main)
        let surface = FabushiAppAgentSurface()
        let uiPreferencesStore = MobileUiPreferencesStore(coordinator: main.coordinator)
        let humanCallMediaPort = HumanCallMediaPort(
            loadPreferences: {
                let value = main.coordinator.callMediaPreferencesProjection()
                return HumanCallMediaPreferences(
                    microphoneId: value.microphoneId,
                    cameraId: value.cameraId
                )
            },
            savePreferences: { value in
                _ = main.coordinator.updateCallMediaPreferences(
                    microphoneId: value.microphoneId,
                    cameraId: value.cameraId
                )
            }
        )
        self.main = main
        self.bridge = bridge
        self.uiPreferencesStore = uiPreferencesStore
        self.humanCallMediaPort = humanCallMediaPort
        self.authCallbackRegistration = authCallbackRegistration
        cloudAgentWakeWatcher = IOSCloudAgentWakeWatcher(coordinator: main.coordinator)
        #if DEBUG
        devControlsPreload = IOSDevControlsPreloadEntrypoint.installIfEnabled(
            bridge: bridge,
            capability: main.devCapability
        )
        #endif
        appAgentSurface = surface
        marketplace = MarketplaceModel(bridge: bridge)
        messaging = MessagingModel(bridge: bridge)
        remoteDeviceGateway = FabushiRemoteDeviceGateway(
            bridge: bridge,
            surface: surface,
            traceURL: base.appendingPathComponent("device-gateway-trace.jsonl")
        )
        deepLinkController = IOSDeepLinkController(
            dispatch: { [weak self] parsed in
                self?.dispatchGrokDeepLink(parsed)
            }
        )
        HumanCallSystemCoordinator.shared.bind { [weak self] action in
            guard let self else {
                throw HumanCallSystemCoordinatorError.runtimeUnavailable
            }
            try await self.handleHumanCallSystemAction(action)
        }
        HumanCallSystemCoordinator.shared.bindVoIPTokenChangeHandler { [weak self] in
            await self?.remoteDeviceGateway.voIPTokenDidChange()
        }
    }

    func start() async {
        refreshAppVersionPolicy()
        await marketplace.runFeatureHostSmokeIfRequested()
        await marketplace.initializeApp()
        main.coordinator.startAutomationRelay()
        await remoteDeviceGateway.setLoggedIn(marketplace.loggedIn)
        if marketplace.loggedIn {
            await messaging.refresh()
        }
        if marketplace.loggedIn {
            cloudAgentWakeWatcher.start()
        } else {
            cloudAgentWakeWatcher.stop()
        }
        deepLinkController?.markReady()
        if main.requiresColdStartResync {
            await resyncAfterLifecycleRecovery(reason: "cold-start")
        } else if marketplace.loggedIn {
            if await reconcileAgentStore(reason: "startup") {
                main.markResyncCompleted()
            } else {
                main.markResyncRequired()
            }
            startAgentStorePeriodicReconcileIfNeeded()
        } else {
            main.markResyncCompleted()
            stopAgentStorePeriodicReconcile()
        }
    }

    func loginStateChanged(_ loggedIn: Bool) async {
        await remoteDeviceGateway.setLoggedIn(loggedIn)
        if loggedIn {
            cloudAgentWakeWatcher.start()
            main.coordinator.startAutomationRelay()
            main.coordinator.reconcileAutomationRelay()
            if await reconcileAgentStore(reason: "login") {
                main.markResyncCompleted()
            } else {
                main.markResyncRequired()
            }
            if !wasBackgrounded {
                startAgentStorePeriodicReconcileIfNeeded()
            }
        } else {
            cloudAgentWakeWatcher.stop()
            stopAgentStorePeriodicReconcile()
            agentStoreReconcileTask?.cancel()
            agentStoreReconcileTask = nil
            agentStoreReconcileGeneration &+= 1
            agentStoreReconcileInFlight?.cancel()
            agentStoreReconcileInFlight = nil
            // Fence account-scoped relay/fire polling immediately on logout.
            // Account transition cleanup will also clear the durable scope, but
            // transport work must not remain live during that handoff window.
            main.coordinator.stopAutomationRelay()
        }
    }

    func scenePhaseChanged(_ phase: ScenePhase) {
        main.scenePhaseChanged(phase)
        switch phase {
        case .background:
            wasBackgrounded = true
            stopAgentStorePeriodicReconcile()
            scheduleAgentStoreReconcile(reason: "background")
        case .active where wasBackgrounded:
            wasBackgrounded = false
            resumeTask?.cancel()
            resumeTask = Task { [weak self] in
                guard let self else { return }
                await self.resyncAfterLifecycleRecovery(reason: "background-resume")
                self.startAgentStorePeriodicReconcileIfNeeded()
            }
        default:
            break
        }
    }

    func protectedDataWillBecomeUnavailable() {
        main.protectedDataWillBecomeUnavailable()
    }

    func protectedDataDidBecomeAvailable() async {
        main.protectedDataDidBecomeAvailable()
        if !wasBackgrounded {
            await resyncAfterLifecycleRecovery(reason: "protected-data-available")
        }
    }

    func memoryPressureReceived() {
        main.memoryPressureReceived()
    }

    func refreshAppVersionPolicy() {
        appVersionPolicyTask?.cancel()
        let retained = appVersionPolicyState.policy
        appVersionPolicyState = retained == nil ? .loading : .ready(retained!)
        appVersionPolicyTask = Task { [weak self] in
            guard let self else { return }
            let next = await self.main.refreshAppVersionPolicy()
            guard !Task.isCancelled else { return }
            self.appVersionPolicyState = next
        }
    }

    func retryAppVersionPolicy() {
        refreshAppVersionPolicy()
    }

    func retryConnection() async {
        if let connectionRetryTask {
            await connectionRetryTask.value
            return
        }
        resumeTask?.cancel()
        bridge.restartTransport(reason: "user-retry")
        let task = Task { [weak self] in
            guard let self else { return }
            await self.resyncAfterLifecycleRecovery(reason: "user-retry")
        }
        connectionRetryTask = task
        await task.value
        if connectionRetryTask != nil {
            connectionRetryTask = nil
        }
    }

    func handleOpenURL(_ url: URL) {
        _ = deepLinkController?.handleCandidate(
            url.absoluteString,
            origin: "scene-open-url"
        )
    }

    private func handleHumanCallSystemAction(_ action: HumanCallSystemAction) async throws {
        switch action {
        case .incoming:
            _ = try await bridge.request(method: "syncHumanCalls", params: [:])
            await messaging.refresh()
        case .answer(let descriptor):
            _ = try await bridge.request(
                method: "transitionCallSession",
                params: [
                    "callId": descriptor.callId,
                    "generation": descriptor.generation,
                    "action": "accept",
                ]
            )
            await messaging.refresh()
        case .end(let descriptor):
            _ = try await bridge.request(
                method: "transitionCallSession",
                params: [
                    "callId": descriptor.callId,
                    "generation": descriptor.generation,
                    "action": "hangup",
                ]
            )
            await messaging.refresh()
        }
    }

    private func dispatchGrokDeepLink(_ parsed: ParsedFabushiDeepLink) {
        switch parsed.route {
        case .authComplete:
            marketplace.handleDeepLink(parsed.canonicalURL)
        case .agent(let agentID):
            marketplace.message = "已接收智能体链接：\(agentID)"
        case .section(let section):
            marketplace.message = "已接收应用链接：\(section)"
        case .info:
            marketplace.message = "Deep Link 支持已就绪"
        case .open:
            marketplace.message = "已通过 Deep Link 打开 Fabushi"
        case .pluginAdd(let pluginID):
            marketplace.query = pluginID
            Task { [weak self] in
                await self?.marketplace.refresh()
            }
        }
    }

    private func reconcileAgentStore(reason: String) async -> Bool {
        guard marketplace.loggedIn else { return true }

        if let inFlight = agentStoreReconcileInFlight {
            let generation = agentStoreReconcileGeneration
            let completed = await inFlight.value
            guard iosAgentStoreReconcileResultIsCurrent(
                expectedGeneration: generation,
                currentGeneration: agentStoreReconcileGeneration,
                loggedIn: marketplace.loggedIn,
                taskCancelled: Task.isCancelled
            ) else { return false }
            main.lifecycleReporter.report(
                .coordinatorHandoff,
                level: completed ? .info : .warn,
                metadata: [
                    "agent_store": "reconcile",
                    "reason": reason,
                    "result": completed ? "coalesced-completed" : "coalesced-failed",
                ]
            )
            return completed
        }

        agentStoreReconcileGeneration &+= 1
        let generation = agentStoreReconcileGeneration
        let task = Task { [weak self] () -> Bool in
            guard let self, self.marketplace.loggedIn else { return false }
            do {
                _ = try await self.main.coordinator.reconcileAgentStoreForCurrentScope()
                guard iosAgentStoreReconcileResultIsCurrent(
                    expectedGeneration: generation,
                    currentGeneration: self.agentStoreReconcileGeneration,
                    loggedIn: self.marketplace.loggedIn,
                    taskCancelled: Task.isCancelled
                ) else { return false }
                self.main.lifecycleReporter.report(
                    .coordinatorHandoff,
                    metadata: [
                        "agent_store": "reconcile",
                        "reason": reason,
                        "result": "completed",
                    ]
                )
                return true
            } catch is CancellationError {
                return false
            } catch {
                guard iosAgentStoreReconcileResultIsCurrent(
                    expectedGeneration: generation,
                    currentGeneration: self.agentStoreReconcileGeneration,
                    loggedIn: self.marketplace.loggedIn,
                    taskCancelled: Task.isCancelled
                ) else { return false }
                self.main.lifecycleReporter.report(
                    .coordinatorHandoff,
                    level: .warn,
                    metadata: [
                        "agent_store": "reconcile",
                        "reason": reason,
                        "result": "failed",
                        "error": String(error.localizedDescription.prefix(512)),
                    ]
                )
                return false
            }
        }
        agentStoreReconcileInFlight = task
        let completed = await task.value
        let resultIsCurrent = iosAgentStoreReconcileResultIsCurrent(
            expectedGeneration: generation,
            currentGeneration: agentStoreReconcileGeneration,
            loggedIn: marketplace.loggedIn,
            taskCancelled: Task.isCancelled
        )
        if agentStoreReconcileGeneration == generation {
            agentStoreReconcileInFlight = nil
        }
        return completed && resultIsCurrent
    }

    private func scheduleAgentStoreReconcile(reason: String) {
        agentStoreReconcileTask?.cancel()
        guard marketplace.loggedIn else {
            agentStoreReconcileTask = nil
            return
        }
        agentStoreReconcileTask = Task { [weak self] in
            guard let self else { return }
            let completed = await self.reconcileAgentStore(reason: reason)
            guard !Task.isCancelled else { return }
            if completed {
                self.main.markResyncCompleted()
            } else {
                self.main.markResyncRequired()
            }
        }
    }

    private func startAgentStorePeriodicReconcileIfNeeded() {
        guard agentStorePeriodicTask == nil,
              marketplace.loggedIn,
              !wasBackgrounded
        else { return }

        agentStorePeriodicTask = Task { [weak self] in
            guard let self else { return }
            var delay = IOS_AGENT_STORE_RECONCILE_INTERVAL_NS
            while !Task.isCancelled {
                do {
                    try await Task.sleep(nanoseconds: delay)
                } catch {
                    return
                }
                guard !Task.isCancelled,
                      self.marketplace.loggedIn,
                      !self.wasBackgrounded
                else { return }

                let completed = await self.reconcileAgentStore(reason: "periodic")
                guard !Task.isCancelled else { return }
                if completed {
                    self.main.markResyncCompleted()
                    delay = IOS_AGENT_STORE_RECONCILE_INTERVAL_NS
                } else {
                    self.main.markResyncRequired()
                    delay = IOS_AGENT_STORE_ERROR_BACKOFF_NS
                }
            }
        }
    }

    private func stopAgentStorePeriodicReconcile() {
        agentStorePeriodicTask?.cancel()
        agentStorePeriodicTask = nil
    }

    private func resyncAfterLifecycleRecovery(reason: String) async {
        await marketplace.refresh()
        await remoteDeviceGateway.resumeAfterBackground()
        if marketplace.loggedIn {
            await messaging.refresh()
        }
        await cloudAgentWakeWatcher.refreshNow()
        main.coordinator.reconcileAutomationRelay()
        main.coordinator.requestAutomationRelayDrain()
        let agentStoreReady = await reconcileAgentStore(reason: reason)
        if agentStoreReady {
            main.markResyncCompleted()
        } else {
            main.markResyncRequired()
        }
        reconnectGeneration &+= 1
        main.lifecycleReporter.report(
            .coordinatorHandoff,
            level: agentStoreReady ? .info : .warn,
            metadata: [
                "resync_reason": reason,
                "result": agentStoreReady ? "completed" : "agent-store-pending",
            ]
        )
        if marketplace.loggedIn, !wasBackgrounded {
            startAgentStorePeriodicReconcileIfNeeded()
        }
    }
}
