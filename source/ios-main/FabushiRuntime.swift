import Foundation
import Observation
import SwiftUI

private let IOS_CLOUD_AGENT_POLL_INTERVAL_NS: UInt64 = 10_000_000_000
private let IOS_CLOUD_AGENT_MAX_WAIT_MS: Int64 = 5 * 60 * 60 * 1_000
private let IOS_CLOUD_AGENT_RESTART_GRACE_MS: Int64 = 3 * 60 * 1_000

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
                if await settle(wake, status: "completed", result: result) { return }
            case 3, 5:
                let errorText = info.permanentError?.trimmingCharacters(in: .whitespacesAndNewlines)
                    ?? info.summary?.trimmingCharacters(in: .whitespacesAndNewlines)
                let result = (errorText?.isEmpty == false)
                    ? errorText!
                    : "The Cursor agent (\(wake.workId)) ended before finishing."
                if await settle(wake, status: "error", result: result) { return }
            default:
                // Desktop normalizes unknown numeric statuses to Unspecified and keeps polling.
                continue
            }
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
        await remoteDeviceGateway.setLoggedIn(marketplace.loggedIn)
        if marketplace.loggedIn {
            await messaging.refresh()
        }
        cloudAgentWakeWatcher.start()
        deepLinkController?.markReady()
        if main.requiresColdStartResync {
            await resyncAfterLifecycleRecovery(reason: "cold-start")
        } else {
            main.markResyncCompleted()
        }
    }

    func loginStateChanged(_ loggedIn: Bool) async {
        await remoteDeviceGateway.setLoggedIn(loggedIn)
    }

    func scenePhaseChanged(_ phase: ScenePhase) {
        main.scenePhaseChanged(phase)
        switch phase {
        case .background:
            wasBackgrounded = true
        case .active where wasBackgrounded:
            wasBackgrounded = false
            resumeTask?.cancel()
            resumeTask = Task { [weak self] in
                await self?.resyncAfterLifecycleRecovery(reason: "background-resume")
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
            guard let metadata = IOSReleaseMetadataReader.read() else {
                self.appVersionPolicyState = .failed(
                    message: IOSAppStoreUpdateService.ServiceError.releaseMetadataUnavailable.localizedDescription,
                    retained: retained
                )
                return
            }
            do {
                let policy = try await IOSAppVersionPolicyClient.fetch(metadata: metadata)
                guard !Task.isCancelled else { return }
                self.appVersionPolicyState = .ready(policy)
            } catch is CancellationError {
                return
            } catch {
                guard !Task.isCancelled else { return }
                self.appVersionPolicyState = .failed(
                    message: error.localizedDescription,
                    retained: retained
                )
            }
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

    private func resyncAfterLifecycleRecovery(reason: String) async {
        await marketplace.refresh()
        await remoteDeviceGateway.resumeAfterBackground()
        if marketplace.loggedIn {
            await messaging.refresh()
        }
        await cloudAgentWakeWatcher.refreshNow()
        main.markResyncCompleted()
        reconnectGeneration &+= 1
        main.lifecycleReporter.report(
            .coordinatorHandoff,
            metadata: [
                "resync_reason": reason,
                "result": "completed",
            ]
        )
    }
}
